//! ClearSweep desktop shell (Tauri 2): exposes the sweep-core API over IPC, adds a tray icon
//! and runs the background agent (smart cleaning) in a thread while the app is open.
//!
//! - The close button hides the window to the tray when a tray icon exists and the
//!   `closeToTray` setting (default on) allows it; "Quit" in the tray menu really exits.
//! - `--hidden` starts minimised to the tray (for users who autostart the desktop app; the
//!   built-in "Run at startup" setting launches the headless `agent` instead, see
//!   `sweep_core::autostart`). Without a working tray the window is always shown.
//! - Without a usable display / WebView (no `DISPLAY` or `WAYLAND_DISPLAY` on Linux, or the window
//!   cannot be created) the app falls back to browser mode: it starts the local web server and
//!   opens the default browser, exactly like `clearsweep ui`. `CLEARSWEEP_BROWSER=1` or `--browser`
//!   forces that. (A WebView library that is not installed at all stops the executable in the
//!   dynamic linker before any code runs; that case cannot be caught here.)
//! - The in-process agent only starts when no other agent (for example the headless one from
//!   autostart) holds the lock.

use serde_json::Value;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use sweep_cli::notify::DesktopNotifier;
use sweep_core::agent::{self, Agent, AgentLock, Notifier};
use sweep_core::clock::SystemClock;
use sweep_core::features::cleaner::{self, CleanOptions, Source};
use sweep_core::features::settings;
use sweep_core::pkgutil::human_bytes;
use sweep_core::{dispatch, ApiError, CancelToken, Ctx, Job, ProgressEvent};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{ipc::Channel, AppHandle, Manager, RunEvent, State, Wry};
use tauri::{WebviewWindow, WindowEvent};

/// Shared state: the execution context and the cancel tokens of in-flight calls.
struct AppState {
    ctx: Arc<Ctx>,
    calls: Mutex<HashMap<String, CancelToken>>,
    /// A tray icon exists, so hiding the window cannot strand the user.
    tray_ok: AtomicBool,
    /// Asks the in-process agent to stop (app exit).
    agent_stop: Arc<AtomicBool>,
    /// "Smart Cleaning" tray item, kept in step with the saved setting.
    smart_item: Mutex<Option<CheckMenuItem<Wry>>>,
    /// A tray "Clean now" is running.
    cleaning: AtomicBool,
}

/// Run an API method. Progress events stream over `on_progress`; the result is the return value.
#[tauri::command]
async fn api_call(
    state: State<'_, AppState>,
    call_id: String,
    method: String,
    params: Value,
    on_progress: Channel<ProgressEvent>,
) -> Result<Value, ApiError> {
    let token = CancelToken::new();
    state
        .calls
        .lock()
        .unwrap()
        .insert(call_id.clone(), token.clone());
    let ctx = state.ctx.clone();

    let joined = tauri::async_runtime::spawn_blocking(move || {
        let job = Job::new(token, move |ev| {
            let _ = on_progress.send(ev);
        });
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch(&ctx, &method, params, &job)
        }))
        .unwrap_or_else(|_| Err(ApiError::internal("handler panicked")))
    })
    .await;

    state.calls.lock().unwrap().remove(&call_id);
    joined.map_err(|e| ApiError::internal(format!("task failed: {e}")))?
}

/// Request cancellation of an in-flight call (no-op when it already finished).
#[tauri::command]
fn api_cancel(state: State<'_, AppState>, call_id: String) {
    if let Some(t) = state.calls.lock().unwrap().get(&call_id) {
        t.cancel();
    }
}

fn show_main_window(app: &AppHandle) -> Option<WebviewWindow> {
    let w = app.get_webview_window("main")?;
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();
    Some(w)
}

fn open_health_check(app: &AppHandle) {
    if let Some(w) = show_main_window(app) {
        let _ = w.eval("window.location.hash = '#/'");
    }
}

/// Tray "Clean now": the rules enabled in Settings, in the background, reported by a
/// notification. "Ask" about running browsers behaves like "skip" (nobody is at the dialog).
fn clean_now(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.cleaning.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = state.ctx.clone();
    let app = app.clone();
    std::thread::spawn(move || {
        let notifier = DesktopNotifier;
        let s = settings::load(&ctx);
        let opts = CleanOptions::from_settings(&s, true);
        let body = match cleaner::run_clean(&ctx, None, &opts, Source::Manual, &Job::detached()) {
            Ok(r) => format!("Cleaned {}", human_bytes(r.total_bytes)),
            Err(e) => format!("Cleaning failed: {}", e.message),
        };
        notifier.notify("ClearSweep", &body);
        app.state::<AppState>()
            .cleaning
            .store(false, Ordering::SeqCst);
    });
}

fn toggle_smart(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Err(e) = settings::update(&state.ctx, |s| s.smart.enabled = !s.smart.enabled) {
        eprintln!("clearsweep: could not change smart cleaning: {}", e.message);
    }
    refresh_smart_item(app);
}

/// Put the tray checkbox back in step with the saved setting (it may have been changed in
/// the Settings page, or the OS flipped the check before we saved).
fn refresh_smart_item(app: &AppHandle) {
    let state = app.state::<AppState>();
    let on = settings::load(&state.ctx).smart.enabled;
    let guard = state.smart_item.lock().unwrap();
    if let Some(item) = guard.as_ref() {
        let _ = item.set_checked(on);
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<AppState>();
    let smart_on = settings::load(&state.ctx).smart.enabled;
    let open = MenuItem::with_id(app, "open", "Open ClearSweep", true, None::<&str>)?;
    let health = MenuItem::with_id(app, "health", "Health Check", true, None::<&str>)?;
    let clean = MenuItem::with_id(app, "clean", "Clean now", true, None::<&str>)?;
    let smart =
        CheckMenuItem::with_id(app, "smart", "Smart Cleaning", true, smart_on, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &health, &sep, &clean, &smart, &sep2, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("ClearSweep")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => {
                show_main_window(app);
            }
            "health" => open_health_check(app),
            "clean" => clean_now(app),
            "smart" => toggle_smart(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    *state.smart_item.lock().unwrap() = Some(smart);
    Ok(())
}

/// The in-process agent (smart cleaning works while the window is hidden). Does nothing when
/// another agent already holds the lock.
fn start_agent(ctx: Arc<Ctx>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let lock = match AgentLock::try_acquire(&ctx) {
            Ok(Some(l)) => l,
            Ok(None) => return,
            Err(e) => {
                eprintln!("clearsweep: background agent not started: {}", e.message);
                return;
            }
        };
        let mut a = Agent::new(
            (*ctx).clone(),
            Arc::new(SystemClock),
            Arc::new(DesktopNotifier),
        );
        agent::run_loop(&mut a, &stop, |events| {
            for e in events {
                if let agent::Event::Error(m) = e {
                    eprintln!("clearsweep: agent: {m}");
                }
            }
        });
        drop(lock);
    });
}

/// True when a desktop window cannot possibly be shown, so the browser fallback is used at once.
fn force_browser() -> bool {
    let forced = std::env::var_os("CLEARSWEEP_BROWSER").is_some_and(|v| !v.is_empty() && v != "0")
        || std::env::args().any(|a| a == "--browser");
    #[cfg(target_os = "linux")]
    let no_display = std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty())
        && std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty());
    #[cfg(not(target_os = "linux"))]
    let no_display = false;
    forced || no_display
}

/// Browser mode: the same as `clearsweep ui`. A hidden (autostart) launch has nobody to show a
/// browser to, so it just ends.
fn run_browser_fallback(reason: &str, hidden: bool) -> ExitCode {
    eprintln!("clearsweep: no desktop window available ({reason}); using browser mode");
    if hidden {
        return ExitCode::SUCCESS;
    }
    sweep_cli::run(["clearsweep", "ui"].map(std::ffi::OsString::from))
}

pub fn run() -> ExitCode {
    let hidden = std::env::args().any(|a| a == "--hidden");
    if force_browser() {
        return run_browser_fallback("no display or browser mode requested", hidden);
    }
    let agent_stop = Arc::new(AtomicBool::new(false));
    // Window-system initialisation failures panic inside tao/gtk instead of returning an error,
    // so catch both kinds and fall back to the browser.
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tauri::Builder::default()
            .manage(AppState {
                ctx: Arc::new(Ctx::system()),
                calls: Mutex::new(HashMap::new()),
                tray_ok: AtomicBool::new(false),
                agent_stop: agent_stop.clone(),
                smart_item: Mutex::new(None),
                cleaning: AtomicBool::new(false),
            })
            .invoke_handler(tauri::generate_handler![api_call, api_cancel])
            .on_window_event(|window, event| {
                let app = window.app_handle();
                match event {
                    WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                        let state = app.state::<AppState>();
                        if state.tray_ok.load(Ordering::SeqCst)
                            && settings::load(&state.ctx).close_to_tray
                        {
                            api.prevent_close();
                            let _ = window.hide();
                        }
                    }
                    WindowEvent::Focused(true) => refresh_smart_item(app),
                    _ => {}
                }
            })
            .setup(move |app| {
                let state = app.state::<AppState>();
                // A missing tray host (e.g. minimal Linux desktops) must not stop the app.
                match build_tray(app.handle()) {
                    Ok(()) => state.tray_ok.store(true, Ordering::SeqCst),
                    Err(e) => eprintln!("clearsweep: tray icon unavailable: {e}"),
                }
                // The window starts hidden (tauri.conf.json) so `--hidden` never flashes it.
                if !(hidden && state.tray_ok.load(Ordering::SeqCst)) {
                    show_main_window(app.handle());
                }
                start_agent(state.ctx.clone(), state.agent_stop.clone());
                Ok(())
            })
            .build(tauri::generate_context!())
    }));
    match built {
        Ok(Ok(app)) => {
            app.run(move |_app, event| {
                if let RunEvent::Exit = event {
                    agent_stop.store(true, Ordering::SeqCst);
                }
            });
            ExitCode::SUCCESS
        }
        Ok(Err(e)) => run_browser_fallback(&e.to_string(), hidden),
        Err(_) => run_browser_fallback("the window system could not be initialised", hidden),
    }
}

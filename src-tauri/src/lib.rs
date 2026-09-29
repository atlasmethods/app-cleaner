//! ClearSweep desktop shell (Tauri 2): exposes the sweep-core API over IPC and adds a tray icon.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use sweep_core::{dispatch, ApiError, CancelToken, Ctx, Job, ProgressEvent};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{ipc::Channel, AppHandle, Manager, State};

/// Shared state: the execution context and the cancel tokens of in-flight calls.
struct AppState {
    ctx: Arc<Ctx>,
    calls: Mutex<HashMap<String, CancelToken>>,
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

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("ClearSweep")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
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
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            ctx: Arc::new(Ctx::system()),
            calls: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![api_call, api_cancel])
        .setup(|app| {
            // A missing tray host (e.g. minimal Linux desktops) must not stop the app.
            if let Err(e) = build_tray(app.handle()) {
                eprintln!("clearsweep: tray icon unavailable: {e}");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running ClearSweep");
}

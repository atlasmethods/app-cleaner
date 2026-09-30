# ClearSweep architecture

This is the long form of the architecture section of the [README](../README.md).

## Overview

```text
                    +--------------------- React frontend (src/) ---------------------+
                    |  pages -> useCall() -> lib/transport.ts  call(method, params)    |
                    +------------------+-------------------------------+---------------+
                        Tauri webview  |                               | browser tab
                                       v                               v
                      src-tauri: api_call / api_cancel     sweep-server: POST /api/call
                      (progress over a Tauri Channel)      (NDJSON stream, token-guarded)
                                       \                               /
                                        v                             v
                                   sweep_core::dispatch(ctx, method, params, job)
                                                     |
                                   feature handlers (cleaner, startup, restore, ...)

   clearsweep call / clean / analyze / agent  ---->  sweep_core, in-process (no transport)
```

There is one implementation of every feature. The frontend does not know which transport it is on except inside `src/lib/transport.ts`.

## Crates

| Crate | Path | Notes |
| --- | --- | --- |
| `sweep-core` | `crates/sweep-core` | All logic. No UI or transport dependencies. |
| `sweep-server` | `crates/sweep-server` | axum server for browser mode. Embeds `dist/` with `rust-embed` (`build.rs` only makes sure the folder exists). |
| `sweep-cli` | `crates/sweep-cli` | Binary `clearsweep` and a library (`sweep_cli`) that exposes `run()` and the notification helper. |
| `clearsweep-desktop` | `src-tauri` | Tauri 2 shell. Its `main` hands `clean`, `analyze`, `agent` and `call` to `sweep_cli::run` and otherwise starts the window. |

The workspace version, edition and license are set once in the root `Cargo.toml`.

## sweep-core

### The API registry

`sweep_core::api` holds a registry of named handlers, `fn(&Ctx, serde_json::Value, &Job) -> Result<serde_json::Value>`. Every module in `src/features/` has a `METHODS` list and a `register` function; `features::register_all` builds the registry once. `dispatch(ctx, method, params, job)` looks a method up and runs it. Conventions:

- Method names are `feature.verb` (`cleaner.analyze`, `startup.set_enabled`, `restore.list_points`). Parameters and results are JSON with camelCase keys.
- Errors are `ApiError { code, message }` with codes `NotImplemented`, `InvalidParams`, `NotFound`, `Unsupported` (not available on this OS), `PermissionDenied`, `Cancelled`, `Io`, `Internal`.
- Ids that a client sends only select. A destructive method looks the thing up again on the server (a fresh scan or listing) and refuses ids that are not in it. Clients never send commands or paths to delete.
- The `doc comment` at the top of each `features/*/mod.rs` lists that feature's methods and their parameters. `clearsweep call` with a method name runs any of them from a shell.

### Jobs: progress and cancellation

Every handler receives a `Job`: `job.progress(ProgressEvent)` reports `{stage, fraction?, message?, current?, total?}` and `job.check_cancelled()` turns a cooperative `CancelToken` into `Cancelled`. Transports create one token per call and keep it by `callId` so `api_cancel` / `/api/cancel` can trip it.

### Ctx: what makes every OS testable on any host

`Ctx` bundles:

- `Env`: the OS (`Os::Linux | Windows | MacOs`), the root, home, config, cache, data and temp directories. System-wide paths are always built with `Env::sys_path`, so they can be redirected under a test directory.
- a `CommandRunner` (`SystemRunner`, or `MockRunner` in tests): every external program (`dpkg-query`, `winget`, `schtasks`, `launchctl`, `pkexec`, ...) runs through it;
- a process source (real or fake) for running-app detection.

Feature code chooses its platform branch from `ctx.env.os`, not from `cfg!`, so the Windows and macOS parsers and command lines are unit-tested on a Linux CI host. Only the code that calls OS APIs directly (the Windows registry through `winreg`, file identity and free space through `windows-sys`, `libc`) is behind `cfg`.

### Safety layer (`safety.rs`)

- `Protected`: paths that are never deleted (roots, home and its standard folders, `~/.ssh`, `~/.gnupg`, ClearSweep's data folder, per-OS system folders) plus the stricter rules for user-defined include folders.
- `ExcludeSet`: the user's exclusions, absolute paths or globs. Building it fails on an invalid pattern.
- `SafeDeleter`: the only way features remove files. It resolves the parent of the target, never the leaf (a symlink is removed as a link), requires the result to lie strictly below an allowed base, and refuses protected and excluded paths. Analysis uses the same checks without deleting, so an analysis reports what a clean would accept.
- Directory walks use `walkdir` with `follow_links(false)` and `same_file_system(true)`.

### The cleaner and its rules

Rules are TOML files embedded with `include_str!` (`features/cleaner/rules/*.toml`, format in that folder's README). Target kinds are `files`, `file`, `sqlite`, `cookies`, `command`, `trash` and `registry` (Windows). One engine runs in either `Analyze` or `Clean` mode over the same scanning code, which is what guarantees that a clean removes what the analysis reported. SQLite databases are opened `immutable=1` for analysis.

### OS integration

- `elevate.rs`: `run_privileged` runs a program with administrator rights through `pkexec` (Linux), `osascript` (macOS) or PowerShell `Start-Process -Verb RunAs` (Windows), and runs it directly when already elevated.
- `osjobs.rs` generates the text of XDG autostart entries, systemd units, crontab lines, launchd plists and Windows command lines; `autostart.rs` (the agent at login) and `features/scheduler/sync.rs` (scheduled cleans) install them.
- The programs written into OS jobs are `<exe> agent` and `<exe> clean --auto --source scheduled --schedule <id>`, where `<exe>` is the running executable (`CLEARSWEEP_EXE` overrides it in tests). Either binary can be that executable.

### Background agent (`agent.rs`)

A state machine with no thread of its own; a driver calls `tick()` about once a second. Its tasks: measure junk every `smart.checkIntervalMinutes`; watch browsers listed in `smart.cleanOnBrowserClose` every 10 s; enforce sleep mode every `smart.enforceSleepMinutes`; catch up missed scheduled cleans every minute. One agent runs per user (an exclusive lock on `agent.lock`). The CLI `agent` command and the desktop app's background thread are the two drivers.

### The data folder

`<user data dir>/clearsweep` (`~/.local/share/clearsweep` on Linux, `%APPDATA%\clearsweep` on Windows, `~/Library/Application Support/clearsweep` on macOS), which Settings > About can open. Contents: `settings.json`, `history.json` (last 100 cleans), `health-last.json`, `schedules.json` (plus a lock file), `optimizer.json` (what sleep mode changed), `uninstalled-apps.json` (applications ClearSweep uninstalled, which gates leftover removal), `agent.lock`, `agent-status.json` and `backups/` (`registry-<ts>/`, `config-<ts>/`, `startup-<ts>/`, `plugins-<ts>/`, `uninstall-<ts>.reg`, `drivers-<ts>/`).

## sweep-server (browser mode)

- Binds `127.0.0.1` only. `serve()` returns the launch URL `http://127.0.0.1:<port>/?t=<token>` and a handle.
- `POST /api/call` with `{callId, method, params}` answers `application/x-ndjson`: zero or more `{"type":"progress", ...}` lines, then exactly one `{"type":"result","value":...}` or `{"type":"error","error":{code,message}}` line. Dropping the connection cancels the job. `POST /api/cancel` with `{callId}` cancels by id. `POST /api/heartbeat` is sent every 5 s by each open page.
- Guards: every route checks that the `Host` header is `127.0.0.1:<port>` or `localhost:<port>`. Everything under `/api` also requires the `X-Sweep-Token` header (constant-time comparison) and, when an `Origin` header is present, that it is the server's own.
- Static files come from the embedded `dist/` (from disk in debug builds) with a restrictive Content-Security-Policy; extension-less routes fall back to the SPA shell.
- With `exit_on_idle` (the default), the server stops when heartbeats were received and then stop for `idle_timeout` (90 s by default).

## sweep-cli

`clap` definitions in `crates/sweep-cli/src/lib.rs`: `ui`, `call`, `clean`, `analyze`, `agent`, and (only with the `testutil` feature) the hidden `dev-fixture`. `HEADLESS_COMMANDS` is the list the desktop executable forwards. Desktop notifications use `notify-rust`; a failure to notify is logged and ignored.

## Desktop shell (`src-tauri`)

- Two IPC commands: `api_call(callId, method, params, onProgress)` runs `dispatch` on a blocking thread with a `Channel` for progress, and `api_cancel(callId)`.
- Tray icon (Open, Health Check, Clean now, Smart Cleaning toggle, Quit), close-to-tray handling, `--hidden`, and an in-process agent thread that only starts if it gets the agent lock.
- If there is no display, the window cannot be created, or `--browser` / `CLEARSWEEP_BROWSER=1` is given, it runs the same code as `clearsweep ui`.
- The window starts hidden (`tauri.conf.json`: 380 x 640, minimum 320 x 560) and is shown once set up, so `--hidden` never flashes it. The only capability is `core:default`.

## Frontend (`src`)

- React 18, React Router (hash router), Tailwind CSS 4, Vite 8, TypeScript, `lucide-react` icons.
- `src/api/*.ts` has one typed module per feature (method names and result types). `src/hooks/useCall.ts` wraps a call with loading, progress, error and cancel state.
- `src/lib/transport.ts` exports `call(method, params, {onProgress, signal})`. In Tauri it uses the IPC commands; in a browser it posts to `/api/call` and reads the NDJSON stream with the token from `?t=` (kept in `sessionStorage`, removed from the address bar). A 401 shows the "open from link" page. In a browser it starts the heartbeat.
- Layout: compact (phone width, bottom tab bar) below 900 px or when "Compact mode" is on; otherwise a navigation rail and a 720 px content column (`src/lib/layout.ts`). Pages live in `src/pages`; `src/nav.ts` defines the tabs and the Tools grid. Text goes through `src/i18n` (only `en.json` exists).

## Tests

| Layer | Where | Notes |
| --- | --- | --- |
| Rust unit tests | next to the code (`tests.rs` per feature) | Mock `CommandRunner`, fake registries and process lists; `Env::for_test` builds a throwaway machine under a temp dir. |
| Rust integration | `crates/*/tests`, `crates/sweep-core/tests` | Server protocol and security, CLI, agent, startup; real-system tests are `#[ignore]`d and run by `scripts/check.sh` as appropriate (root, dpkg, a Chromium). |
| Frontend unit | `src/**/*.test.ts(x)` | Vitest + Testing Library on jsdom. |
| End to end | `tests/e2e` | Playwright against a real `clearsweep ui` server on a fake machine built by the `dev-fixture` command. |
| Desktop smoke | `scripts/desktop-smoke.py` | Real WebKit webview under `xvfb-run`. |

The test-only environment variables are listed in the [README](../README.md#test-only-environment-hooks).

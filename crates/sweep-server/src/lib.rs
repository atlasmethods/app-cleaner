//! ClearSweep browser-mode server.
//!
//! Serves the embedded frontend and exposes the core API over a small local HTTP
//! protocol. Everything under `/api` requires the per-launch token; every route is
//! protected against DNS rebinding by validating the `Host` header.

mod security;
mod static_files;

use axum::body::Body;
use axum::extract::{Json, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use sweep_core::{dispatch, ApiError, CancelToken, Ctx, Job};
use tokio::sync::{mpsc, Notify};
use tokio_stream::wrappers::UnboundedReceiverStream;

pub use security::generate_token;

/// Options for [`serve`].
pub struct ServeOptions {
    /// 0 picks a random free port.
    pub port: u16,
    /// Per-launch secret required in the `X-Sweep-Token` header.
    pub token: String,
    /// Shut down when heartbeats were received and then stop for `idle_timeout`.
    pub exit_on_idle: bool,
    /// Serve the frontend from this directory instead of the embedded copy.
    pub dist_override: Option<PathBuf>,
    pub idle_timeout: Duration,
    /// Context to run API calls in; defaults to [`Ctx::system`].
    pub ctx: Option<Arc<Ctx>>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            port: 0,
            token: generate_token(),
            exit_on_idle: true,
            dist_override: None,
            idle_timeout: Duration::from_secs(90),
            ctx: None,
        }
    }
}

pub(crate) struct AppState {
    pub token: String,
    pub port: u16,
    pub ctx: Arc<Ctx>,
    pub dist: Option<PathBuf>,
    calls: Mutex<HashMap<String, CancelToken>>,
    last_heartbeat: Mutex<Option<Instant>>,
}

impl AppState {
    fn cancel_all(&self) {
        for t in self.calls.lock().unwrap().values() {
            t.cancel();
        }
    }
}

/// Handle to a running server.
pub struct ServerHandle {
    pub port: u16,
    pub token: String,
    state: Arc<AppState>,
    shutdown: Arc<Notify>,
    join: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    /// `http://127.0.0.1:<port>`
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    /// Ask the server to stop (in-flight calls are cancelled).
    pub fn shutdown(&self) {
        self.state.cancel_all();
        self.shutdown.notify_waiters();
        self.shutdown.notify_one();
    }
    /// Wait until the server has stopped.
    pub async fn wait(self) {
        let _ = self.join.await;
    }
}

/// Start the server. Returns the full launch URL (including `?t=<token>`) and a handle.
pub async fn serve(opts: ServeOptions) -> std::io::Result<(String, ServerHandle)> {
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], opts.port))).await?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(AppState {
        token: opts.token.clone(),
        port,
        ctx: opts.ctx.unwrap_or_else(|| Arc::new(Ctx::system())),
        dist: opts.dist_override,
        calls: Mutex::new(HashMap::new()),
        last_heartbeat: Mutex::new(None),
    });
    let shutdown = Arc::new(Notify::new());

    let app = router(state.clone());

    let sd = shutdown.clone();
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { sd.notified().await })
            .await;
    });

    if opts.exit_on_idle {
        let st = state.clone();
        let sd = shutdown.clone();
        let timeout = opts.idle_timeout;
        let tick = (timeout / 4).clamp(Duration::from_millis(50), Duration::from_secs(1));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tick).await;
                let idle = st
                    .last_heartbeat
                    .lock()
                    .unwrap()
                    .map(|t| t.elapsed() > timeout);
                if idle == Some(true) {
                    st.cancel_all();
                    sd.notify_one();
                    break;
                }
            }
        });
    }

    let url = format!("http://127.0.0.1:{port}/?t={}", opts.token);
    Ok((
        url,
        ServerHandle {
            port,
            token: opts.token,
            state,
            shutdown,
            join,
        },
    ))
}

fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/call", post(call))
        .route("/cancel", post(cancel))
        .route("/heartbeat", post(heartbeat))
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found") })
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::api_guard,
        ));

    Router::new()
        .nest("/api", api)
        .fallback(static_files::handler)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::host_guard,
        ))
        .with_state(state)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallRequest {
    call_id: String,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelRequest {
    call_id: String,
}

fn line(v: Value) -> Result<Bytes, Infallible> {
    let mut s = v.to_string();
    s.push('\n');
    Ok(Bytes::from(s))
}

/// Response body stream that cancels the job when dropped (client disconnect).
struct JobStream {
    rx: UnboundedReceiverStream<Result<Bytes, Infallible>>,
    token: CancelToken,
}

impl Stream for JobStream {
    type Item = Result<Bytes, Infallible>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl Drop for JobStream {
    fn drop(&mut self) {
        // No-op when the job already finished.
        self.token.cancel();
    }
}

async fn call(State(state): State<Arc<AppState>>, Json(req): Json<CallRequest>) -> Response {
    let token = CancelToken::new();
    state
        .calls
        .lock()
        .unwrap()
        .insert(req.call_id.clone(), token.clone());

    let (tx, rx) = mpsc::unbounded_channel::<Result<Bytes, Infallible>>();
    let ctx = state.ctx.clone();
    let job_token = token.clone();
    let st = state.clone();
    tokio::task::spawn_blocking(move || {
        let ptx = tx.clone();
        let job = Job::new(job_token.clone(), move |ev| {
            if let Ok(Value::Object(mut m)) = serde_json::to_value(&ev) {
                m.insert("type".into(), json!("progress"));
                let _ = ptx.send(line(Value::Object(m)));
            }
        });
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch(&ctx, &req.method, req.params, &job)
        }))
        .unwrap_or_else(|_| Err(ApiError::internal("handler panicked")));
        let last = match res {
            Ok(value) => json!({"type": "result", "value": value}),
            Err(error) => json!({"type": "error", "error": error}),
        };
        let _ = tx.send(line(last));
        st.calls.lock().unwrap().remove(&req.call_id);
    });

    let stream = JobStream {
        rx: UnboundedReceiverStream::new(rx),
        token,
    };
    let mut resp = Response::new(Body::from_stream(stream));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

async fn cancel(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CancelRequest>,
) -> impl IntoResponse {
    let found = match state.calls.lock().unwrap().get(&req.call_id) {
        Some(t) => {
            t.cancel();
            true
        }
        None => false,
    };
    Json(json!({ "ok": true, "found": found }))
}

async fn heartbeat(State(state): State<Arc<AppState>>) -> StatusCode {
    *state.last_heartbeat.lock().unwrap() = Some(Instant::now());
    StatusCode::NO_CONTENT
}

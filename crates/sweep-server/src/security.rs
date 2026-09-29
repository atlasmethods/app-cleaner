//! Request guards: Host check (all routes), token + Origin checks (`/api/*`).

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::AppState;

/// 32 random bytes, hex encoded.
pub fn generate_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("OS random number generator unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn allowed_authorities(port: u16) -> [String; 2] {
    [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
}

/// DNS-rebinding defence: the Host header must be a loopback name with our port.
pub async fn host_guard(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_ascii_lowercase);
    match host {
        Some(h) if allowed_authorities(state.port).contains(&h) => next.run(req).await,
        _ => (StatusCode::FORBIDDEN, "forbidden: bad Host header").into_response(),
    }
}

/// Token must match exactly (constant time); a present Origin must be our own.
pub async fn api_guard(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let supplied = req
        .headers()
        .get("x-sweep-token")
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    let ok = !supplied.is_empty() && bool::from(supplied.ct_eq(state.token.as_bytes()));
    if !ok {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let good = origin.to_str().ok().is_some_and(|o| {
            allowed_authorities(state.port)
                .iter()
                .any(|a| o.eq_ignore_ascii_case(&format!("http://{a}")))
        });
        if !good {
            return (StatusCode::FORBIDDEN, "forbidden: bad Origin").into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_hex_chars_and_random() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}

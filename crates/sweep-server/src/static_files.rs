//! Static frontend: embedded `dist/` (read from disk in debug builds) or a directory override.

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::path::{Component, Path};
use std::sync::Arc;

use crate::AppState;

#[derive(RustEmbed)]
#[folder = "../../dist"]
struct Assets;

const CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
                   connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn load(state: &AppState, rel: &str) -> Option<Vec<u8>> {
    match &state.dist {
        Some(dir) => {
            let p = Path::new(rel);
            if p.components().any(|c| !matches!(c, Component::Normal(_))) {
                return None;
            }
            std::fs::read(dir.join(p)).ok()
        }
        None => Assets::get(rel).map(|f| f.data.into_owned()),
    }
}

pub async fn handler(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    let rel = uri.path().trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };

    let (path, data) = match load(&state, rel) {
        Some(d) => (rel, d),
        None => {
            let last = rel.rsplit('/').next().unwrap_or(rel);
            // Missing asset files are real 404s; extension-less routes fall back to the SPA shell.
            if last.contains('.') {
                return (StatusCode::NOT_FOUND, "not found").into_response();
            }
            match load(&state, "index.html") {
                Some(d) => ("index.html", d),
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        "frontend not built: run `pnpm build` first",
                    )
                        .into_response()
                }
            }
        }
    };

    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut resp = Response::new(axum::body::Body::from(data));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref()).unwrap(),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if path == "index.html" {
            "no-store"
        } else {
            "public, max-age=3600"
        }),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

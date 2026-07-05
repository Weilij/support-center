//! Request-scoped correlation id (CRD §7.1). Reuse an inbound `X-Request-ID` (or mint
//! one), expose it through a task-local so response envelopes AND tracing logs share
//! the same id, store it in request extensions for extractors, and echo it back in the
//! `X-Request-ID` response header.

use axum::body::Body;
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

tokio::task_local! {
    /// The current request's correlation id. Set for the duration of each HTTP
    /// request; absent in non-HTTP contexts (background workers, tests), where
    /// `envelope::request_id()` falls back to generating one.
    pub static REQUEST_ID: String;
}

/// Correlation id stored in request extensions for handlers/extractors that want it.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

const HEADER: &str = "x-request-id";
const MAX_LEN: usize = 200;

/// Accept an inbound id only if it is a short, header/log-safe token; otherwise mint one.
fn sanitize(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > MAX_LEN
        || !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(raw.to_string())
}

pub async fn request_id_layer(mut req: Request<Body>, next: Next) -> Response {
    let id = req
        .headers()
        .get(HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(sanitize)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    req.extensions_mut().insert(RequestId(id.clone()));

    let span = tracing::info_span!("request", request_id = %id);
    let mut resp = REQUEST_ID
        .scope(id.clone(), next.run(req).instrument(span))
        .await;

    if let Ok(value) = HeaderValue::from_str(&id) {
        resp.headers_mut().insert(HEADER, value);
    }
    resp
}

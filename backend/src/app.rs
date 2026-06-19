//! Router assembly reproducing the CRD §7.1 pipeline order (lines 5673-5684):
//! CORS → metrics → public/priority routes → domain routes → error trap (per-handler) →
//! security headers (post-handler) → root probe / unknown-route fallback.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{middleware as axum_mw, Json, Router};
use serde_json::json;
use std::sync::Arc;

use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(root_probe))
        // Public/priority realtime gateway routes (CRD route precedence): the
        // WS upgrade paths authenticate via query-param token during the
        // handshake and must not sit behind the bearer-auth catch-alls.
        .merge(crate::realtime::routes(state.clone()))
        // Public inbound webhook ingress (CRD §4.2): signature-verified, no
        // JWT; mounted with the public/priority routes so the endpoints can
        // never sit behind (or be shadowed by) authenticated patterns.
        .merge(crate::domain::webhooks::routes(state.clone()))
        .merge(crate::domain::auth::routes(state.clone()))
        .merge(crate::domain::tags::routes(state.clone()))
        .merge(crate::domain::customers::routes(state.clone()))
        .merge(crate::domain::teams::routes(state.clone()))
        .merge(crate::domain::agents::routes(state.clone()))
        .merge(crate::domain::activity::routes(state.clone()))
        .merge(crate::domain::conversations::routes(state.clone()))
        .merge(crate::domain::sessions::routes(state.clone()))
        .merge(crate::domain::messaging::routes(state.clone()))
        .merge(crate::domain::customer_conversations::routes(state.clone()))
        .merge(crate::domain::channels::routes(state.clone()))
        .merge(crate::domain::auto_reply::routes(state.clone()))
        .merge(crate::domain::delayed_messages::routes(state.clone()))
        .merge(crate::domain::liff::routes(state.clone()))
        .merge(crate::domain::files::routes(state.clone()))
        .merge(crate::domain::queue::routes(state.clone()))
        .merge(crate::domain::notifications::routes(state.clone()))
        .merge(crate::domain::monitoring::routes(state.clone()))
        .merge(crate::domain::analytics::routes(state.clone()))
        .merge(crate::domain::reports::routes(state.clone()))
        .merge(crate::domain::system::routes(state.clone()))
        .fallback(unknown_route)
        .layer(axum_mw::from_fn(
            crate::middleware::security_headers::security_headers_layer,
        ))
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            crate::middleware::csrf::csrf_layer,
        ))
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            crate::middleware::metrics::metrics_layer,
        ))
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            crate::middleware::cors::cors_layer,
        ))
        .with_state(state)
}

/// Service root probe per CRD 5632-5635.
async fn root_probe() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "message": "Multi-Channel Customer Support System API",
            "timestamp": crate::db::now_iso(),
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
        .into_response()
}

/// Unknown-route fallback per CRD 5637-5640 — note: no `success` flag, by spec.
async fn unknown_route(req: axum::extract::Request) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": "Not Found",
            "message": format!("The requested endpoint {} was not found", req.uri().path()),
            "timestamp": crate::db::now_iso(),
        })),
    )
        .into_response()
}

//! Conversation-Session Management (CRD §1.2B, lines 329-483), mounted at
//! `/api/sessions`.
//!
//! Module-wide gates: health/info are open; all other routes require ops-area
//! authorization (supervisor/system_admin). Mutating and creating endpoints also
//! carry a 60 req / 60 s "session"-scoped rate limit, a 1 MB
//! declared-content-length cap (413), and an endpoint-catalog 404 for unknown
//! paths under the module.

pub mod handlers;
pub mod store;
pub mod topics;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

use crate::middleware::auth::require_auth;
use crate::middleware::rate_limit::{self, RatePolicy};
use crate::state::AppState;

/// Per-client mutation budget under the "session" namespace (CRD 331).
const SESSION_RATE: RatePolicy = RatePolicy {
    scope: "session",
    max_requests: 60,
    window: Duration::from_secs(60),
};

const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// Declared-content-length cap: oversized bodies yield 413 (CRD 331).
async fn size_limit(req: Request<Body>, next: Next) -> Response {
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_BODY_BYTES) {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "success": false,
                "error": "Request body too large (max 1MB)",
                "timestamp": crate::db::now_iso(),
            })),
        )
            .into_response();
    }
    next.run(req).await
}

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Health and info are open (no auth middleware, CRD 335-341).
    let open = Router::new()
        .route("/health", get(handlers::health))
        .route("/info", get(handlers::info));

    // Mutating/creating endpoints carry the session-scoped rate limit (CRD 331).
    let rate_limited = Router::new()
        .route("/", post(handlers::create_session))
        .route("/batch", post(handlers::batch))
        .route("/get-or-create", post(handlers::get_or_create))
        .route(
            "/{sessionId}",
            put(handlers::update_session).delete(handlers::delete_session),
        )
        .route("/{sessionId}/close", post(handlers::close_session))
        .route("/{sessionId}/reopen", post(handlers::reopen_session))
        .layer(from_fn(rate_limit::limit(state.clone(), SESSION_RATE)));

    let plain = Router::new()
        .route("/", get(handlers::list_sessions))
        .route("/search", get(handlers::search_sessions))
        .route("/stats", get(handlers::stats))
        .route(
            "/stats/{conversationId}",
            get(handlers::stats_for_conversation),
        )
        .route("/activity", get(handlers::activity_stats))
        .route("/detect-boundary", post(handlers::detect_boundary))
        .route("/topics/stats", get(handlers::topic_stats))
        .route("/topics/analyze", post(handlers::analyze_topic))
        .route("/topics/suggest", post(handlers::suggest_topics))
        .route("/{sessionId}", get(handlers::get_session))
        .route("/{sessionId}/messages", get(handlers::session_messages))
        .route("/{sessionId}/health", get(handlers::session_health))
        .route("/{sessionId}/topic", put(handlers::update_topic));

    let authed = rate_limited
        .merge(plain)
        .layer(from_fn_with_state(state.clone(), require_auth));

    let module = open
        .merge(authed)
        // Unknown paths under the module return the endpoint catalog (CRD 470-471).
        .fallback(handlers::unmatched)
        .layer(from_fn(size_limit));

    Router::new().nest("/api/sessions", module)
}

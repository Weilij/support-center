//! Conversations (Agent Side) (CRD §2.1, lines 651-830), mounted at
//! `/api/conversations` (CRD 1463, 6560-6576).
//!
//! The conversation-label family (`GET/POST/DELETE /api/conversations/{id}/tags`,
//! CRD 728-753) is implemented in `crate::domain::tags`, whose behavior already
//! matches this section (auth-only access, conversation-existence check, identical
//! success messages); it is not duplicated here.

pub mod channels;
pub mod handlers;
pub mod store;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/conversations", get(handlers::list_conversations))
        .route("/api/conversations/bulk", post(handlers::bulk))
        .route("/api/conversations/{id}", get(handlers::detail))
        .route("/api/conversations/{id}/read", put(handlers::mark_read))
        .route("/api/conversations/{id}/assign", post(handlers::assign))
        .route("/api/conversations/{id}/unassign", post(handlers::unassign))
        .route("/api/conversations/{id}/transfer", post(handlers::transfer))
        .route(
            "/api/conversations/{id}/messages",
            get(handlers::list_messages).post(handlers::send_message),
        )
        .route(
            "/api/conversations/{id}/attachments",
            // Raise the transport body cap so the documented 10 MB application
            // limit (CRD 779, 782) is what callers observe, not the framework's
            // default 2 MB limit.
            post(handlers::upload_attachment)
                .layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

//! Customer-Facing Conversations (CRD §2.3, lines 1042-1170), mounted at
//! `/api/customer-conversations` plus the `/api/customer-ws` upgrade target.
//!
//! These routes do not use the bearer-auth middleware: each handler performs
//! the section's own session-credential validation (X-Session-Id /
//! Authorization Bearer / sessionId query) and the shared four-way access rule
//! (CRD 1045, 1053, 1130-1135).

pub mod handlers;

use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::state::AppState;

pub fn routes(_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/customer-conversations/{conversationId}/messages",
            get(handlers::history).post(handlers::send_reply),
        )
        .route(
            "/api/customer-conversations/{conversationId}/upload",
            post(handlers::upload)
                .layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route("/api/customer-ws", get(handlers::subscribe_ws))
}

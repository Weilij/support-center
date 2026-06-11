//! Messaging (CRD §2.2, lines 830-1042), mounted at `/api/messages` (CRD 835).
//!
//! All endpoints require a valid bearer credential except the two informational
//! endpoints (health / info), which are public (CRD 835, 840, 845). The
//! delayed-send / recall / offline-buffer service capabilities (CRD 983-1006,
//! 1018, 1038) live in [`service`] and are invoked programmatically rather than
//! over HTTP.

pub mod handlers;
pub mod service;
pub mod store;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let public = Router::new()
        .route("/api/messages/health", get(handlers::health))
        .route("/api/messages/info", get(handlers::info));

    let protected = Router::new()
        .route("/api/messages", post(handlers::create_message))
        .route("/api/messages/search", get(handlers::search_messages))
        .route("/api/messages/stats", get(handlers::stats))
        .route("/api/messages/tags", get(handlers::list_tags))
        .route("/api/messages/export", get(handlers::export_messages))
        .route("/api/messages/export/count", get(handlers::export_count))
        .route("/api/messages/export/customers", get(handlers::export_customers))
        .route("/api/messages/export/agents", get(handlers::export_agents))
        .route("/api/messages/bulk-create", post(handlers::bulk_create))
        .route("/api/messages/bulk-delete", post(handlers::bulk_delete))
        .route(
            "/api/messages/conversation/{conversationId}",
            get(handlers::conversation_messages),
        )
        .route(
            "/api/messages/{id}",
            get(handlers::get_message)
                .put(handlers::update_message)
                .delete(handlers::recall_message),
        )
        .route(
            "/api/messages/{id}/attachments",
            // Raise the transport body cap so the documented 10 MB application
            // limit (CRD 957) is what callers observe, not the framework's
            // default 2 MB limit.
            get(handlers::list_attachments)
                .post(handlers::upload_attachment)
                .layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route("/api/messages/{id}/forward", post(handlers::forward_message))
        .route(
            "/api/messages/{id}/tags",
            put(handlers::set_tags).delete(handlers::remove_tags),
        )
        .layer(from_fn_with_state(state.clone(), require_auth));

    public.merge(protected)
}

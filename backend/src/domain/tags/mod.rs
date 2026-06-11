//! Tags & Labeling (CRD §2.6, lines 1453-1644).
//!
//! Route families implemented here: label management under `/api/tags` and the
//! conversation-label association family under `/api/conversations/{id}/tags`.
//! The customer-label association family lives in `crate::domain::customers`
//! (routes rooted at `/api/customers`).

pub mod handlers;
pub mod store;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // The health probe is explicitly exempt from auth enforcement (CRD 1468).
    let public = Router::new().route("/api/tags/health", get(handlers::health));

    let authed = Router::new()
        .route("/api/tags", get(handlers::list_tags).post(handlers::create_tag))
        .route("/api/tags/bulk", post(handlers::bulk_operation))
        .route(
            "/api/tags/{id}",
            get(handlers::get_tag).put(handlers::update_tag).delete(handlers::delete_tag),
        )
        .route("/api/tags/{id}/stats", get(handlers::tag_stats))
        .route("/api/tags/{id}/customers", get(handlers::tag_customers))
        .route("/api/tags/{id}/conversations", get(handlers::tag_conversations))
        .route(
            "/api/conversations/{id}/tags",
            get(handlers::conversation_tags)
                .post(handlers::add_conversation_tags)
                .delete(handlers::remove_conversation_tags),
        )
        .layer(from_fn_with_state(state.clone(), require_auth));

    public.merge(authed)
}

//! Channel Integrations (CRD §4.1, lines 2612-2720), mounted at `/api/channels`.
//!
//! Per-team messaging-platform connections with encrypted credential storage
//! (`crate::crypto`, CRD 5716-5727), generated per-connection inbound webhook
//! addresses with a secret routing token (resolvable via
//! `store::resolve_by_webhook_token`, CRD 2722), live verification, usage
//! statistics and a derived health indicator.

pub mod handlers;
pub mod resolve;
pub mod store;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/channels",
            get(handlers::list_channels).post(handlers::create_channel),
        )
        .route(
            "/api/channels/{id}",
            get(handlers::get_channel)
                .put(handlers::update_channel)
                .delete(handlers::delete_channel),
        )
        .route("/api/channels/{id}/verify", post(handlers::verify_channel))
        .route("/api/channels/{id}/stats", get(handlers::channel_stats))
        .route("/api/channels/{id}/health", get(handlers::channel_health))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

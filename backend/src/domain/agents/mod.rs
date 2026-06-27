//! Agents/Operators (CRD §3.3, lines 2154-2321): operator listing/search/profile
//! management, bulk update/transfer, skill inventory, and presence system.
//!
//! Note: operator creation has no exposed endpoint in this module (CRD 2299).

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
        .route("/api/agents", get(handlers::list_agents))
        .route("/api/agents/batch", put(handlers::batch_update))
        .route("/api/agents/batch/transfer", put(handlers::batch_transfer))
        .route("/api/agents/search", post(handlers::search_agents))
        .route(
            "/api/agents/status/statistics",
            get(handlers::status_statistics),
        )
        .route(
            "/api/agents/{agentId}",
            get(handlers::get_agent)
                .put(handlers::update_agent)
                .delete(handlers::delete_agent),
        )
        .route(
            "/api/agents/{agentId}/skills",
            get(handlers::get_skills).post(handlers::add_skill),
        )
        .route(
            "/api/agents/{agentId}/skills/statistics",
            get(handlers::skill_statistics),
        )
        .route(
            "/api/agents/{agentId}/skills/{skillId}",
            put(handlers::update_skill).delete(handlers::delete_skill),
        )
        .route(
            "/api/agents/{agentId}/status",
            get(handlers::get_status).put(handlers::update_status),
        )
        .route(
            "/api/agents/{agentId}/status/history",
            get(handlers::status_history),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

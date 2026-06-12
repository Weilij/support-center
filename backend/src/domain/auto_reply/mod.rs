//! Auto-Reply (CRD §2.5, lines 1334-1451): rule/schedule/log management plus
//! the internal evaluation engine triggered by inbound webhook processing.

pub mod engine;
pub mod handlers;

use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/auto-reply/rules",
            get(handlers::list_rules).post(handlers::create_rule),
        )
        // Health routes are registered before the {id} routes so the static
        // segment cannot be captured as a rule id.
        .route("/api/auto-reply/rules/health", get(|| async {
            handlers::health(axum::extract::Path("rules".to_string())).await
        }))
        .route("/api/auto-reply/schedules/health", get(|| async {
            handlers::health(axum::extract::Path("schedules".to_string())).await
        }))
        .route("/api/auto-reply/logs/health", get(|| async {
            handlers::health(axum::extract::Path("logs".to_string())).await
        }))
        .route(
            "/api/auto-reply/rules/{id}",
            axum::routing::put(handlers::update_rule).delete(handlers::delete_rule),
        )
        .route(
            "/api/auto-reply/schedules",
            get(handlers::get_schedules).post(handlers::replace_schedules),
        )
        .route("/api/auto-reply/logs", get(handlers::list_logs))
        .layer(from_fn_with_state(state, require_auth))
}

//! Monitoring & Health (CRD §6.3, lines 4697-4879).

pub mod center;
pub mod handlers;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // The liveness probe is public and excluded from traffic metrics
    // (the metrics middleware already skips /api/monitoring, CRD 4712).
    let public = Router::new().route("/api/monitoring/health", get(handlers::public_health));

    let authed = Router::new()
        .route("/api/monitoring/metrics", get(handlers::metrics))
        .route("/api/monitoring/alerts", get(handlers::alerts))
        .route(
            "/api/monitoring/alerts/history",
            get(handlers::alert_history),
        )
        .route(
            "/api/monitoring/circuit-breaker/status",
            get(handlers::breaker_status),
        )
        .route(
            "/api/monitoring/circuit-breaker/reset",
            post(handlers::breaker_reset),
        )
        .route(
            "/api/monitoring/circuit-breaker/open",
            post(handlers::breaker_open),
        )
        .route(
            "/api/monitoring/instances/{type}",
            get(handlers::instances_by_type),
        )
        .route(
            "/api/monitoring/health-check",
            post(handlers::manual_health_check),
        )
        .route("/api/monitoring/dashboard", get(handlers::dashboard))
        .route(
            "/api/monitoring/health/history",
            get(handlers::health_history),
        )
        .route(
            "/api/monitoring/health/check",
            post(handlers::app_health_check),
        )
        .route("/api/monitoring/config", put(handlers::update_config))
        .route("/api/monitoring/stats", get(handlers::monitor_stats))
        .layer(from_fn_with_state(state, require_auth));

    public.merge(authed)
}

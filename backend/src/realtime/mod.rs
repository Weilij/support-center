//! Real-time infrastructure: WebSocket gateway & protocol (CRD §5.1, lines
//! 3221-3467) plus the §1.3 real-time connection gate (lines 596-646).
//!
//! The WS upgrade paths are PUBLIC routes (no bearer middleware): browsers
//! cannot send custom headers on upgrade, so the credential travels as a
//! `token` query parameter and is verified during the handshake (CRD 597).
//! Per-route authentication for the HTTP surface happens in-handler.

pub mod endpoints;
pub mod gate;
pub mod hub;
pub mod socket;

pub use hub::RealtimeHub;

use axum::middleware::from_fn;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::middleware::rate_limit::{self, RatePolicy};
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Connect/disconnect carry the websocket rate-limit preset (CRD 5620-5626).
    let gateway = Router::new()
        .route("/api/websocket/connect", get(socket::connect))
        .route("/api/websocket/disconnect", post(endpoints::disconnect))
        .layer(from_fn(rate_limit::limit(state.rate_limiter.clone(), RatePolicy::WEBSOCKET)));

    let ops = Router::new()
        .route("/api/websocket/migration-status", get(endpoints::migration_status))
        .route("/api/websocket/migration-config", post(endpoints::migration_config))
        .route("/api/websocket/health", get(endpoints::health))
        .route("/api/websocket/readiness", get(endpoints::readiness))
        .route("/api/websocket/liveness", get(endpoints::liveness))
        .route("/api/websocket/metrics", get(endpoints::metrics))
        .route("/api/websocket/health-detail", get(endpoints::health_detail))
        .route("/api/websocket/comparison", get(endpoints::comparison))
        .route("/api/websocket/dashboard/metrics", get(endpoints::dashboard_metrics))
        .route("/api/websocket/dashboard/connections", get(endpoints::dashboard_connections))
        .route("/api/websocket/dashboard/history", get(endpoints::dashboard_history))
        .route("/api/websocket/dashboard/trends", get(endpoints::dashboard_trends))
        .route(
            "/api/websocket/dashboard/durable-objects",
            get(endpoints::dashboard_durable_objects),
        )
        .route("/api/websocket/dashboard/alerts", get(endpoints::dashboard_alerts))
        .route("/api/websocket/analytics/dashboard", get(endpoints::analytics_dashboard))
        .route("/api/websocket/analytics/trends", get(endpoints::analytics_trends))
        .route("/api/websocket/analytics/errors", post(endpoints::analytics_record_error))
        .route("/api/websocket/analytics/quality", post(endpoints::analytics_record_quality))
        .route(
            "/api/websocket/analytics/alerts/trigger",
            post(endpoints::analytics_trigger_alert),
        )
        .route("/api/websocket/analytics/health", get(endpoints::analytics_health))
        .route(
            "/api/websocket/analytics/config/alerts",
            get(endpoints::analytics_alert_config)
                .put(endpoints::analytics_update_alert_config),
        )
        .route("/api/websocket/analytics/export/trends", get(endpoints::analytics_export_trends))
        .route("/api/websocket/test-connection", get(endpoints::test_connection));

    gateway.merge(ops)
}

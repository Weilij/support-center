//! Analytics (CRD §6.1, lines 4203-4503): core insights, period comparison,
//! dashboards, realtime dashboard control, and the security dashboard.

pub mod comparison;
pub mod core;
pub mod dashboard;
pub mod security;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Security-dashboard health is the one public path (CRD 4447).
    let public = Router::new().route("/api/security/dashboard/health", get(security::health));

    let authed = Router::new()
        // Core analytics.
        .route("/api/analytics/conversations", get(core::conversations))
        .route("/api/analytics/messages", get(core::messages))
        .route("/api/analytics/users", get(core::users))
        .route("/api/analytics/performance", get(core::performance))
        .route("/api/analytics/custom", post(core::custom))
        .route("/api/analytics/export", post(core::export))
        .route("/api/analytics/health", get(core::health))
        .route("/api/analytics/metrics", post(core::record_metrics))
        .route("/api/analytics/metrics/{name}", get(core::query_metric))
        // Comparison family.
        .route("/api/analytics/comparison/metric", get(comparison::single))
        .route("/api/analytics/comparison/metrics", get(comparison::multi))
        .route("/api/analytics/comparison/preset/conversation", get(comparison::preset_conversation))
        .route("/api/analytics/comparison/preset/message", get(comparison::preset_message))
        .route("/api/analytics/comparison/preset/user-activity", get(comparison::preset_user_activity))
        .route("/api/analytics/comparison/cache/stats", get(comparison::cache_stats))
        // Dashboards.
        .route("/api/analytics/dashboard/health", get(dashboard::health))
        .route("/api/analytics/dashboard/widget-types", get(dashboard::widget_types))
        .route("/api/analytics/dashboard/templates", get(dashboard::list_dashboard_templates))
        .route("/api/analytics/dashboard/widget-templates", get(dashboard::list_widget_templates))
        .route("/api/analytics/dashboard/layout/optimize", post(dashboard::optimize_layout))
        .route(
            "/api/analytics/dashboard/config",
            get(dashboard::get_config).post(dashboard::save_config).put(dashboard::save_config),
        )
        .route(
            "/api/analytics/dashboard/config/{dashboardId}",
            get(dashboard::get_config).post(dashboard::save_config).put(dashboard::save_config),
        )
        .route("/api/analytics/dashboard/data", get(dashboard::dashboard_data))
        .route("/api/analytics/dashboard/data/{dashboardId}", get(dashboard::dashboard_data))
        .route("/api/analytics/dashboard/widget", post(dashboard::create_widget))
        .route("/api/analytics/dashboard/widget/{widgetId}", put(dashboard::update_widget))
        .route("/api/analytics/dashboard/widget/{widgetId}/data", get(dashboard::widget_data))
        .route("/api/analytics/dashboard/widget/{widgetId}/clone", post(dashboard::clone_widget))
        .route(
            "/api/analytics/dashboard/templates/{templateId}/create",
            post(dashboard::create_from_template),
        )
        .route(
            "/api/analytics/dashboard/widget-templates/{templateId}/create",
            post(dashboard::create_widget_from_template),
        )
        // Realtime dashboard control.
        .route("/api/analytics/realtime/broadcast", post(dashboard::broadcast))
        .route(
            "/api/analytics/realtime/trigger-update/{dashboardId}",
            post(dashboard::trigger_dashboard),
        )
        .route(
            "/api/analytics/realtime/trigger-update/{dashboardId}/{widgetId}",
            post(dashboard::trigger_widget),
        )
        .route("/api/analytics/realtime/status", get(dashboard::realtime_status))
        .route("/api/analytics/realtime/health", get(dashboard::realtime_health))
        .route("/api/analytics/realtime/cleanup", post(dashboard::realtime_cleanup))
        // Security dashboard (admin-gated in handlers).
        .route("/api/security/dashboard/metrics", get(security::metrics))
        .route("/api/security/dashboard/events/recent", get(security::recent_events))
        .route("/api/security/dashboard/summary", get(security::summary))
        .layer(from_fn_with_state(state, require_auth));

    public.merge(authed)
}

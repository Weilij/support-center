//! System Settings & Administration (CRD §6.6, lines 5247-5487).

pub mod admin;
pub mod handlers;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let public = Router::new()
        .route("/api/system/health", get(handlers::basic_health))
        .route("/api/system/api", get(handlers::api_descriptor))
        .route("/api/health/health", get(handlers::health_health))
        .route("/api/health/status", get(handlers::health_status))
        .route("/api/health/ready", get(handlers::health_ready))
        .route("/api/health/live", get(handlers::health_live))
        .route("/api/reminders/health", get(reminders_health))
        .route("/api/data-optimization/health", get(admin::opt_health));

    let authed = Router::new()
        // /api/system
        .route("/api/system/system/status", get(handlers::system_status))
        .route("/api/system/stats", get(handlers::stats))
        .route("/api/system/messages/recall-stats", get(handlers::recall_stats))
        .route("/api/system/messages/{messageId}/replies", get(handlers::message_replies))
        .route(
            "/api/system/conversations/{conversationId}/message-tree",
            get(handlers::message_tree),
        )
        .route(
            "/api/system/conversations/{conversationId}/sessions",
            get(handlers::conversation_sessions),
        )
        .route("/api/system/info", get(handlers::info))
        .route(
            "/api/system/settings",
            get(handlers::get_settings).put(handlers::update_settings),
        )
        .route("/api/system/metrics", get(handlers::metrics))
        .route("/api/system/integrations/{platform}/test", post(handlers::test_integration))
        .route("/api/system/api-status", get(handlers::api_status))
        .route("/api/system/config-check", get(handlers::config_check))
        // /api/health (authenticated tier)
        .route("/api/health/system", get(handlers::health_system))
        .route("/api/health/infrastructure", get(handlers::health_infrastructure))
        .route("/api/health/services", get(handlers::health_services))
        .route("/api/health/stats", get(handlers::health_stats))
        .route("/api/health/component/{component}", get(handlers::health_component))
        .route("/api/health/metrics", get(handlers::health_metrics_text))
        .route("/api/health/check/all", post(handlers::health_check_all))
        // /api/feedback
        .route("/api/feedback", post(handlers::submit_feedback).get(handlers::feedback_list))
        .route("/api/feedback/stats", get(handlers::feedback_stats))
        .route(
            "/api/feedback/conversation/{conversationId}",
            get(handlers::feedback_for_conversation),
        )
        // /api/alert-config (admin-gated in handlers)
        .route("/api/alert-config/channels/slack", post(admin::config_slack))
        .route("/api/alert-config/channels/email", post(admin::config_email))
        .route("/api/alert-config/channels/webhook", post(admin::config_webhook))
        .route("/api/alert-config/channels/status", get(admin::channel_status))
        .route("/api/alert-config/logs", get(admin::config_logs))
        .route("/api/alert-config/test-alert", post(admin::test_alert))
        // /api/data-optimization (admin-gated except health above)
        .route(
            "/api/data-optimization/config",
            get(admin::opt_get_config).put(admin::opt_put_config),
        )
        .route("/api/data-optimization/stats", get(admin::opt_stats))
        .route("/api/data-optimization/test-cache", post(admin::opt_test_cache))
        .route("/api/data-optimization/cleanup", post(admin::opt_cleanup))
        .route("/api/data-optimization/test-batch", post(admin::opt_test_batch))
        .route("/api/data-optimization/indexes", post(admin::opt_create_index))
        .route(
            "/api/data-optimization/indexes/{indexName}/{field}",
            get(admin::opt_query_index),
        )
        .route("/api/data-optimization/initialize-baseline", post(admin::opt_init_baseline))
        // /api/monitoring/kv (admin-gated)
        .route("/api/monitoring/kv/activity-cache", get(admin::kv_activity_cache))
        .route("/api/monitoring/kv/request-frequency", get(admin::kv_request_frequency))
        .route("/api/monitoring/kv/savings", get(admin::kv_savings))
        .route("/api/monitoring/kv/health", get(admin::kv_health))
        .route("/api/monitoring/kv/reset", post(admin::kv_reset))
        // /api/user-experience
        .route("/api/user-experience/metrics", post(admin::ux_metrics))
        .route("/api/user-experience/behavior", post(admin::ux_behavior))
        .route("/api/user-experience/survey/invitation", get(admin::ux_survey_invitation))
        .route("/api/user-experience/survey", post(admin::ux_survey_submit))
        .route("/api/user-experience/report", get(admin::ux_report))
        .route(
            "/api/user-experience/ab-tests/{testId}/assignment",
            get(admin::ux_ab_assignment),
        )
        .route("/api/user-experience/ab-tests/{testId}/metrics", post(admin::ux_ab_metrics))
        .route("/api/user-experience/ab-tests", post(admin::ux_ab_create))
        .route("/api/user-experience/personal-dashboard", get(admin::ux_personal_dashboard))
        .route("/api/user-experience/health", get(admin::ux_health))
        // migrations (admin-gated)
        .route(
            "/api/admin/migrations/backfill-legacy-filenames",
            post(admin::backfill_legacy_filenames),
        )
        .layer(from_fn_with_state(state, require_auth));

    public.merge(authed)
}

/// Public reminders module health (CRD 5383).
async fn reminders_health() -> axum::response::Response {
    crate::envelope::ok(serde_json::json!({
        "status": "healthy", "module": "reminders", "timestamp": crate::db::now_iso(),
    }))
}

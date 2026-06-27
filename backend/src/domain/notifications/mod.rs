//! Notifications (CRD §6.4, lines 4881-5104): in-app inbox, internal
//! triggers, task reminders, and the alerting subsystem.

pub mod alerts;
pub mod handlers;
pub mod reminders;
pub mod service;

use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Health/info are unauthenticated (CRD 5000-5002).
    let public = Router::new()
        .route("/api/notifications/health", get(handlers::health))
        .route("/api/notifications/info", get(handlers::module_info));

    let inbox = Router::new()
        .route(
            "/api/notifications",
            get(handlers::list).post(handlers::create),
        )
        .route(
            "/api/notifications/",
            get(handlers::list).post(handlers::create),
        )
        .route("/api/notifications/bulk", post(handlers::bulk_create))
        .route(
            "/api/notifications/mark-all-read",
            put(handlers::mark_all_read),
        )
        .route("/api/notifications/stats", get(handlers::stats))
        .route(
            "/api/notifications/unread-count",
            get(handlers::unread_count),
        )
        .route("/api/notifications/recent", get(handlers::recent))
        .route("/api/notifications/cleanup", delete(handlers::cleanup))
        .route(
            "/api/notifications/channels/stats",
            get(handlers::channel_stats),
        )
        .route(
            "/api/notifications/channels/{channelType}/test",
            post(handlers::test_channel),
        )
        .route(
            "/api/notifications/new-message",
            post(handlers::trigger_new_message),
        )
        .route(
            "/api/notifications/conversation-assigned",
            post(handlers::trigger_assigned),
        )
        .route("/api/notifications/system", post(handlers::trigger_system))
        .route("/api/notifications/broadcast", post(handlers::broadcast))
        .route(
            "/api/notifications/{id}",
            get(handlers::get_one).delete(handlers::delete_one),
        )
        .route("/api/notifications/{id}/read", put(handlers::mark_read));

    let reminder_routes = Router::new()
        .route(
            "/api/reminders",
            post(reminders::create).get(reminders::list),
        )
        .route("/api/reminders/upcoming", get(reminders::upcoming))
        .route("/api/reminders/stats", get(reminders::stats))
        .route("/api/reminders/process", post(reminders::process))
        .route(
            "/api/reminders/{id}",
            get(reminders::get_one)
                .put(reminders::update)
                .delete(reminders::delete),
        )
        .route("/api/reminders/{id}/complete", put(reminders::complete));

    public.merge(
        inbox
            .merge(reminder_routes)
            .layer(from_fn_with_state(state, require_auth)),
    )
}

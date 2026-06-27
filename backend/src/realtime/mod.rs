//! Real-time infrastructure: WebSocket gateway & protocol (CRD §5.1, lines
//! 3221-3467) plus the §1.3 real-time connection gate (lines 596-646).
//!
//! The WS upgrade paths are PUBLIC routes (no bearer middleware): browsers
//! cannot send custom headers on upgrade, so the credential travels as a
//! `token` query parameter and is verified during the handshake (CRD 597).
//! Per-route authentication for the HTTP surface happens in-handler.

pub mod broadcaster;
pub mod collaboration;
pub mod customer;
pub mod endpoints;
pub mod gate;
pub mod hub;
pub mod latest;
pub mod module;
pub mod rooms;
pub mod socket;
pub mod user_sessions;

pub use hub::RealtimeHub;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::middleware::rate_limit::{self, RatePolicy};
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Connect/disconnect carry the websocket rate-limit preset (CRD 5620-5626).
    let gateway = Router::new()
        .route("/api/websocket/connect", get(socket::connect))
        .route("/api/websocket/disconnect", post(endpoints::disconnect))
        .layer(from_fn(rate_limit::limit(
            state.clone(),
            RatePolicy::WEBSOCKET,
        )));

    let ops = Router::new()
        .route(
            "/api/websocket/migration-status",
            get(endpoints::migration_status),
        )
        .route(
            "/api/websocket/migration-config",
            post(endpoints::migration_config),
        )
        .route("/api/websocket/health", get(endpoints::health))
        .route("/api/websocket/readiness", get(endpoints::readiness))
        .route("/api/websocket/liveness", get(endpoints::liveness))
        .route("/api/websocket/metrics", get(endpoints::metrics))
        .route(
            "/api/websocket/health-detail",
            get(endpoints::health_detail),
        )
        .route("/api/websocket/comparison", get(endpoints::comparison))
        .route(
            "/api/websocket/dashboard/metrics",
            get(endpoints::dashboard_metrics),
        )
        .route(
            "/api/websocket/dashboard/connections",
            get(endpoints::dashboard_connections),
        )
        .route(
            "/api/websocket/dashboard/history",
            get(endpoints::dashboard_history),
        )
        .route(
            "/api/websocket/dashboard/trends",
            get(endpoints::dashboard_trends),
        )
        .route(
            "/api/websocket/dashboard/durable-objects",
            get(endpoints::dashboard_durable_objects),
        )
        .route(
            "/api/websocket/dashboard/alerts",
            get(endpoints::dashboard_alerts),
        )
        .route(
            "/api/websocket/analytics/dashboard",
            get(endpoints::analytics_dashboard),
        )
        .route(
            "/api/websocket/analytics/trends",
            get(endpoints::analytics_trends),
        )
        .route(
            "/api/websocket/analytics/errors",
            post(endpoints::analytics_record_error),
        )
        .route(
            "/api/websocket/analytics/quality",
            post(endpoints::analytics_record_quality),
        )
        .route(
            "/api/websocket/analytics/alerts/trigger",
            post(endpoints::analytics_trigger_alert),
        )
        .route(
            "/api/websocket/analytics/health",
            get(endpoints::analytics_health),
        )
        .route(
            "/api/websocket/analytics/config/alerts",
            get(endpoints::analytics_alert_config).put(endpoints::analytics_update_alert_config),
        )
        .route(
            "/api/websocket/analytics/export/trends",
            get(endpoints::analytics_export_trends),
        )
        .route(
            "/api/websocket/test-connection",
            get(endpoints::test_connection),
        );

    // Conversation room surface (CRD §5.2 lines 3469-3577), mounted per room.
    let rooms = Router::new()
        .route(
            "/api/realtime/rooms/{conversation_id}/websocket",
            get(rooms::room_connect),
        )
        .route(
            "/api/realtime/rooms/{conversation_id}/challenge",
            post(rooms::challenge),
        )
        .route(
            "/api/realtime/rooms/{conversation_id}/connect",
            post(rooms::connect_status),
        )
        .route(
            "/api/realtime/rooms/{conversation_id}/disconnect",
            post(rooms::force_disconnect),
        )
        .route(
            "/api/realtime/rooms/{conversation_id}/broadcast",
            post(rooms::broadcast),
        )
        .route(
            "/api/realtime/rooms/{conversation_id}/participants",
            post(rooms::participants),
        )
        .route(
            "/api/realtime/rooms/{conversation_id}/metrics",
            post(rooms::room_metrics),
        );

    // Routed event delivery (CRD §5.2 lines 3581-3660).
    let delivery = Router::new()
        .route(
            "/api/realtime/broadcaster/broadcast",
            post(broadcaster::queue_event),
        )
        .route(
            "/api/realtime/broadcaster/queue-event",
            post(broadcaster::queue_event),
        )
        .route(
            "/api/realtime/broadcaster/broadcast-to-conversations",
            post(broadcaster::to_conversations),
        )
        .route(
            "/api/realtime/broadcaster/broadcast-to-users",
            post(broadcaster::to_users),
        )
        .route(
            "/api/realtime/broadcaster/broadcast-to-teams",
            post(broadcaster::to_teams),
        )
        .route(
            "/api/realtime/broadcaster/broadcast-to-teams-and-admins",
            post(broadcaster::to_teams_and_admins),
        )
        .route(
            "/api/realtime/broadcaster/broadcast-global",
            post(broadcaster::global),
        )
        .route(
            "/api/realtime/broadcaster/batch-broadcast",
            post(broadcaster::batch),
        )
        .route(
            "/api/realtime/broadcaster/register-connection",
            post(broadcaster::register_connection),
        )
        .route(
            "/api/realtime/broadcaster/unregister-connection",
            post(broadcaster::unregister_connection),
        )
        .route(
            "/api/realtime/broadcaster/update-filters",
            post(broadcaster::update_filters),
        )
        .route(
            "/api/realtime/broadcaster/flush-queue",
            post(broadcaster::flush_queue),
        )
        .route(
            "/api/realtime/broadcaster/system-broadcast",
            post(broadcaster::system_broadcast),
        )
        .route(
            "/api/realtime/broadcaster/metrics",
            post(broadcaster::metrics),
        )
        .route(
            "/api/realtime/broadcaster/status",
            post(broadcaster::status),
        )
        .route(
            "/api/realtime/broadcaster/health",
            post(broadcaster::status),
        )
        .route(
            "/api/realtime/broadcaster/debug-connections",
            post(broadcaster::debug_connections),
        );

    // User real-time sessions (CRD §5.3 lines 3694-3845), scoped to the
    // authenticated user.
    let sessions = Router::new()
        .route(
            "/api/realtime/session/websocket",
            get(user_sessions::session_connect),
        )
        .route(
            "/api/realtime/session/connect",
            post(user_sessions::subscribe),
        )
        .route(
            "/api/realtime/session/subscribe",
            post(user_sessions::subscribe),
        )
        .route(
            "/api/realtime/session/disconnect",
            post(user_sessions::unsubscribe),
        )
        .route(
            "/api/realtime/session/unsubscribe",
            post(user_sessions::unsubscribe),
        )
        .route(
            "/api/realtime/session/presence",
            post(user_sessions::presence),
        )
        .route(
            "/api/realtime/session/preferences",
            get(user_sessions::get_preferences)
                .put(user_sessions::put_preferences)
                // Any other method on this path -> 405 (CRD 3760).
                .fallback(user_sessions::method_not_allowed),
        )
        .route("/api/realtime/session/status", get(user_sessions::status))
        .route("/api/realtime/session/metrics", get(user_sessions::metrics))
        .route(
            "/api/realtime/session/broadcast",
            post(user_sessions::broadcast),
        )
        .route(
            "/api/realtime/session/batch-events",
            post(user_sessions::batch_events),
        );

    // Customer-side per-conversation channels (CRD §5.4 lines 3847-3974).
    // No bearer middleware: the WS path authenticates in-handshake and the
    // message API authenticates via its own headers; any unknown path under
    // the surface answers 404 plain text (CRD 3944).
    let customer = Router::new().nest(
        "/api/customer-channel",
        Router::new()
            .route("/ws", get(customer::channel_ws))
            .route("/notify-message", post(customer::notify_message))
            .route(
                "/notify-message-updated",
                post(customer::notify_message_updated),
            )
            .route(
                "/messages",
                get(customer::list_messages).post(customer::create_message),
            )
            .route(
                "/upload",
                post(customer::upload)
                    .layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024)),
            )
            .fallback(customer::not_found_plain),
    );

    // Realtime module management/monitoring surface (CRD §5.5 lines
    // 3974-4197): every route requires a valid bearer token (CRD 3981);
    // finer role checks happen per handler.
    let module_routes = Router::new()
        .route("/api/realtime/typing", post(module::typing))
        .route("/api/realtime/broadcast", post(module::broadcast))
        .route(
            "/api/realtime/conversation/{id}/status",
            get(module::conversation_status),
        )
        .route("/api/realtime/online-status", post(module::online_status))
        .route(
            "/api/realtime/config",
            get(module::get_config).put(module::put_config),
        )
        .route("/api/realtime/stats", get(module::stats))
        .route("/api/realtime/health", get(module::health))
        .route(
            "/api/realtime/monitoring/dashboard",
            get(module::monitoring_dashboard),
        )
        .route(
            "/api/realtime/monitoring/metrics",
            get(module::monitoring_metrics),
        )
        .route(
            "/api/realtime/monitoring/alerts",
            get(module::monitoring_alerts).post(module::resolve_alert),
        )
        .route(
            "/api/realtime/monitoring/health",
            get(module::monitoring_health),
        )
        .route(
            "/api/realtime/monitoring/config",
            get(module::monitoring_config).post(module::monitoring_config),
        )
        .layer(from_fn_with_state(state.clone(), require_auth));

    // Collaboration surface (CRD §3.4 lines 2321-2446), authenticated.
    let collab = Router::new()
        .route(
            "/api/collaboration/conversations/{conversation_id}/state",
            get(collaboration::conversation_state),
        )
        .route(
            "/api/collaboration/conversations/{conversation_id}/viewers",
            get(collaboration::viewers),
        )
        .route(
            "/api/collaboration/conversations/{conversation_id}/join",
            post(collaboration::join),
        )
        .route(
            "/api/collaboration/conversations/{conversation_id}/leave",
            post(collaboration::leave),
        )
        .route("/api/collaboration/typing", post(collaboration::typing))
        .route("/api/collaboration/presence", post(collaboration::presence))
        .route("/api/collaboration/stats", get(collaboration::stats))
        .route("/api/collaboration/cleanup", post(collaboration::cleanup))
        .route("/api/collaboration/health", get(collaboration::health))
        .layer(from_fn_with_state(state.clone(), require_auth));

    // Background queue-processing loops (CRD 3692): fast loop for high/urgent
    // events, slower loop for normal/low events.
    broadcaster::spawn_loops(state.clone());
    broadcaster::spawn_remote_fanout_loop(state.clone());
    customer::spawn_remote_fanout_loop(state.clone());

    gateway
        .merge(ops)
        .merge(rooms)
        .merge(delivery)
        .merge(sessions)
        .merge(customer)
        .merge(module_routes)
        .merge(collab)
}

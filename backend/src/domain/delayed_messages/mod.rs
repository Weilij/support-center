//! Delayed / Scheduled Messages HTTP surface (CRD §2.4, lines 1171-1332).
//!
//! Two route families over the shared scheduling service
//! (`crate::domain::messaging::service`): the real-time buffer family at
//! `/api/delayed-messages-v2` (primary) and the legacy family at
//! `/api/delayed-messages` (recall/cancellation markers).

pub mod handlers;

use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Health is public (CRD 1237).
    let public = Router::new().route("/api/delayed-messages-v2/health", get(handlers::health));

    let authed = Router::new()
        .route("/api/delayed-messages-v2/send", post(handlers::v2_send))
        .route("/api/delayed-messages-v2/cancel/{messageId}", delete(handlers::v2_cancel))
        .route("/api/delayed-messages-v2/status/{messageId}", get(handlers::v2_status))
        .route("/api/delayed-messages-v2/pending", get(handlers::v2_pending))
        .route("/api/delayed-messages-v2/failed", get(handlers::v2_failed))
        .route("/api/delayed-messages-v2/metrics", get(handlers::v2_metrics))
        .route("/api/delayed-messages/send", post(handlers::legacy_send))
        .route("/api/delayed-messages/recall/{messageId}", post(handlers::legacy_recall))
        .route("/api/delayed-messages/pending", get(handlers::legacy_pending))
        .route("/api/delayed-messages/reschedule/{messageId}", post(handlers::legacy_reschedule))
        .layer(from_fn_with_state(state, require_auth));

    public.merge(authed)
}

//! Activity Log & Reversible Actions (CRD §3.5, lines 2448-2612).
//!
//! System-wide audit trail: listing/filtering, single-entry lookup, per-actor and
//! admin statistics families, age-based cleanup, and the undo ("restore") of
//! reversible actions with conflict detection and an exactly-once guarded claim.

pub mod handlers;
pub mod restore;
pub mod store;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let authed = Router::new()
        .route("/api/activities", get(handlers::list_activities))
        .route("/api/activities/", get(handlers::list_activities))
        .route("/api/activities/overview", get(handlers::overview))
        .route("/api/activities/trends", get(handlers::trends))
        .route("/api/activities/heatmap", get(handlers::heatmap))
        .route("/api/activities/metrics", get(handlers::metrics))
        .route("/api/activities/cleanup", post(handlers::cleanup))
        .route(
            "/api/activities/stats/resources",
            get(handlers::resource_stats),
        )
        .route("/api/activities/stats/roles", get(handlers::role_stats))
        .route("/api/activities/stats/custom", get(handlers::custom_stats))
        .route(
            "/api/activities/user/{userId}/stats",
            get(handlers::user_stats),
        )
        .route("/api/activities/{id}", get(handlers::get_activity))
        .layer(from_fn_with_state(state.clone(), require_auth));

    // The restore operation authenticates inside the handler: the CRD's observable
    // ordering loads and validates the entry (404 / NOT_REVERSIBLE / ALREADY_RESTORED)
    // before the caller is authenticated (CRD 2553-2556).
    let open = Router::new().route(
        "/api/activities/{id}/restore",
        post(restore::restore_activity),
    );

    authed.merge(open)
}

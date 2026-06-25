//! Reports (CRD §6.2, lines 4505-4695).

pub mod handlers;
pub mod scheduler;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::middleware::rate_limit::{limit, RatePolicy};
use crate::state::AppState;

/// Mutating/expensive report endpoints share a ~30/min window (CRD 4690).
const REPORTS_POLICY: RatePolicy = RatePolicy::reports();

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let public = Router::new()
        .route("/api/reports/health", get(handlers::health))
        .route("/api/reports/info", get(handlers::info));

    let authed = Router::new()
        .route("/api/reports", post(handlers::generate).get(handlers::list))
        .route("/api/reports/stats", get(handlers::stats))
        .route("/api/reports/batch", post(handlers::batch))
        .route("/api/reports/preview", post(handlers::preview))
        .route("/api/reports/templates/{type}", get(handlers::templates))
        .route(
            "/api/reports/scheduled",
            post(handlers::create_scheduled).get(handlers::list_scheduled),
        )
        .route(
            "/api/reports/scheduled/{id}",
            put(handlers::update_scheduled).delete(handlers::delete_scheduled),
        )
        .route("/api/reports/{id}", get(handlers::detail).delete(handlers::delete_report))
        .route("/api/reports/{id}/download", get(handlers::download))
        .layer(from_fn(limit(state.clone(), REPORTS_POLICY)))
        .layer(from_fn_with_state(state, require_auth));

    public.merge(authed)
}

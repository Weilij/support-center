//! LIFF (LINE Front-end Framework) Integration (CRD §4.3, lines 2862-2994).
//!
//! Public mini-page endpoints (`/api/liff/*`, `/join`) plus admin bulk code
//! generation/coverage. The per-team front-end code endpoints
//! (`/api/teams/{id}/qr-code/liff*`) live in `crate::domain::teams`.

pub mod handlers;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_admin;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // The mini-page calls these before any authentication exists (CRD 2870,
    // 2878, 2886, 2894, 2904); /join is a public browser navigation.
    let public = Router::new()
        .route("/api/liff/health", get(handlers::health))
        .route("/api/liff/config", get(handlers::config))
        .route("/api/liff/teams/{teamId}", get(handlers::team_info))
        .route("/api/liff/assign-team", post(handlers::assign_team))
        .route("/api/liff/welcome", post(handlers::welcome))
        .route("/join", get(handlers::join_page));

    let admin = Router::new()
        .route(
            "/api/admin/liff-qr/batch-generate",
            post(handlers::batch_generate),
        )
        .route("/api/admin/liff-qr/status", get(handlers::coverage_status))
        .layer(from_fn_with_state(state, require_admin));

    public.merge(admin)
}

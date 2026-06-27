//! Authentication & account management (CRD §1.1, lines 126-293).

pub mod handlers;
pub mod store;
pub mod tokens;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::middleware::rate_limit::{limit, trusted_client_ip_layer, RatePolicy};
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Strict per-IP limit on the credential-issuing endpoints (CRD 138, 168: ~5 per 5 min).
    let login_routes = Router::new()
        .route("/api/auth/login", post(handlers::login))
        .route("/api/auth/refresh", post(handlers::refresh))
        .layer(from_fn(limit(state.clone(), RatePolicy::LOGIN)));

    let public_routes = Router::new()
        .route("/phase2-auth/verify-token", post(handlers::verify_token))
        .route("/api/auth/logout", post(handlers::logout));

    let authed_routes = Router::new()
        .route("/api/auth/register", post(handlers::register))
        .route("/api/auth/profile", get(handlers::profile))
        .route("/api/auth/me", get(handlers::me).put(handlers::update_me))
        .route("/api/auth/change-password", post(handlers::change_password))
        .route(
            "/api/teams/members/{memberId}/reset",
            post(handlers::reset_member_password),
        )
        .route(
            "/phase2-auth/monitoring-token",
            post(handlers::monitoring_token),
        )
        .route("/phase2-auth/user-token", post(handlers::user_token))
        .route(
            "/phase2-auth/refresh-token",
            post(handlers::refresh_service_token),
        )
        .route("/phase2-auth/batch-tokens", post(handlers::batch_tokens))
        .route("/phase2-auth/status", get(handlers::auth_status))
        .layer(from_fn_with_state(state.clone(), require_auth));

    login_routes
        .merge(public_routes)
        .merge(authed_routes)
        .layer(from_fn_with_state(state, trusted_client_ip_layer))
}

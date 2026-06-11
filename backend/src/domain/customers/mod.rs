//! Customers directory & customer-label associations (CRD §3.1, lines 1644-1792,
//! plus the customer-label family of §2.6, lines 1551-1592).

pub mod handlers;
pub mod store;

use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/customers", get(handlers::list_customers))
        // The spec mounts the listing at the trailing-slash path (CRD 1655).
        .route("/api/customers/", get(handlers::list_customers))
        .route("/api/customers/tags/available", get(handlers::available_tags))
        .route(
            "/api/customers/platform/{platform}/{platformUserId}",
            get(handlers::get_customer_by_platform),
        )
        .route("/api/customers/{customerId}", get(handlers::get_customer))
        .route(
            "/api/customers/{customerId}/tags",
            get(handlers::get_customer_tags)
                .post(handlers::add_customer_tags)
                .delete(handlers::remove_customer_tags)
                .put(handlers::replace_customer_tags),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

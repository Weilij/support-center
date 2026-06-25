//! Inbound Webhook Ingestion & Platform Parsing (CRD §4.2, lines 2728-2862).
//!
//! Public ingress for messaging-platform events: these routes carry NO
//! session/JWT authentication — trust is established by platform request
//! signatures only — and they are mounted among the public/priority routes in
//! `crate::app` so no authenticated catch-all can shadow them (CRD route
//! precedence, §7.1).

pub mod handlers;
pub mod ingest;
pub mod parse;
pub mod urls;

use axum::routing::{any, get};
use axum::Router;
use std::sync::Arc;

use crate::state::AppState;

pub fn routes(_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        // LINE-style platform: POST delivery + GET readiness probe (CRD 2730,
        // 2774).
        .route(
            "/api/webhook",
            get(handlers::line_probe).post(handlers::line_webhook),
        )
        // Facebook/Instagram-style platform: subscription handshake (GET) and
        // event delivery (POST) on the same path (CRD 2780).
        .route("/api/webhooks/facebook", any(handlers::facebook_webhook))
        // Shopee Webchat push events. Public, signature-gated by partner key.
        .route(
            "/api/webhooks/shopee",
            axum::routing::post(handlers::shopee_webhook),
        )
}

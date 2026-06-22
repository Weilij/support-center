//! Shopee Open Platform v2 integration foundation (Track B4a): request signing,
//! OAuth token lifecycle, and per-shop encrypted token storage. The gated
//! SellerChat inbound/outbound land in B4b/B4c on top of this.
pub mod client;
pub mod sign;
pub mod store;

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Json;
use axum::Router;
use serde_json::json;

use crate::state::AppState;

/// GET /api/shopee/auth/callback?code=&shop_id= — exchange the OAuth code for
/// tokens and persist them per-shop. The only B4a HTTP surface.
async fn auth_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let code = params.get("code").map(String::as_str).unwrap_or_default();
    let shop_id = params.get("shop_id").and_then(|s| s.parse::<i64>().ok());
    if code.is_empty() || shop_id.is_none() {
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"success": false, "error": "code and shop_id are required"}))).into_response();
    }
    let shop_id = shop_id.unwrap();
    let Some(client) = client::ShopeeClient::from_config(&state.config) else {
        return (axum::http::StatusCode::NOT_IMPLEMENTED, Json(json!({"success": false, "error": "Shopee is not configured"}))).into_response();
    };
    match client.fetch_token(code, shop_id).await {
        Ok(t) => {
            let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(t.expire_in)).to_rfc3339();
            match store::save_tokens(&state.db, state.config.encryption_key.as_deref(), shop_id, &t.access_token, &t.refresh_token, &expires_at).await {
                Ok(()) => Json(json!({"success": true, "shopId": shop_id})).into_response(),
                Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"success": false, "error": e}))).into_response(),
            }
        }
        Err(e) => (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"success": false, "error": e}))).into_response(),
    }
}

pub fn routes(_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new().route("/api/shopee/auth/callback", get(auth_callback))
}

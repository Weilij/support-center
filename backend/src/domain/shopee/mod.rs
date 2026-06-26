//! Shopee Open Platform v2 integration foundation (Track B4a): request signing,
//! OAuth token lifecycle, and per-shop encrypted token storage. The gated
//! SellerChat inbound/outbound land in B4b/B4c on top of this.
pub mod client;
pub mod sign;
pub mod store;

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::middleware::from_fn_with_state;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Extension;
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::middleware::auth::{require_auth, AuthUser};
use crate::state::AppState;

fn require_admin(user: &AuthUser) -> Option<axum::response::Response> {
    if user.is_admin() {
        None
    } else {
        Some((
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"success": false, "error": "Administrator role required"})),
        )
            .into_response())
    }
}

fn backend_base(state: &AppState) -> String {
    state
        .config
        .backend_url
        .as_deref()
        .map(|b| b.trim_end_matches('/').to_string())
        .unwrap_or_else(|| format!("http://localhost:{}", state.config.port))
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    #[serde(rename = "shopId", alias = "shop_id")]
    shop_id: i64,
    #[serde(rename = "teamId", alias = "team_id")]
    team_id: Option<i64>,
}

/// GET /api/shopee/auth/authorize?shopId= — create an admin-bound OAuth state
/// and return the signed Shopee authorization URL.
async fn auth_authorize(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<AuthorizeQuery>,
) -> axum::response::Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }
    if q.shop_id <= 0 {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "shopId must be a positive integer"})),
        )
            .into_response();
    }
    let Some(client) = client::ShopeeClient::from_config(&state.config) else {
        return (
            axum::http::StatusCode::NOT_IMPLEMENTED,
            Json(json!({"success": false, "error": "Shopee is not configured"})),
        )
            .into_response();
    };

    let oauth_state = uuid::Uuid::new_v4().simple().to_string();
    state.shopee_oauth.put(&oauth_state, &user.id, q.shop_id, q.team_id);
    let redirect_url = reqwest::Url::parse_with_params(
        &format!("{}/api/shopee/auth/callback", backend_base(&state)),
        &[("state", oauth_state.clone())],
    )
    .expect("callback URL")
    .to_string();
    let authorization_url = client.authorization_url(&redirect_url, chrono::Utc::now().timestamp());
    Json(json!({
        "success": true,
        "authorizationUrl": authorization_url,
        "state": oauth_state,
        "shopId": q.shop_id,
        "teamId": q.team_id,
    }))
    .into_response()
}

/// GET /api/shopee/auth/callback?state=&code=&shop_id= — exchange the OAuth code
/// for tokens and persist them per-shop, only after consuming an admin-bound
/// one-time state created by `auth_authorize`.
async fn auth_callback(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }
    let oauth_state = params.get("state").map(String::as_str).unwrap_or_default();
    let code = params.get("code").map(String::as_str).unwrap_or_default();
    let shop_id = params.get("shop_id").and_then(|s| s.parse::<i64>().ok());
    if oauth_state.is_empty() || code.is_empty() || shop_id.is_none() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "state, code and shop_id are required"})),
        )
            .into_response();
    }
    let shop_id = shop_id.unwrap();
    let Some(expected) = state.shopee_oauth.take(oauth_state) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "Invalid or expired OAuth state"})),
        )
            .into_response();
    };
    if expected.user_id != user.id || expected.shop_id != shop_id {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"success": false, "error": "OAuth state does not match this admin session or shop"})),
        )
            .into_response();
    }
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

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/shopee/auth/authorize", get(auth_authorize))
        .route("/api/shopee/auth/callback", get(auth_callback))
        .layer(from_fn_with_state(state, require_auth))
}

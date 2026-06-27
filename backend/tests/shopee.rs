mod common;

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use common::{spawn_app, spawn_app_custom};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

fn encryption_key(byte: u8) -> String {
    base64::engine::general_purpose::STANDARD.encode([byte; 32])
}

async fn mock_shopee_token_server() -> (String, Arc<Mutex<Vec<Value>>>) {
    async fn token_handler(
        axum::extract::State(calls): axum::extract::State<Arc<Mutex<Vec<Value>>>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        calls.lock().unwrap().push(body);
        Json(json!({
            "access_token": "oauth-access-token",
            "refresh_token": "oauth-refresh-token",
            "expire_in": 3600,
        }))
    }

    let calls = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/api/v2/auth/token/get", post(token_handler))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), calls)
}

#[tokio::test]
async fn auth_callback_requires_code_and_shop_id() {
    let app = spawn_app().await;
    app.seed_agent("admin@shopee.test", "Password1!", "admin")
        .await;
    let (token, _, _) = app.login("admin@shopee.test", "Password1!").await;
    let (status, _, _) = app
        .request("GET", "/api/shopee/auth/callback", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request(
            "GET",
            "/api/shopee/auth/callback?code=abc",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn store_encrypts_and_reveals_shop_tokens() {
    let key = encryption_key(7);
    let app = spawn_app_custom(|c| c.encryption_key = Some(key.clone())).await;
    let expires_at = "2030-01-01T00:00:00Z";

    mcss_backend::domain::shopee::store::save_tokens(
        &app.state.db,
        app.state.config.encryption_key.as_deref(),
        42,
        "access-plain",
        "refresh-plain",
        expires_at,
    )
    .await
    .unwrap();

    let raw: (String, String, String) = sqlx::query_as(
        "SELECT access_token, refresh_token, expires_at FROM shopee_shops WHERE shop_id = 42",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_ne!(raw.0, "access-plain");
    assert_ne!(raw.1, "refresh-plain");
    assert!(raw.0.starts_with("enc:v1:"));
    assert!(raw.1.starts_with("enc:v1:"));
    assert_eq!(raw.2, expires_at);

    let loaded = mcss_backend::domain::shopee::store::load(
        &app.state.db,
        app.state.config.encryption_key.as_deref(),
        42,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(loaded.access_token, "access-plain");
    assert_eq!(loaded.refresh_token, "refresh-plain");
    assert_eq!(loaded.expires_at, expires_at);
}

#[tokio::test]
async fn store_rejects_tokens_encrypted_with_different_key() {
    let key = encryption_key(11);
    let other = encryption_key(12);
    let app = spawn_app_custom(|c| c.encryption_key = Some(key.clone())).await;

    mcss_backend::domain::shopee::store::save_tokens(
        &app.state.db,
        app.state.config.encryption_key.as_deref(),
        43,
        "access-secret",
        "refresh-secret",
        "2030-01-01T00:00:00Z",
    )
    .await
    .unwrap();

    let err = mcss_backend::domain::shopee::store::load(&app.state.db, Some(&other), 43)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Credential decryption failed"),
        "{err}"
    );
}

#[tokio::test]
async fn oauth_callback_exchanges_code_and_persists_encrypted_tokens() {
    let (host, calls) = mock_shopee_token_server().await;
    let key = encryption_key(19);
    let app = spawn_app_custom(|c| {
        c.encryption_key = Some(key.clone());
        c.shopee_partner_id = Some(123);
        c.shopee_partner_key = Some("partner-secret".into());
        c.shopee_host = Some(host);
        c.backend_url = Some("https://support.example".into());
    })
    .await;
    app.seed_agent("admin@shopee.test", "Password1!", "admin")
        .await;
    let (token, _, _) = app.login("admin@shopee.test", "Password1!").await;

    let (status, body, _) = app
        .request(
            "GET",
            "/api/shopee/auth/authorize?shopId=42&teamId=7",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let state = body["state"].as_str().unwrap();
    assert_eq!(body["shopId"], 42);
    assert_eq!(body["teamId"], 7);
    assert!(body["authorizationUrl"]
        .as_str()
        .unwrap()
        .contains("redirect=https%3A%2F%2Fsupport.example%2Fapi%2Fshopee%2Fauth%2Fcallback"));

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/shopee/auth/callback?state={state}&code=oauth-code&shop_id=42"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);
    assert_eq!(body["shopId"], 42);

    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["code"], "oauth-code");
        assert_eq!(calls[0]["shop_id"], 42);
        assert_eq!(calls[0]["partner_id"], 123);
    }

    let raw: (String, String) =
        sqlx::query_as("SELECT access_token, refresh_token FROM shopee_shops WHERE shop_id = 42")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_ne!(raw.0, "oauth-access-token");
    assert_ne!(raw.1, "oauth-refresh-token");
    assert!(raw.0.starts_with("enc:v1:"));
    assert!(raw.1.starts_with("enc:v1:"));

    let loaded = mcss_backend::domain::shopee::store::load(
        &app.state.db,
        app.state.config.encryption_key.as_deref(),
        42,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(loaded.access_token, "oauth-access-token");
    assert_eq!(loaded.refresh_token, "oauth-refresh-token");
}

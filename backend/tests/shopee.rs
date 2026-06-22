mod common;

use axum::http::StatusCode;
use common::spawn_app;

#[tokio::test]
async fn auth_callback_requires_code_and_shop_id() {
    let app = spawn_app().await;
    let (status, _, _) = app.request("GET", "/api/shopee/auth/callback", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("GET", "/api/shopee/auth/callback?code=abc", None, None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

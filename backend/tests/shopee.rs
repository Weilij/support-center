mod common;

use axum::http::StatusCode;
use common::spawn_app;

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

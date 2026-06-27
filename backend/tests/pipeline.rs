//! Cross-cutting pipeline behavior per CRD §7.1.

mod common;

use axum::http::StatusCode;
use common::spawn_app;

#[tokio::test]
async fn root_probe_returns_greeting() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["message"].is_string());
    assert!(body["timestamp"].is_string());
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn unknown_route_returns_spec_fallback_shape() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/nope/missing", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Not Found");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("/api/nope/missing"));
    assert!(body["timestamp"].is_string());
    // Spec note: the fallback envelope has NO `success` flag (CRD 5640).
    assert!(body.get("success").is_none());
}

#[tokio::test]
async fn preflight_from_allowed_origin_returns_204_with_cors_headers() {
    let app = spawn_app().await;
    let (status, _, headers) = app
        .request_with_headers(
            "OPTIONS",
            "/api/auth/login",
            None,
            None,
            &[("Origin", "http://localhost:5173")],
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        headers.get("access-control-allow-origin").unwrap(),
        "http://localhost:5173"
    );
    assert!(headers
        .get("access-control-allow-methods")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("PATCH"));
    assert_eq!(
        headers.get("access-control-allow-credentials").unwrap(),
        "true"
    );
    assert_eq!(headers.get("access-control-max-age").unwrap(), "86400");
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
}

#[tokio::test]
async fn preflight_from_disallowed_origin_returns_structured_403() {
    let app = spawn_app().await;
    let (status, body, headers) = app
        .request_with_headers(
            "OPTIONS",
            "/api/auth/login",
            None,
            None,
            &[("Origin", "https://evil.example.com")],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "CORS_ORIGIN_NOT_ALLOWED");
    assert_eq!(body["origin"], "https://evil.example.com");
    assert!(body["allowedOrigins"].is_array());
    assert_eq!(body["isConfigurationIssue"], false);
    assert!(body["remediation"]["steps"].is_array());
    assert_eq!(
        headers.get("x-cors-error-code").unwrap(),
        "CORS_ORIGIN_NOT_ALLOWED"
    );
}

#[tokio::test]
async fn allowed_origin_responses_get_cors_decoration() {
    let app = spawn_app().await;
    let (_, _, headers) = app
        .request_with_headers(
            "GET",
            "/",
            None,
            None,
            &[("Origin", "http://localhost:5173")],
        )
        .await;
    assert_eq!(
        headers.get("access-control-allow-origin").unwrap(),
        "http://localhost:5173"
    );
    assert_eq!(
        headers.get("access-control-allow-credentials").unwrap(),
        "true"
    );
}

#[tokio::test]
async fn disallowed_origin_still_executes_but_without_cors_headers() {
    let app = spawn_app().await;
    let (status, _, headers) = app
        .request_with_headers(
            "GET",
            "/",
            None,
            None,
            &[("Origin", "https://evil.example.com")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn security_headers_are_attached_to_every_response() {
    let app = spawn_app().await;
    let (_, _, headers) = app.request("GET", "/", None, None).await;
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
    assert!(headers.get("referrer-policy").is_some());
    assert!(headers.get("permissions-policy").is_some());
    assert!(headers.get("content-security-policy").is_some());
    // No HSTS over insecure transport.
    assert!(headers.get("strict-transport-security").is_none());
}

#[tokio::test]
async fn login_rate_limit_blocks_after_five_attempts() {
    let app = spawn_app().await;
    for _ in 0..5 {
        let (status, _, headers) = app
            .request(
                "POST",
                "/api/auth/login",
                None,
                Some(serde_json::json!({"email": "x@y.z", "password": "nope"})),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(headers.get("x-ratelimit-limit").is_some());
        assert!(headers.get("x-ratelimit-remaining").is_some());
    }
    let (status, body, headers) = app
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(serde_json::json!({"email": "x@y.z", "password": "nope"})),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["success"], false);
    assert_eq!(body["code"], "TOO_MANY_REQUESTS");
    assert!(headers.get("retry-after").is_some());
}

#[tokio::test]
async fn pagination_clamping_follows_spec_bounds() {
    use mcss_backend::envelope::clamp_page;
    assert_eq!(clamp_page(None, None), (1, 20));
    assert_eq!(clamp_page(Some(0), Some(0)), (1, 1));
    assert_eq!(clamp_page(Some(-5), Some(1000)), (1, 100));
    assert_eq!(clamp_page(Some(5000), Some(50)), (1000, 50));
}

#[tokio::test]
async fn metric_path_normalization_collapses_dynamic_segments() {
    use mcss_backend::middleware::metrics::normalize_path;
    assert_eq!(
        normalize_path("/api/conversations/123/messages"),
        "/api/conversations/:id/messages"
    );
    assert_eq!(
        normalize_path("/api/users/550e8400-e29b-41d4-a716-446655440000"),
        "/api/users/:id"
    );
    assert_eq!(
        normalize_path("/api/files/deadbeefdeadbeef01"),
        "/api/files/:id"
    );
    assert_eq!(normalize_path("/api/auth/login"), "/api/auth/login");
}

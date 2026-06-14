//! CSRF double-submit-cookie protection (security review §3).
//!
//! Rules (per spec):
//! - GET / HEAD / OPTIONS → pass through (safe methods).
//! - `/api/auth/login`    → pass through (no cookie yet on first login).
//! - No `mcss_csrf` cookie present → pass through (Bearer / webhook clients).
//! - `mcss_csrf` cookie present + mutating method → require header
//!   `X-CSRF-Token` == cookie value; otherwise 403 JSON.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::cookies::cookie_value;

pub async fn csrf_layer(req: Request<Body>, next: Next) -> Response {
    // Safe methods are never subject to CSRF.
    if matches!(
        req.method(),
        &Method::GET | &Method::HEAD | &Method::OPTIONS
    ) {
        return next.run(req).await;
    }

    // Login endpoint has no session cookie yet — let it through.
    if req.uri().path() == "/api/auth/login" {
        return next.run(req).await;
    }

    let headers = req.headers();
    let csrf_cookie = cookie_value(headers, "mcss_csrf");

    // No csrf cookie → Bearer / webhook / API client; skip enforcement.
    let Some(expected) = csrf_cookie else {
        return next.run(req).await;
    };

    // Cookie-based session: require matching X-CSRF-Token header.
    let presented = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if presented != expected {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "success": false,
                "error": "CSRF token mismatch",
                "code": "CSRF_TOKEN_MISMATCH",
            })),
        )
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware::from_fn;
    use axum::routing::{get, post};
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn ok_handler() -> StatusCode {
        StatusCode::OK
    }

    fn app() -> Router {
        Router::new()
            .route("/api/something", get(ok_handler).post(ok_handler))
            .route("/api/auth/login", post(ok_handler))
            .layer(from_fn(csrf_layer))
    }

    #[tokio::test]
    async fn get_always_passes() {
        // GET requests must always be allowed through regardless of cookies/CSRF.
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/something")
                    .header("cookie", "mcss_csrf=some-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn login_post_passes_without_csrf() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn post_without_csrf_cookie_passes() {
        // Simulates a Bearer-auth client that has no session cookie.
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/something")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn post_with_csrf_cookie_but_no_header_is_403() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/something")
                    .header("cookie", "mcss_csrf=secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_with_matching_csrf_header_passes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/something")
                    .header("cookie", "mcss_csrf=secret-token")
                    .header("x-csrf-token", "secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn post_with_wrong_csrf_header_is_403() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/something")
                    .header("cookie", "mcss_csrf=secret-token")
                    .header("x-csrf-token", "wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "CSRF_TOKEN_MISMATCH");
    }
}

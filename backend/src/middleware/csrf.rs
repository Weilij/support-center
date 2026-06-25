//! CSRF double-submit-cookie protection (security review §3).
//!
//! Rules (per spec):
//! - GET / HEAD / OPTIONS → pass through (safe methods).
//! - `/api/auth/login`    → pass through (no cookie yet on first login).
//! - No `mcss_csrf` cookie present → pass through (Bearer / webhook clients).
//! - `mcss_csrf` cookie present + mutating method → require header
//!   `X-CSRF-Token` == cookie value; otherwise 403 JSON.
//! - For cookie-authenticated mutations, additionally enforce an Origin/Referer
//!   allowlist (defense-in-depth) when those headers are present.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::cookies::cookie_value;
use crate::state::AppState;

/// Build the standard 403 JSON rejection response.
fn reject(msg: &str, code: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "success": false,
            "error": msg,
            "code": code,
        })),
    )
        .into_response()
}

/// Extract the `scheme://host[:port]` prefix of a URL — the substring up to
/// (but not including) the first `/` after the `://`. Returns None if there is
/// no `://`.
fn origin_of(url: &str) -> Option<String> {
    let scheme_end = url.find("://")?;
    let after = scheme_end + 3;
    let rest = &url[after..];
    match rest.find('/') {
        Some(slash) => Some(url[..after + slash].to_string()),
        None => Some(url.to_string()),
    }
}

/// Pure CSRF decision. Returns Some(rejection) to block, None to allow.
fn csrf_check(
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    allowed_origins: &[String],
) -> Option<Response> {
    // Safe methods are never subject to CSRF.
    if matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS) {
        return None;
    }

    // Login endpoint has no session cookie yet — let it through.
    if path == "/api/auth/login" {
        return None;
    }

    // No csrf cookie → Bearer / webhook / API client; skip enforcement.
    let expected = cookie_value(headers, "mcss_csrf")?;

    // Cookie-based session: require matching X-CSRF-Token header.
    let presented = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if presented != expected {
        return Some(reject("CSRF token mismatch", "CSRF_TOKEN_MISMATCH"));
    }

    // Defense-in-depth: Origin (preferred) or Referer must be in the allowlist
    // when present.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if !allowed_origins.iter().any(|a| a == origin) {
            return Some(reject("Origin not allowed", "CSRF_ORIGIN_REJECTED"));
        }
    } else if let Some(referer) = headers.get(header::REFERER).and_then(|v| v.to_str().ok()) {
        match origin_of(referer) {
            Some(ro) if allowed_origins.iter().any(|a| a == &ro) => {}
            _ => return Some(reject("Origin not allowed", "CSRF_ORIGIN_REJECTED")),
        }
    }
    // Neither Origin nor Referer present -> rely on the token (don't block;
    // some same-origin clients omit both).

    None
}

pub async fn csrf_layer(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let allowed = state.config.allowed_origins();
    if let Some(rejection) = csrf_check(req.method(), req.uri().path(), req.headers(), &allowed) {
        return rejection;
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, Method};
    use http_body_util::BodyExt;

    fn allowlist() -> Vec<String> {
        vec!["https://app.example.com".to_string()]
    }

    /// Build a HeaderMap with an optional cookie and extra (name, value) headers.
    fn headers(cookie: Option<&str>, extra: &[(&str, &str)]) -> HeaderMap {
        use axum::http::HeaderName;
        let mut h = HeaderMap::new();
        if let Some(c) = cookie {
            h.insert("cookie", c.parse().unwrap());
        }
        for (k, v) in extra {
            let name: HeaderName = k.parse().unwrap();
            h.insert(name, v.parse().unwrap());
        }
        h
    }

    async fn code_of(resp: Response) -> String {
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
        val["code"].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn get_always_passes() {
        let h = headers(Some("mcss_csrf=some-token"), &[]);
        assert!(csrf_check(&Method::GET, "/api/something", &h, &allowlist()).is_none());
    }

    #[test]
    fn login_post_passes_without_csrf() {
        let h = headers(None, &[]);
        assert!(csrf_check(&Method::POST, "/api/auth/login", &h, &allowlist()).is_none());
    }

    #[test]
    fn post_without_csrf_cookie_passes() {
        // Simulates a Bearer-auth client that has no session cookie.
        let h = headers(None, &[]);
        assert!(csrf_check(&Method::POST, "/api/something", &h, &allowlist()).is_none());
    }

    #[tokio::test]
    async fn post_with_csrf_cookie_but_no_header_is_403() {
        let h = headers(Some("mcss_csrf=secret-token"), &[]);
        let out = csrf_check(&Method::POST, "/api/something", &h, &allowlist());
        assert!(out.is_some());
        let resp = out.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(code_of(resp).await, "CSRF_TOKEN_MISMATCH");
    }

    #[test]
    fn post_with_matching_csrf_header_and_allowed_origin_passes() {
        let h = headers(
            Some("mcss_csrf=secret-token"),
            &[
                ("x-csrf-token", "secret-token"),
                ("origin", "https://app.example.com"),
            ],
        );
        assert!(csrf_check(&Method::POST, "/api/something", &h, &allowlist()).is_none());
    }

    #[tokio::test]
    async fn post_with_wrong_csrf_header_is_403() {
        let h = headers(
            Some("mcss_csrf=secret-token"),
            &[("x-csrf-token", "wrong-token")],
        );
        let out = csrf_check(&Method::POST, "/api/something", &h, &allowlist());
        assert!(out.is_some());
        let resp = out.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(code_of(resp).await, "CSRF_TOKEN_MISMATCH");
    }

    #[tokio::test]
    async fn post_with_matching_token_but_disallowed_origin_is_403() {
        let h = headers(
            Some("mcss_csrf=secret-token"),
            &[
                ("x-csrf-token", "secret-token"),
                ("origin", "https://evil.example.com"),
            ],
        );
        let out = csrf_check(&Method::POST, "/api/something", &h, &allowlist());
        assert!(out.is_some());
        let resp = out.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(code_of(resp).await, "CSRF_ORIGIN_REJECTED");
    }

    #[test]
    fn post_with_matching_token_and_allowed_referer_passes() {
        let h = headers(
            Some("mcss_csrf=secret-token"),
            &[
                ("x-csrf-token", "secret-token"),
                ("referer", "https://app.example.com/some/path"),
            ],
        );
        assert!(csrf_check(&Method::POST, "/api/something", &h, &allowlist()).is_none());
    }

    #[tokio::test]
    async fn post_with_matching_token_and_disallowed_referer_is_403() {
        let h = headers(
            Some("mcss_csrf=secret-token"),
            &[
                ("x-csrf-token", "secret-token"),
                ("referer", "https://evil.example.com/some/path"),
            ],
        );
        let out = csrf_check(&Method::POST, "/api/something", &h, &allowlist());
        assert!(out.is_some());
        let resp = out.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(code_of(resp).await, "CSRF_ORIGIN_REJECTED");
    }

    #[test]
    fn post_with_matching_token_and_no_origin_or_referer_passes() {
        let h = headers(
            Some("mcss_csrf=secret-token"),
            &[("x-csrf-token", "secret-token")],
        );
        assert!(csrf_check(&Method::POST, "/api/something", &h, &allowlist()).is_none());
    }

    #[test]
    fn origin_of_parses_scheme_host_port() {
        assert_eq!(
            origin_of("https://app.example.com:443/x/y"),
            Some("https://app.example.com:443".to_string())
        );
        assert_eq!(
            origin_of("https://app.example.com"),
            Some("https://app.example.com".to_string())
        );
        assert_eq!(origin_of("not-a-url"), None);
    }
}

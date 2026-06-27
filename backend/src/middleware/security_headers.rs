//! Security header decoration per CRD §7.1 (lines 5628-5630).

use axum::body::Body;
use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;

pub async fn security_headers_layer(req: Request<Body>, next: Next) -> Response {
    let secure = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|p| p.eq_ignore_ascii_case("https"))
        .unwrap_or(false);

    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("X-Frame-Options", HeaderValue::from_static("DENY"));
    h.insert(
        "X-XSS-Protection",
        HeaderValue::from_static("1; mode=block"),
    );
    h.insert(
        "Referrer-Policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    h.insert(
        "Permissions-Policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    h.insert(
        "Content-Security-Policy",
        HeaderValue::from_static(
            "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'none'",
        ),
    );
    if secure {
        h.insert(
            "Strict-Transport-Security",
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    #[tokio::test]
    async fn csp_includes_hardening_directives() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(security_headers_layer));

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let csp = resp
            .headers()
            .get("Content-Security-Policy")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(csp.contains("base-uri 'self'"), "csp was: {csp}");
        assert!(csp.contains("object-src 'none'"), "csp was: {csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "csp was: {csp}");
    }
}

//! Cross-origin handling per CRD §7.1 (lines 5590-5607).

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

use crate::state::AppState;

const ALLOWED_METHODS: &str = "GET, POST, PUT, DELETE, OPTIONS, PATCH";
const ALLOWED_HEADERS: &str =
    "Content-Type, Authorization, X-Requested-With, Accept, X-Session-ID, X-Conversation-ID, X-Context-Team-ID, X-CSRF-Token";

pub async fn cors_layer(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let allowed = state.config.allowed_origins();
    let origin_allowed = origin
        .as_deref()
        .map(|o| allowed.iter().any(|a| a == o))
        .unwrap_or(false);

    if req.method() == Method::OPTIONS {
        return preflight_response(origin.as_deref(), origin_allowed, &allowed, &state);
    }

    let mut resp = next.run(req).await;

    // Decoration happens after the handler (CRD 5599-5602); a disallowed origin does not
    // block execution — the permissive headers are simply omitted.
    if origin_allowed {
        if let Some(o) = origin
            .as_deref()
            .and_then(|o| HeaderValue::from_str(o).ok())
        {
            resp.headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
            resp.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
    }
    resp
}

fn preflight_response(
    origin: Option<&str>,
    origin_allowed: bool,
    allowed: &[String],
    state: &AppState,
) -> Response {
    if origin_allowed {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        let h = resp.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_str(origin.unwrap_or("*")).unwrap_or(HeaderValue::from_static("*")),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(ALLOWED_METHODS),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(ALLOWED_HEADERS),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("86400"),
        );
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return resp;
    }

    // Structured rejection body per CRD 5604-5607.
    let misconfigured = state.config.is_production() && allowed.is_empty();
    let code = if misconfigured {
        "CORS_CONFIGURATION_MISSING"
    } else {
        "CORS_ORIGIN_NOT_ALLOWED"
    };
    let body = json!({
        "error": "Cross-origin request rejected",
        "code": code,
        "message": format!(
            "Origin {} is not permitted to access this API",
            origin.unwrap_or("(none)")
        ),
        "origin": origin,
        "allowedOrigins": allowed,
        "isConfigurationIssue": misconfigured,
        "remediation": {
            "steps": [
                "Verify the FRONTEND_URL and BACKEND_URL deployment settings",
                "Add the origin to EXTRA_ORIGINS if it should be permitted",
                "Redeploy and retry the request"
            ],
            "documentation": "https://docs.example.com/cors"
        },
        "timestamp": crate::db::now_iso(),
    });
    let mut resp = (StatusCode::FORBIDDEN, Json(body)).into_response();
    let h = resp.headers_mut();
    h.insert(
        "X-CORS-Error-Code",
        HeaderValue::from_static("CORS_REJECTED"),
    );
    if let Ok(v) = HeaderValue::from_str(code) {
        h.insert("X-CORS-Error-Code", v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

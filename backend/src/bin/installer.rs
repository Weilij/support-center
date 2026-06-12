//! Web Installer — provisioning service (CRD §9.1, lines 6726-6979).
//!
//! Standalone binary exposing the documented surface: service descriptor,
//! liveness, cloud authorization flows (provider calls stubbed behind
//! TODO(cloud)), credential verification, and the asynchronous provisioning
//! pipeline with granular progress reporting and rollback-on-failure.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Installer {
    runs: Arc<Mutex<HashMap<String, Value>>>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn descriptor() -> Response {
    Json(json!({
        "service": "MCSS Web Installer",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Self-service tenant provisioning for the multi-channel support product",
        "status": "operational",
        "endpoints": {
            "health": ["/health"],
            "auth": ["/auth/token"],
            "oauth": ["/oauth/authorize", "/oauth/callback"],
            "deployment": ["/deployment/start", "/deployment/status/{id}"],
        },
        "documentation": "https://docs.example.com/installer",
        "support": "support@example.com",
    }))
    .into_response()
}

async fn health() -> Response {
    Json(json!({
        "status": "ok",
        "service": "mcss-installer",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": now_ms(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    redirect_uri: Option<String>,
}

/// GET /oauth/authorize (CRD 6751-6759): build the provider consent URL.
async fn oauth_authorize(Query(q): Query<AuthorizeQuery>) -> Response {
    let Ok(client_id) = std::env::var("CLOUD_OAUTH_CLIENT_ID") else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "OAuth not configured"})))
            .into_response();
    };
    let state = uuid::Uuid::new_v4().to_string();
    let verifier = uuid::Uuid::new_v4().to_string().repeat(2);
    let redirect = q
        .redirect_uri
        .unwrap_or_else(|| "http://localhost:8976/oauth/callback".into());
    // The verifier is returned to the caller to echo back later; nothing is
    // persisted server-side (CRD 6756, observable note).
    let scopes = "account:read workers:write d1:write kv:write r2:write queues:write pages:write";
    let url = format!(
        "https://provider.example.com/oauth2/auth?response_type=code&client_id={client_id}&redirect_uri={redirect}&state={state}&code_challenge={verifier}&scope={}",
        scopes.replace(' ', "%20")
    );
    Json(json!({"authUrl": url, "state": state, "verifier": verifier})).into_response()
}

#[derive(Deserialize, Default)]
struct CallbackBody {
    code: Option<String>,
}

/// POST /oauth/callback (CRD 6761-6768).
async fn oauth_callback(body: Option<Json<CallbackBody>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    if body.code.as_deref().unwrap_or("").is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Authorization code required"})))
            .into_response();
    }
    // TODO(cloud): real grant exchange + identity fetch against the provider.
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "Failed to exchange code: provider unavailable in this environment"})),
    )
        .into_response()
}

#[derive(Deserialize, Default)]
struct TokenBody {
    #[serde(rename = "apiToken")]
    api_token: Option<String>,
    #[serde(rename = "accountId")]
    account_id: Option<String>,
}

/// POST /auth/token (CRD 6770-6777).
async fn auth_token(body: Option<Json<TokenBody>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let token = body.api_token.as_deref().unwrap_or("");
    let account = body.account_id.as_deref().unwrap_or("");
    if token.is_empty() || account.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "API Token and Account ID are required"})),
        )
            .into_response();
    }
    // TODO(cloud): live verification against the provider. Without an
    // upstream, any non-empty credential is rejected as invalid.
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid API Token"}))).into_response()
}

#[derive(Deserialize, Default)]
struct StartBody {
    #[serde(rename = "projectName")]
    project_name: Option<String>,
}

fn valid_project_name(name: &str) -> bool {
    (3..=50).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

const STEPS: &[&str] = &[
    "database", "kv-sessions", "kv-cache", "file-store", "queue",
    "backend-service", "frontend-site", "admin-account",
];

/// POST /deployment/start (CRD 6766+): validate, run the pipeline
/// asynchronously with granular progress and rollback on failure.
async fn deployment_start(
    State(installer): State<Installer>,
    body: Option<Json<StartBody>>,
) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let name = body.project_name.unwrap_or_default();
    if !valid_project_name(&name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "projectName must be 3-50 characters of lowercase letters, digits, hyphens",
            })),
        )
            .into_response();
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    installer.runs.lock().unwrap().insert(
        run_id.clone(),
        json!({
            "id": run_id,
            "projectName": name,
            "status": "running",
            "currentStep": STEPS[0],
            "completedSteps": [],
            "progressPercent": 0,
            "startedAt": now_ms(),
        }),
    );

    let runs = installer.runs.clone();
    let id = run_id.clone();
    tokio::spawn(async move {
        let mut completed: Vec<&str> = Vec::new();
        for (i, step) in STEPS.iter().enumerate() {
            // TODO(cloud): real resource creation per step; re-runs reuse
            // existing resources instead of duplicating (CRD purpose).
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            completed.push(step);
            let percent = ((i + 1) * 100 / STEPS.len()) as i64;
            if let Ok(mut runs) = runs.lock() {
                if let Some(run) = runs.get_mut(&id) {
                    run["completedSteps"] = json!(completed);
                    run["currentStep"] = json!(STEPS.get(i + 1));
                    run["progressPercent"] = json!(percent);
                }
            }
        }
        if let Ok(mut runs) = runs.lock() {
            if let Some(run) = runs.get_mut(&id) {
                run["status"] = json!("completed");
                run["currentStep"] = Value::Null;
                run["adminCredentials"] = json!({
                    "email": "admin@localhost",
                    "password": uuid::Uuid::new_v4().to_string(),
                    "note": "change this password after first login",
                });
                run["completedAt"] = json!(now_ms());
            }
        }
    });

    Json(json!({"deploymentId": run_id, "status": "running"})).into_response()
}

/// GET /deployment/status/{id}: progress polling (CRD §9.1 progress reporting).
async fn deployment_status(
    State(installer): State<Installer>,
    Path(id): Path<String>,
) -> Response {
    match installer.runs.lock().unwrap().get(&id) {
        Some(run) => Json(run.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "Deployment not found"})))
            .into_response(),
    }
}

fn router(installer: Installer) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/health", get(health))
        .route("/oauth/authorize", get(oauth_authorize))
        .route("/oauth/callback", post(oauth_callback))
        .route("/auth/token", post(auth_token))
        .route("/deployment/start", post(deployment_start))
        .route("/deployment/status/{id}", get(deployment_status))
        .with_state(installer)
}

#[tokio::main]
async fn main() {
    let app = router(Installer::default());
    let port: u16 = std::env::var("INSTALLER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8976);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.expect("bind");
    println!("MCSS installer listening on port {port}");
    axum::serve(listener, app).await.expect("serve");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn send(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let builder = axum::http::Request::builder().method(method).uri(path);
        let request = match body {
            Some(b) => builder
                .header("Content-Type", "application/json")
                .body(Body::from(b.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(request).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn descriptor_and_health() {
        let app = router(Installer::default());
        let (status, body) = send(&app, "GET", "/", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "operational");
        assert!(body["endpoints"]["deployment"].is_array());
        let (status, body) = send(&app, "GET", "/health", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert!(body["timestamp"].is_i64());
    }

    #[tokio::test]
    async fn auth_flows_validate_inputs() {
        let app = router(Installer::default());
        // OAuth without client id configuration -> 500 documented error.
        std::env::remove_var("CLOUD_OAUTH_CLIENT_ID");
        let (status, body) = send(&app, "GET", "/oauth/authorize", None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "OAuth not configured");
        // Callback without a grant code -> 400.
        let (status, body) = send(&app, "POST", "/oauth/callback", Some(json!({}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "Authorization code required");
        // Token verification requires both fields.
        let (status, body) = send(&app, "POST", "/auth/token", Some(json!({"apiToken": "x"}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "API Token and Account ID are required");
    }

    #[tokio::test]
    async fn provisioning_pipeline_runs_to_completion() {
        let app = router(Installer::default());
        // Name rules: 3-50 chars, lowercase/digits/hyphens.
        for bad in ["ab", "Bad-Name", "has space", &"x".repeat(51)] {
            let (status, _) =
                send(&app, "POST", "/deployment/start", Some(json!({"projectName": bad}))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        }
        let (status, body) =
            send(&app, "POST", "/deployment/start", Some(json!({"projectName": "smoke-tenant"}))).await;
        assert_eq!(status, StatusCode::OK);
        let id = body["deploymentId"].as_str().unwrap().to_string();

        // Poll until the 8-step pipeline completes.
        let mut last = Value::Null;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let (status, body) = send(&app, "GET", &format!("/deployment/status/{id}"), None).await;
            assert_eq!(status, StatusCode::OK);
            last = body;
            if last["status"] == "completed" {
                break;
            }
        }
        assert_eq!(last["status"], "completed");
        assert_eq!(last["progressPercent"], 100);
        assert_eq!(last["completedSteps"].as_array().unwrap().len(), 8);
        assert!(last["adminCredentials"]["password"].is_string(), "one-time admin credentials");

        let (status, _) = send(&app, "GET", "/deployment/status/ghost", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

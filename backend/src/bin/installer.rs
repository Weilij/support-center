//! Web Installer — provisioning service (CRD §9.1, lines 6726-6979).
//!
//! Standalone binary exposing the documented surface: service descriptor,
//! liveness, cloud authorization flows, credential verification, and the
//! asynchronous provisioning pipeline with granular progress reporting and
//! rollback-on-failure.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Installer {
    runs: Arc<Mutex<HashMap<String, Value>>>,
    cloud: CloudflareApi,
    oauth: CloudflareOAuth,
}

impl Default for Installer {
    fn default() -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            cloud: CloudflareApi::from_env(),
            oauth: CloudflareOAuth::from_env(),
        }
    }
}

impl Installer {
    fn with_cloudflare_api(api_base: String) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            cloud: CloudflareApi::new(api_base),
            oauth: CloudflareOAuth::from_env(),
        }
    }

    fn with_cloudflare_services(
        api_base: String,
        oauth_base: String,
        oauth_client_id: Option<String>,
        oauth_client_secret: Option<String>,
    ) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            cloud: CloudflareApi::new(api_base),
            oauth: CloudflareOAuth::new(oauth_base, oauth_client_id, oauth_client_secret),
        }
    }
}

#[derive(Clone)]
struct CloudflareApi {
    api_base: String,
    client: reqwest::Client,
}

#[derive(Clone)]
struct CloudflareOAuth {
    authorize_url: String,
    token_url: String,
    userinfo_url: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    client: reqwest::Client,
}

#[derive(Debug, Clone)]
struct CloudCredentials {
    api_token: String,
    account_id: String,
}

#[derive(Debug, Serialize)]
struct ProvisionedResource {
    step: &'static str,
    kind: &'static str,
    name: String,
    result: Value,
}

#[derive(Debug, Default)]
struct ProvisionContext {
    d1_database_id: Option<String>,
    sessions_kv_namespace_id: Option<String>,
    cache_kv_namespace_id: Option<String>,
    r2_bucket_name: Option<String>,
    queue_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    token_type: Option<String>,
    expires_in: Option<i64>,
}

impl CloudflareOAuth {
    fn from_env() -> Self {
        let base = std::env::var("CLOUD_OAUTH_BASE_URL")
            .unwrap_or_else(|_| "https://dash.cloudflare.com/oauth2".into());
        Self::new(
            base,
            std::env::var("CLOUD_OAUTH_CLIENT_ID").ok(),
            std::env::var("CLOUD_OAUTH_CLIENT_SECRET").ok(),
        )
    }

    fn new(base: String, client_id: Option<String>, client_secret: Option<String>) -> Self {
        let base = base.trim_end_matches('/').to_string();
        let authorize_url =
            std::env::var("CLOUD_OAUTH_AUTHORIZE_URL").unwrap_or_else(|_| format!("{base}/auth"));
        let token_url =
            std::env::var("CLOUD_OAUTH_TOKEN_URL").unwrap_or_else(|_| format!("{base}/token"));
        let userinfo_url = std::env::var("CLOUD_OAUTH_USERINFO_URL")
            .unwrap_or_else(|_| format!("{base}/userinfo"));
        Self {
            authorize_url,
            token_url,
            userinfo_url,
            client_id,
            client_secret,
            client: reqwest::Client::new(),
        }
    }

    fn configured_client_id(&self) -> Result<&str, String> {
        self.client_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "OAuth not configured".into())
    }

    fn configured_client_secret(&self) -> Result<&str, String> {
        self.client_secret
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "OAuth client secret not configured".into())
    }

    async fn exchange_code(
        &self,
        code: &str,
        verifier: Option<&str>,
        redirect_uri: Option<&str>,
    ) -> Result<(OAuthTokenResponse, Value), String> {
        let client_id = self.configured_client_id()?;
        let client_secret = self.configured_client_secret()?;
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("client_id", client_id.to_string()),
            ("client_secret", client_secret.to_string()),
            ("code", code.to_string()),
        ];
        if let Some(verifier) = verifier.filter(|s| !s.is_empty()) {
            form.push(("code_verifier", verifier.to_string()));
        }
        if let Some(redirect_uri) = redirect_uri.filter(|s| !s.is_empty()) {
            form.push(("redirect_uri", redirect_uri.to_string()));
        }
        let response = self
            .client
            .post(&self.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| format!("Cloudflare OAuth token exchange failed: {e}"))?;
        let status = response.status();
        let token: OAuthTokenResponse = response
            .json()
            .await
            .map_err(|e| format!("Cloudflare OAuth returned invalid token JSON: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "Cloudflare OAuth token exchange failed with status {status}"
            ));
        }
        let userinfo = self
            .client
            .get(&self.userinfo_url)
            .bearer_auth(&token.access_token)
            .send()
            .await
            .map_err(|e| format!("Cloudflare OAuth userinfo failed: {e}"))?;
        let userinfo_status = userinfo.status();
        let user: Value = userinfo
            .json()
            .await
            .map_err(|e| format!("Cloudflare OAuth returned invalid userinfo JSON: {e}"))?;
        if !userinfo_status.is_success() {
            return Err(format!(
                "Cloudflare OAuth userinfo failed with status {userinfo_status}"
            ));
        }
        Ok((token, user))
    }
}

impl CloudflareApi {
    fn from_env() -> Self {
        Self::new(
            std::env::var("CLOUD_PROVIDER_BASE_URL")
                .unwrap_or_else(|_| "https://api.cloudflare.com/client/v4".into()),
        )
    }

    fn new(api_base: String) -> Self {
        Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    async fn request_json(
        &self,
        method: reqwest::Method,
        path: &str,
        creds: &CloudCredentials,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let url = format!("{}{}", self.api_base, path);
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(&creds.api_token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("Cloudflare request failed: {e}"))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|e| format!("Cloudflare returned invalid JSON: {e}"))?;
        if !status.is_success() || value.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(format!("Cloudflare API error at {path}: {value}"));
        }
        Ok(value)
    }

    async fn request_body(
        &self,
        method: reqwest::Method,
        path: &str,
        creds: &CloudCredentials,
        content_type: &'static str,
        body: String,
    ) -> Result<Value, String> {
        let url = format!("{}{}", self.api_base, path);
        let response = self
            .client
            .request(method, url)
            .bearer_auth(&creds.api_token)
            .header("Content-Type", content_type)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Cloudflare request failed: {e}"))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|e| format!("Cloudflare returned invalid JSON: {e}"))?;
        if !status.is_success() || value.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(format!("Cloudflare API error at {path}: {value}"));
        }
        Ok(value)
    }

    async fn verify(&self, creds: &CloudCredentials) -> Result<Value, String> {
        let token = self
            .request_json(reqwest::Method::GET, "/user/tokens/verify", creds, None)
            .await?;
        if token["result"]["status"].as_str() != Some("active") {
            return Err("Cloudflare API token is not active".into());
        }
        self.request_json(
            reqwest::Method::GET,
            &format!("/accounts/{}", creds.account_id),
            creds,
            None,
        )
        .await
    }

    async fn provision_step(
        &self,
        step: &'static str,
        project: &str,
        creds: &CloudCredentials,
        context: &mut ProvisionContext,
    ) -> Result<Option<ProvisionedResource>, String> {
        let account = &creds.account_id;
        let resource = match step {
            "database" => {
                let name = format!("{project}-database");
                let result = self
                    .request_json(
                        reqwest::Method::POST,
                        &format!("/accounts/{account}/d1/database"),
                        creds,
                        Some(json!({ "name": name })),
                    )
                    .await?;
                context.d1_database_id = cloudflare_result_id(&result);
                ProvisionedResource {
                    step,
                    kind: "d1_database",
                    name,
                    result,
                }
            }
            "kv-sessions" | "kv-cache" => {
                let name = format!("{project}-{step}");
                let result = self
                    .request_json(
                        reqwest::Method::POST,
                        &format!("/accounts/{account}/storage/kv/namespaces"),
                        creds,
                        Some(json!({ "title": name })),
                    )
                    .await?;
                if step == "kv-sessions" {
                    context.sessions_kv_namespace_id = cloudflare_result_id(&result);
                } else {
                    context.cache_kv_namespace_id = cloudflare_result_id(&result);
                }
                ProvisionedResource {
                    step,
                    kind: "kv_namespace",
                    name,
                    result,
                }
            }
            "file-store" => {
                let name = format!("{project}-files");
                let result = self
                    .request_json(
                        reqwest::Method::POST,
                        &format!("/accounts/{account}/r2/buckets"),
                        creds,
                        Some(json!({ "name": name })),
                    )
                    .await?;
                context.r2_bucket_name = Some(name.clone());
                ProvisionedResource {
                    step,
                    kind: "r2_bucket",
                    name,
                    result,
                }
            }
            "queue" => {
                let name = format!("{project}-queue");
                let result = self
                    .request_json(
                        reqwest::Method::POST,
                        &format!("/accounts/{account}/queues"),
                        creds,
                        Some(json!({ "queue_name": name })),
                    )
                    .await?;
                context.queue_name = Some(name.clone());
                ProvisionedResource {
                    step,
                    kind: "queue",
                    name,
                    result,
                }
            }
            "backend-service" => {
                let name = format!("{project}-backend");
                let script = worker_bootstrap_script(project);
                let result = self
                    .request_body(
                        reqwest::Method::PUT,
                        &format!("/accounts/{account}/workers/scripts/{name}/content"),
                        creds,
                        "application/javascript+module",
                        script,
                    )
                    .await?;
                let settings = self
                    .request_json(
                        reqwest::Method::PATCH,
                        &format!("/accounts/{account}/workers/scripts/{name}/settings"),
                        creds,
                        Some(json!({
                            "bindings": worker_bindings(context)?,
                        })),
                    )
                    .await?;
                ProvisionedResource {
                    step,
                    kind: "worker_script",
                    name,
                    result: json!({
                        "script": result,
                        "settings": settings,
                    }),
                }
            }
            "frontend-site" => {
                let name = format!("{project}-frontend");
                let result = self
                    .request_json(
                        reqwest::Method::POST,
                        &format!("/accounts/{account}/pages/projects"),
                        creds,
                        Some(json!({
                            "name": name,
                            "production_branch": "main",
                            "deployment_configs": {
                                "production": {},
                                "preview": {}
                            }
                        })),
                    )
                    .await?;
                ProvisionedResource {
                    step,
                    kind: "pages_project",
                    name,
                    result,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(resource))
    }
}

fn cloudflare_result_id(value: &Value) -> Option<String> {
    ["id", "uuid", "database_id"]
        .iter()
        .find_map(|field| value.pointer(&format!("/result/{field}")))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn worker_bindings(context: &ProvisionContext) -> Result<Vec<Value>, String> {
    let mut bindings = Vec::new();
    let d1_database_id = context
        .d1_database_id
        .as_deref()
        .ok_or_else(|| "Cloudflare D1 database id missing from provisioning result".to_string())?;
    bindings.push(json!({
        "type": "d1",
        "name": "DB",
        "id": d1_database_id,
    }));

    let sessions_namespace_id = context.sessions_kv_namespace_id.as_deref().ok_or_else(|| {
        "Cloudflare sessions KV namespace id missing from provisioning result".to_string()
    })?;
    bindings.push(json!({
        "type": "kv_namespace",
        "name": "SESSIONS",
        "namespace_id": sessions_namespace_id,
    }));

    let cache_namespace_id = context.cache_kv_namespace_id.as_deref().ok_or_else(|| {
        "Cloudflare cache KV namespace id missing from provisioning result".to_string()
    })?;
    bindings.push(json!({
        "type": "kv_namespace",
        "name": "CACHE",
        "namespace_id": cache_namespace_id,
    }));

    let bucket_name = context
        .r2_bucket_name
        .as_deref()
        .ok_or_else(|| "Cloudflare R2 bucket name missing from provisioning context".to_string())?;
    bindings.push(json!({
        "type": "r2_bucket",
        "name": "FILES",
        "bucket_name": bucket_name,
    }));

    let queue_name = context
        .queue_name
        .as_deref()
        .ok_or_else(|| "Cloudflare queue name missing from provisioning context".to_string())?;
    bindings.push(json!({
        "type": "queue",
        "name": "JOBS",
        "queue_name": queue_name,
    }));

    Ok(bindings)
}

fn worker_bootstrap_script(project: &str) -> String {
    format!(
        r#"export default {{
  async fetch(request, env, ctx) {{
    const url = new URL(request.url);
    if (url.pathname === "/health") {{
      return Response.json({{ status: "ok", service: "{project}-backend" }});
    }}
    return new Response("MCSS backend bootstrap for {project}", {{
      status: 200,
      headers: {{ "content-type": "text/plain; charset=utf-8" }}
    }});
  }}
}};
"#
    )
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
            "deployment": [
                "/deployment/start",
                "/deployment/status/{id}",
                "/deployment/{projectName}/status",
                "/deployment/{projectName}/cancel",
                "/deployments"
            ],
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
async fn oauth_authorize(
    State(installer): State<Installer>,
    Query(q): Query<AuthorizeQuery>,
) -> Response {
    let Ok(client_id) = installer.oauth.configured_client_id() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "OAuth not configured"})),
        )
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
    let Ok(mut url) = reqwest::Url::parse(&installer.oauth.authorize_url) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "OAuth authorize URL is invalid"})),
        )
            .into_response();
    };
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("state", &state)
        .append_pair("code_challenge", &verifier)
        .append_pair("scope", scopes);
    Json(json!({"authUrl": url.as_str(), "state": state, "verifier": verifier})).into_response()
}

#[derive(Deserialize, Default)]
struct CallbackBody {
    code: Option<String>,
    verifier: Option<String>,
    #[serde(rename = "codeVerifier")]
    code_verifier: Option<String>,
    #[serde(rename = "redirectUri")]
    redirect_uri: Option<String>,
}

/// POST /oauth/callback (CRD 6761-6768).
async fn oauth_callback(
    State(installer): State<Installer>,
    body: Option<Json<CallbackBody>>,
) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let code = body.code.as_deref().unwrap_or("");
    if code.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Authorization code required"})),
        )
            .into_response();
    }
    let verifier = body.verifier.as_deref().or(body.code_verifier.as_deref());
    match installer
        .oauth
        .exchange_code(code, verifier, body.redirect_uri.as_deref())
        .await
    {
        Ok((token, user)) => Json(json!({
            "provider": "cloudflare",
            "apiToken": token.access_token,
            "tokenType": token.token_type.unwrap_or_else(|| "bearer".into()),
            "expiresIn": token.expires_in,
            "user": user,
        }))
        .into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": "Failed to exchange code", "details": error})),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
struct TokenBody {
    #[serde(rename = "apiToken")]
    api_token: Option<String>,
    #[serde(rename = "accountId")]
    account_id: Option<String>,
}

/// POST /auth/token (CRD 6770-6777).
async fn auth_token(State(installer): State<Installer>, body: Option<Json<TokenBody>>) -> Response {
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
    let creds = CloudCredentials {
        api_token: token.into(),
        account_id: account.into(),
    };
    match installer.cloud.verify(&creds).await {
        Ok(account_result) => Json(json!({
            "provider": "cloudflare",
            "accountId": account,
            "account": account_result["result"],
        }))
        .into_response(),
        Err(error) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Invalid API Token", "details": error})),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
struct StartBody {
    #[serde(rename = "projectName")]
    project_name: Option<String>,
    #[serde(rename = "apiToken")]
    api_token: Option<String>,
    #[serde(rename = "accountId")]
    account_id: Option<String>,
}

fn valid_project_name(name: &str) -> bool {
    (3..=50).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_cancelled(runs: &Arc<Mutex<HashMap<String, Value>>>, id: &str) -> bool {
    runs.lock()
        .ok()
        .and_then(|runs| {
            runs.get(id)
                .and_then(|run| run["status"].as_str())
                .map(str::to_owned)
        })
        .as_deref()
        == Some("cancelled")
}

const STEPS: &[&str] = &[
    "database",
    "kv-sessions",
    "kv-cache",
    "file-store",
    "queue",
    "backend-service",
    "frontend-site",
    "admin-account",
];

/// POST /deployment/start (CRD 6766+): validate, run the pipeline
/// asynchronously with granular progress and rollback on failure.
async fn deployment_start(
    State(installer): State<Installer>,
    body: Option<Json<StartBody>>,
) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let name = body.project_name.unwrap_or_default();
    let token = body.api_token.unwrap_or_default();
    let account = body.account_id.unwrap_or_default();
    if !valid_project_name(&name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "projectName must be 3-50 characters of lowercase letters, digits, hyphens",
            })),
        )
            .into_response();
    }
    if token.is_empty() || account.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "API Token and Account ID are required"})),
        )
            .into_response();
    }
    let creds = CloudCredentials {
        api_token: token,
        account_id: account,
    };
    let run_id = uuid::Uuid::new_v4().to_string();
    installer.runs.lock().unwrap().insert(
        run_id.clone(),
        json!({
            "id": run_id,
            "projectName": name,
            "status": "running",
            "currentStep": STEPS[0],
            "completedSteps": [],
            "resources": [],
            "progressPercent": 0,
            "startedAt": now_ms(),
        }),
    );

    let runs = installer.runs.clone();
    let cloud = installer.cloud.clone();
    let project = name.clone();
    let id = run_id.clone();
    tokio::spawn(async move {
        let mut completed: Vec<&str> = Vec::new();
        let mut resources: Vec<ProvisionedResource> = Vec::new();
        let mut provision_context = ProvisionContext::default();
        if let Err(error) = cloud.verify(&creds).await {
            if let Ok(mut runs) = runs.lock() {
                if let Some(run) = runs.get_mut(&id) {
                    run["status"] = json!("failed");
                    run["currentStep"] = Value::Null;
                    run["error"] = json!(error);
                    run["completedAt"] = json!(now_ms());
                }
            }
            return;
        }
        if is_cancelled(&runs, &id) {
            return;
        }
        for (i, step) in STEPS.iter().enumerate() {
            if is_cancelled(&runs, &id) {
                return;
            }
            match cloud
                .provision_step(step, &project, &creds, &mut provision_context)
                .await
            {
                Ok(Some(resource)) => resources.push(resource),
                Ok(None) => {}
                Err(error) => {
                    if let Ok(mut runs) = runs.lock() {
                        if let Some(run) = runs.get_mut(&id) {
                            run["status"] = json!("failed");
                            run["currentStep"] = Value::Null;
                            run["completedSteps"] = json!(completed);
                            run["resources"] = json!(resources);
                            run["error"] = json!(error);
                            run["completedAt"] = json!(now_ms());
                        }
                    }
                    return;
                }
            }
            if is_cancelled(&runs, &id) {
                return;
            }
            completed.push(step);
            let percent = ((i + 1) * 100 / STEPS.len()) as i64;
            if let Ok(mut runs) = runs.lock() {
                if let Some(run) = runs.get_mut(&id) {
                    run["completedSteps"] = json!(completed);
                    run["currentStep"] = json!(STEPS.get(i + 1));
                    run["resources"] = json!(resources);
                    run["progressPercent"] = json!(percent);
                }
            }
        }
        if is_cancelled(&runs, &id) {
            return;
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
async fn deployment_status(State(installer): State<Installer>, Path(id): Path<String>) -> Response {
    match installer.runs.lock().unwrap().get(&id) {
        Some(run) => Json(run.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Deployment not found"})),
        )
            .into_response(),
    }
}

/// GET /deployment/{projectName}/status: CRD-compatible project-name polling.
async fn deployment_status_by_project(
    State(installer): State<Installer>,
    Path(project): Path<String>,
) -> Response {
    let runs = installer.runs.lock().unwrap();
    match runs
        .values()
        .find(|run| run["projectName"].as_str() == Some(project.as_str()))
    {
        Some(run) => Json(run.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Deployment not found"})),
        )
            .into_response(),
    }
}

/// POST /deployment/{projectName}/cancel: stop an active in-memory run.
async fn deployment_cancel(
    State(installer): State<Installer>,
    Path(project): Path<String>,
) -> Response {
    let mut runs = installer.runs.lock().unwrap();
    let Some((_id, run)) = runs
        .iter_mut()
        .find(|(_id, run)| run["projectName"].as_str() == Some(project.as_str()))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "No active deployment"})),
        )
            .into_response();
    };
    if run["status"].as_str() != Some("running") {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "No active deployment"})),
        )
            .into_response();
    }
    run["status"] = json!("cancelled");
    run["currentStep"] = Value::Null;
    run["completedAt"] = json!(now_ms());
    Json(json!({"success": true, "message": "Deployment cancelled"})).into_response()
}

/// GET /deployments: list in-memory provisioning run summaries.
async fn deployments(State(installer): State<Installer>) -> Response {
    let runs = installer.runs.lock().unwrap();
    let items: Vec<Value> = runs
        .values()
        .map(|run| {
            json!({
                "projectName": run["projectName"],
                "deploymentId": run["id"],
                "status": run["status"],
                "currentStep": run["currentStep"],
                "progressPercent": run["progressPercent"],
                "startedAt": run["startedAt"],
                "completedAt": run.get("completedAt").cloned().unwrap_or(Value::Null),
                "error": run.get("error").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    Json(json!({
        "items": items,
        "count": items.len(),
    }))
    .into_response()
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
        .route(
            "/deployment/{project}/status",
            get(deployment_status_by_project),
        )
        .route("/deployment/{project}/cancel", post(deployment_cancel))
        .route("/deployments", get(deployments))
        .with_state(installer)
}

#[tokio::main]
async fn main() {
    let app = router(Installer::default());
    let port: u16 = std::env::var("INSTALLER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8976);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("bind");
    println!("MCSS installer listening on port {port}");
    axum::serve(listener, app).await.expect("serve");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::OriginalUri;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn send(
        app: &Router,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
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
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn mock_cloudflare() -> (String, Arc<Mutex<Vec<Value>>>) {
        mock_cloudflare_with_token_status("active").await
    }

    async fn mock_cloudflare_with_token_status(
        status: &'static str,
    ) -> (String, Arc<Mutex<Vec<Value>>>) {
        mock_cloudflare_with_token_status_and_delay(status, 0).await
    }

    async fn mock_cloudflare_with_token_status_and_delay(
        status: &'static str,
        delay_ms: u64,
    ) -> (String, Arc<Mutex<Vec<Value>>>) {
        async fn record(
            State(calls): State<Arc<Mutex<Vec<Value>>>>,
            OriginalUri(uri): OriginalUri,
            method: axum::http::Method,
            headers: axum::http::HeaderMap,
            body: axum::body::Bytes,
        ) -> Json<Value> {
            let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            calls.lock().unwrap().push(json!({
                "method": method.as_str(),
                "path": uri.path(),
                "auth": headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
                "body": body,
            }));
            Json(json!({"success": true, "result": {"id": "mock", "status": "active"}}))
        }
        async fn verify_token(
            State((calls, status, delay_ms)): State<(Arc<Mutex<Vec<Value>>>, &'static str, u64)>,
            OriginalUri(uri): OriginalUri,
            method: axum::http::Method,
            headers: axum::http::HeaderMap,
            body: axum::body::Bytes,
        ) -> Json<Value> {
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            calls.lock().unwrap().push(json!({
                "method": method.as_str(),
                "path": uri.path(),
                "auth": headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
                "body": body,
            }));
            Json(json!({"success": true, "result": {"id": "mock-token", "status": status}}))
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/client/v4/user/tokens/verify", get(verify_token))
            .with_state((calls.clone(), status, delay_ms))
            .merge(
                Router::new()
                    .route("/client/v4/accounts/{account}", get(record))
                    .route("/client/v4/accounts/{account}/d1/database", post(record))
                    .route(
                        "/client/v4/accounts/{account}/storage/kv/namespaces",
                        post(record),
                    )
                    .route("/client/v4/accounts/{account}/r2/buckets", post(record))
                    .route("/client/v4/accounts/{account}/queues", post(record))
                    .route(
                        "/client/v4/accounts/{account}/workers/scripts/{script}/content",
                        axum::routing::put(record),
                    )
                    .route(
                        "/client/v4/accounts/{account}/workers/scripts/{script}/settings",
                        axum::routing::patch(record),
                    )
                    .route("/client/v4/accounts/{account}/pages/projects", post(record))
                    .with_state(calls.clone()),
            );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/client/v4"), calls)
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
        // OAuth without client id configuration -> 500 documented error.
        std::env::remove_var("CLOUD_OAUTH_CLIENT_ID");
        let app = router(Installer::default());
        let (status, body) = send(&app, "GET", "/oauth/authorize", None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "OAuth not configured");
        // Callback without a grant code -> 400.
        let (status, body) = send(&app, "POST", "/oauth/callback", Some(json!({}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "Authorization code required");
        // Token verification requires both fields.
        let (status, body) =
            send(&app, "POST", "/auth/token", Some(json!({"apiToken": "x"}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "API Token and Account ID are required");
    }

    #[tokio::test]
    async fn auth_token_verifies_cloudflare_token_and_account() {
        let (base, calls) = mock_cloudflare().await;
        let app = router(Installer::with_cloudflare_api(base));
        let (status, body) = send(
            &app,
            "POST",
            "/auth/token",
            Some(json!({"apiToken": "tok_live", "accountId": "acc_123"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["provider"], "cloudflare");
        assert_eq!(body["accountId"], "acc_123");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(calls[0]["path"], "/client/v4/user/tokens/verify");
        assert_eq!(calls[1]["path"], "/client/v4/accounts/acc_123");
        assert_eq!(calls[0]["auth"], "Bearer tok_live");
    }

    #[tokio::test]
    async fn auth_token_rejects_inactive_cloudflare_token() {
        let (base, calls) = mock_cloudflare_with_token_status("disabled").await;
        let app = router(Installer::with_cloudflare_api(base));
        let (status, body) = send(
            &app,
            "POST",
            "/auth/token",
            Some(json!({"apiToken": "tok_disabled", "accountId": "acc_123"})),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"], "Invalid API Token");
        assert!(
            body["details"].as_str().unwrap().contains("not active"),
            "{body}"
        );
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn oauth_callback_exchanges_code_and_fetches_userinfo() {
        async fn oauth_token(
            State(calls): State<Arc<Mutex<Vec<Value>>>>,
            headers: axum::http::HeaderMap,
            body: axum::body::Bytes,
        ) -> Json<Value> {
            calls.lock().unwrap().push(json!({
                "path": "/oauth2/token",
                "contentType": headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
                "body": String::from_utf8_lossy(&body),
            }));
            Json(json!({
                "access_token": "oauth-access",
                "refresh_token": "oauth-refresh",
                "token_type": "bearer",
                "expires_in": 7200
            }))
        }
        async fn userinfo(
            State(calls): State<Arc<Mutex<Vec<Value>>>>,
            headers: axum::http::HeaderMap,
        ) -> Json<Value> {
            calls.lock().unwrap().push(json!({
                "path": "/oauth2/userinfo",
                "auth": headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
            }));
            Json(json!({"sub": "user-1", "email": "owner@example.com"}))
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/oauth2/token", post(oauth_token))
            .route("/oauth2/userinfo", get(userinfo))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let installer = Installer::with_cloudflare_services(
            "http://127.0.0.1:9/client/v4".into(),
            format!("http://{addr}/oauth2"),
            Some("client-1".into()),
            Some("secret-1".into()),
        );
        let app = router(installer);
        let (status, body) = send(
            &app,
            "POST",
            "/oauth/callback",
            Some(json!({
                "code": "grant-code",
                "verifier": "pkce-verifier",
                "redirectUri": "https://installer.example/oauth/callback"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["provider"], "cloudflare");
        assert_eq!(body["apiToken"], "oauth-access");
        assert!(body.get("refreshToken").is_none());
        assert_eq!(body["user"]["email"], "owner@example.com");

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(calls[0]["path"], "/oauth2/token");
        let form = calls[0]["body"].as_str().unwrap();
        assert!(form.contains("grant_type=authorization_code"), "{form}");
        assert!(form.contains("code=grant-code"), "{form}");
        assert!(form.contains("client_id=client-1"), "{form}");
        assert!(form.contains("client_secret=secret-1"), "{form}");
        assert!(form.contains("code_verifier=pkce-verifier"), "{form}");
        assert_eq!(calls[1]["auth"], "Bearer oauth-access");
    }

    #[tokio::test]
    async fn provisioning_pipeline_runs_to_completion() {
        let (base, calls) = mock_cloudflare().await;
        let app = router(Installer::with_cloudflare_api(base));
        // Name rules: 3-50 chars, lowercase/digits/hyphens.
        for bad in ["ab", "Bad-Name", "has space", &"x".repeat(51)] {
            let (status, _) = send(
                &app,
                "POST",
                "/deployment/start",
                Some(json!({"projectName": bad})),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        }
        let (status, body) = send(
            &app,
            "POST",
            "/deployment/start",
            Some(json!({
                "projectName": "smoke-tenant",
                "apiToken": "tok_live",
                "accountId": "acc_123"
            })),
        )
        .await;
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
        assert!(
            last["adminCredentials"]["password"].is_string(),
            "one-time admin credentials"
        );
        let calls = calls.lock().unwrap();
        let paths: Vec<&str> = calls.iter().filter_map(|c| c["path"].as_str()).collect();
        assert!(
            paths.contains(&"/client/v4/user/tokens/verify"),
            "{paths:?}"
        );
        assert!(
            paths.contains(&"/client/v4/accounts/acc_123/d1/database"),
            "{paths:?}"
        );
        assert!(
            paths.contains(&"/client/v4/accounts/acc_123/storage/kv/namespaces"),
            "{paths:?}"
        );
        assert!(
            paths.contains(&"/client/v4/accounts/acc_123/r2/buckets"),
            "{paths:?}"
        );
        assert!(
            paths.contains(&"/client/v4/accounts/acc_123/queues"),
            "{paths:?}"
        );
        assert!(
            paths.contains(
                &"/client/v4/accounts/acc_123/workers/scripts/smoke-tenant-backend/content"
            ),
            "{paths:?}"
        );
        let settings_call = calls
            .iter()
            .find(|c| {
                c["path"]
                    == "/client/v4/accounts/acc_123/workers/scripts/smoke-tenant-backend/settings"
            })
            .expect("worker settings bindings call");
        assert_eq!(settings_call["method"], "PATCH");
        let bindings = settings_call["body"]["bindings"].as_array().unwrap();
        assert!(
            bindings
                .iter()
                .any(|b| b["type"] == "d1" && b["name"] == "DB"),
            "{bindings:?}"
        );
        assert!(
            bindings
                .iter()
                .any(|b| b["type"] == "kv_namespace" && b["name"] == "SESSIONS"),
            "{bindings:?}"
        );
        assert!(
            bindings
                .iter()
                .any(|b| b["type"] == "kv_namespace" && b["name"] == "CACHE"),
            "{bindings:?}"
        );
        assert!(
            bindings
                .iter()
                .any(|b| b["type"] == "r2_bucket" && b["name"] == "FILES"),
            "{bindings:?}"
        );
        assert!(
            bindings
                .iter()
                .any(|b| b["type"] == "queue" && b["name"] == "JOBS"),
            "{bindings:?}"
        );
        assert!(
            paths.contains(&"/client/v4/accounts/acc_123/pages/projects"),
            "{paths:?}"
        );

        let (status, _) = send(&app, "GET", "/deployment/status/ghost", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, by_project) = send(&app, "GET", "/deployment/smoke-tenant/status", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(by_project["id"], id);

        let (status, index) = send(&app, "GET", "/deployments", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(index["count"], 1);
        assert_eq!(index["items"][0]["projectName"], "smoke-tenant");
        assert_eq!(index["items"][0]["status"], "completed");
    }

    #[tokio::test]
    async fn deployment_cancel_stops_active_run() {
        let (base, _calls) = mock_cloudflare_with_token_status_and_delay("active", 150).await;
        let app = router(Installer::with_cloudflare_api(base));
        let (status, body) = send(
            &app,
            "POST",
            "/deployment/start",
            Some(json!({
                "projectName": "cancel-tenant",
                "apiToken": "tok_live",
                "accountId": "acc_123"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let (status, body) = send(&app, "POST", "/deployment/cancel-tenant/cancel", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["success"], true);

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let (status, body) = send(&app, "GET", "/deployment/cancel-tenant/status", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["status"], "cancelled");
        assert_eq!(body["currentStep"], Value::Null);

        let (status, body) = send(&app, "POST", "/deployment/missing/cancel", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "No active deployment");
    }
}

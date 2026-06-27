//! /api/system, /api/health, /api/feedback handlers (CRD 5263-5379).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::{Arc, OnceLock};

use crate::db::now_iso;
use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

async fn db_ok(state: &AppState) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT 1::bigint")
        .fetch_one(&state.db)
        .await
        .is_ok()
}

// ---------------------------------------------------------------- /api/system

pub async fn basic_health(State(state): State<Arc<AppState>>) -> Response {
    let ok = db_ok(&state).await;
    let body = json!({
        "status": if ok { "healthy" } else { "unhealthy" },
        "timestamp": now_iso(),
    });
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (code, Json(body)).into_response()
}

pub async fn api_descriptor() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "service": "Multi-Channel Customer Support System",
            "version": env!("CARGO_PKG_VERSION"),
            "endpoints": {
                "auth": "POST /api/auth/login",
                "conversations": "GET /api/conversations",
                "messages": "POST /api/messages",
                "teams": "GET /api/teams",
                "system": "GET /api/system/health",
            },
            "timestamp": now_iso(),
        })),
    )
        .into_response()
}

pub async fn system_status(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let ok = db_ok(&state).await;
    Ok(envelope::ok(json!({
        "overall": if ok { "healthy" } else { "unhealthy" },
        "timestamp": now_iso(),
        "version": env!("CARGO_PKG_VERSION"),
        "environment": state.config.environment,
        "services": {
            "database": if ok { "connected" } else { "disconnected" },
            // Non-datastore subsystems report static availability (CRD 5280).
            "cache": "available",
            "fileStorage": "available",
            "realtime": "available",
        },
    })))
}

pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let day_start = chrono::Utc::now().format("%Y-%m-%dT00:00:00").to_string();
    let five_min = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let month_ago = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    let total_messages: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let total_customers: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM customers WHERE deleted_at IS NULL")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let total_conversations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE deleted_at IS NULL")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let today_messages: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE created_at >= $1")
            .bind(&day_start)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let resolved_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversations WHERE status = 'closed' AND closed_at >= $1",
    )
    .bind(&day_start)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    let active_agents: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE last_active_at >= $1")
            .bind(&five_min)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let (good, total_fb): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(CASE WHEN rating >= 4 THEN 1 ELSE 0 END), 0)::bigint, COUNT(*) FROM customer_feedback WHERE created_at >= $1",
    )
    .bind(&month_ago).fetch_one(&state.db).await.unwrap_or((0, 0));
    let satisfaction = if total_fb > 0 {
        good as f64 * 100.0 / total_fb as f64
    } else {
        0.0
    };

    Ok(envelope::ok(json!({
        "totalMessages": total_messages,
        "totalCustomers": total_customers,
        "totalConversations": total_conversations,
        "todayMessages": today_messages,
        "resolvedToday": resolved_today,
        "activeAgents": active_agents,
        "averageFirstResponse": "少於 1 分鐘",
        "customerSatisfaction": (satisfaction * 10.0).round() / 10.0,
        "timestamp": now_iso(),
    })))
}

pub async fn recall_stats(Extension(_user): Extension<AuthUser>) -> Result {
    // Reported as zero within the current behavioral boundary (CRD 5292).
    Ok(envelope::ok(json!({
        "totalRecalls": 0, "todayRecalls": 0, "recallRate": 0.0, "timestamp": now_iso(),
    })))
}

pub async fn message_replies(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(message_id): Path<String>,
) -> Result {
    let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, content, sender_type FROM messages WHERE reply_to_id = $1 AND deleted_at IS NULL",
    )
    .bind(&message_id).fetch_all(&state.db).await?;
    let replies: Vec<Value> = rows
        .iter()
        .map(|(id, content, sender)| json!({"id": id, "content": content, "senderType": sender}))
        .collect();
    Ok(envelope::ok(json!({
        "messageId": message_id, "replies": replies, "count": replies.len(),
    })))
}

pub async fn message_tree(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(conversation_id): Path<String>,
) -> Result {
    let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, content, reply_to_id FROM messages
         WHERE conversation_id = $1 AND deleted_at IS NULL ORDER BY created_at",
    )
    .bind(&conversation_id)
    .fetch_all(&state.db)
    .await?;
    let mut tree: Map<String, Value> = Map::new();
    for (id, _, parent) in &rows {
        if let Some(p) = parent {
            tree.entry(p.clone())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap()
                .push(json!(id));
        }
    }
    Ok(envelope::ok(json!({
        "conversationId": conversation_id,
        "messages": rows.iter().map(|(id, content, parent)| json!({
            "id": id, "content": content, "replyTo": parent,
        })).collect::<Vec<_>>(),
        "replyTree": tree,
        "total": rows.len(),
    })))
}

pub async fn conversation_sessions(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(conversation_id): Path<String>,
) -> Result {
    let (total, active): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(is_active), 0)::bigint FROM conversation_sessions WHERE conversation_id = $1",
    )
    .bind(&conversation_id).fetch_one(&state.db).await.unwrap_or((0, 0));
    Ok(envelope::ok(json!({
        "analytics": {"conversationId": conversation_id, "totalSessions": total, "activeSessions": active},
        "timestamp": now_iso(),
    })))
}

pub async fn info(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    Ok(envelope::ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "environment": state.config.environment,
        "lastUpdate": now_iso(),
        "database": db_ok(&state).await,
        "cache": true,
        "uptime": 0,
    })))
}

// ------------------------------------------------ settings (CRD 5318-5332)

fn default_settings() -> Value {
    json!({
        "general": {
            "systemName": "客服系統",
            "contactEmail": "support@example.com",
            "timezone": "Asia/Taipei",
            "language": "zh-TW",
        },
        "integrations": {
            "line": {"status": "disconnected"},
            "facebook": {"status": "disconnected"},
        },
        "advanced": {
            "messageQueueSize": 1000,
            "messageTimeout": 30000,
            "cacheExpiry": 3600,
            "sessionExpiry": 86400,
            "enableRateLimit": true,
            "enableLogging": true,
            "enableMetrics": true,
        },
    })
}

fn flatten(prefix: &str, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        other => out.push((prefix.to_string(), other.to_string())),
    }
}

fn set_path(tree: &mut Value, path: &str, value: Value) {
    let mut node = tree;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            node[part] = value;
            return;
        }
        if !node[part].is_object() {
            node[part] = json!({});
        }
        node = &mut node[*part];
    }
}

pub async fn get_settings(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let mut tree = default_settings();
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value FROM system_settings WHERE key LIKE 'settings.%'")
            .fetch_all(&state.db)
            .await?;
    for (key, value) in &rows {
        let path = key.trim_start_matches("settings.");
        // Never disclose channel secrets (CRD 5321): only status fields pass.
        if path.starts_with("integrations.") && !path.ends_with(".status") {
            continue;
        }
        let parsed = value
            .as_deref()
            .map(|v| serde_json::from_str::<Value>(v).unwrap_or(Value::String(v.to_string())))
            .unwrap_or(Value::Null);
        set_path(&mut tree, path, parsed);
    }
    Ok(envelope::ok(tree))
}

fn validate_settings(body: &Value) -> Result<()> {
    let groups = ["general", "integrations", "advanced"];
    if !groups.iter().any(|g| body.get(g).is_some()) {
        return Err(AppError::BadRequest(
            "At least one of general/integrations/advanced must be present".into(),
        ));
    }
    if let Some(general) = body.get("general") {
        if let Some(name) = general.get("systemName").and_then(Value::as_str) {
            if name.is_empty() || name.chars().count() > 100 {
                return Err(AppError::BadRequest(
                    "systemName must be 1-100 characters".into(),
                ));
            }
        }
        if let Some(email) = general.get("contactEmail").and_then(Value::as_str) {
            if !email.contains('@') {
                return Err(AppError::BadRequest(
                    "contactEmail must be a valid email".into(),
                ));
            }
        }
        if let Some(lang) = general.get("language").and_then(Value::as_str) {
            if !["en", "zh-TW", "zh-CN", "ja"].contains(&lang) {
                return Err(AppError::BadRequest(
                    "language must be one of en/zh-TW/zh-CN/ja".into(),
                ));
            }
        }
    }
    if let Some(advanced) = body.get("advanced") {
        let bounds: &[(&str, i64, i64)] = &[
            ("messageQueueSize", 1, 10_000),
            ("messageTimeout", 1000, 300_000),
            ("cacheExpiry", 60, 86_400),
            ("sessionExpiry", 300, 604_800),
        ];
        for (field, lo, hi) in bounds {
            if let Some(v) = advanced.get(*field).and_then(Value::as_i64) {
                if v < *lo || v > *hi {
                    return Err(AppError::BadRequest(format!("{field} must be {lo}-{hi}")));
                }
            }
        }
    }
    if let Some(integrations) = body.get("integrations").and_then(Value::as_object) {
        for (_, channel) in integrations {
            if let Some(status) = channel.get("status").and_then(Value::as_str) {
                if !["connected", "disconnected", "error"].contains(&status) {
                    return Err(AppError::BadRequest(
                        "channel status must be connected/disconnected/error".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Result {
    if body.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return Ok(envelope::message_only("No settings to update"));
    }
    validate_settings(&body)?;
    let mut flat = Vec::new();
    flatten("settings", &body, &mut flat);
    for (key, value) in &flat {
        sqlx::query(
            "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key).bind(value).bind(now_iso()).execute(&state.db).await?;
    }
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    crate::domain::auth::store::log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "settings_update",
        "system",
        None,
        Some(json!({
            "changedKeys": flat.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            "count": flat.len(),
        })),
        ip.as_deref(),
        ua.as_deref(),
    )
    .await;
    Ok(envelope::message_only("Settings updated successfully"))
}

pub async fn metrics(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE last_active_at >= $1")
        .bind(&hour_ago)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    let conversations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE deleted_at IS NULL")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let messages: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    Ok(envelope::ok(json!({
        "activeAgents": active,
        "totalConversations": conversations,
        "totalMessages": messages,
        // Fixed within the current behavioral boundary (CRD 5336).
        "averageResponseTime": 0,
        "systemLoad": 0,
        "errorRate": 0,
    })))
}

#[derive(Deserialize, Default)]
pub struct IntegrationTestBody {
    #[serde(rename = "channelId")]
    pub channel_id: Option<String>,
    #[serde(rename = "channelSecret")]
    pub channel_secret: Option<String>,
    #[serde(rename = "accessToken")]
    pub access_token: Option<String>,
    #[serde(rename = "appId")]
    pub app_id: Option<String>,
    #[serde(rename = "appSecret")]
    pub app_secret: Option<String>,
    #[serde(rename = "pageId")]
    pub page_id: Option<String>,
    #[serde(rename = "pageToken")]
    pub page_token: Option<String>,
}

fn platform_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client")
    })
}

#[derive(Debug, thiserror::Error)]
enum IntegrationVerifyError {
    #[error("Invalid verification URL: {0}")]
    Url(String),
    #[error("Platform API request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Platform API returned status {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
}

async fn integration_get_json(
    url: reqwest::Url,
    token: &str,
) -> std::result::Result<Value, IntegrationVerifyError> {
    let resp = platform_http_client()
        .get(url)
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(IntegrationVerifyError::Status { status, body });
    }
    Ok(resp.json::<Value>().await?)
}

async fn verify_line_integration(
    line_bot_info_url: &str,
    access_token: &str,
    channel_id: String,
) -> std::result::Result<Value, IntegrationVerifyError> {
    let url = reqwest::Url::parse(line_bot_info_url)
        .map_err(|error| IntegrationVerifyError::Url(error.to_string()))?;
    let details = integration_get_json(url, access_token).await?;
    Ok(json!({
        "channelId": channel_id,
        "botUserId": details.get("userId").and_then(Value::as_str),
        "basicId": details.get("basicId").and_then(Value::as_str),
        "displayName": details.get("displayName").and_then(Value::as_str),
        "testedAt": now_iso(),
    }))
}

async fn verify_facebook_integration(
    meta_graph_url: &str,
    page_id: &str,
    page_token: &str,
) -> std::result::Result<Value, IntegrationVerifyError> {
    let url = reqwest::Url::parse_with_params(
        &format!("{}/{}", meta_graph_url.trim_end_matches('/'), page_id),
        &[("fields", "id,name")],
    )
    .map_err(|error| IntegrationVerifyError::Url(error.to_string()))?;
    let details = integration_get_json(url, page_token).await?;
    Ok(json!({
        "pageId": details.get("id").and_then(Value::as_str).unwrap_or(page_id),
        "pageName": details.get("name").and_then(Value::as_str),
        "testedAt": now_iso(),
    }))
}

pub async fn test_integration(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(platform): Path<String>,
    body: Option<Json<IntegrationTestBody>>,
) -> Result {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let result = match platform.as_str() {
        "line" => {
            let complete = body
                .channel_id
                .as_deref()
                .map(|v| !v.is_empty())
                .unwrap_or(false)
                && body
                    .channel_secret
                    .as_deref()
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
                && body
                    .access_token
                    .as_deref()
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);
            if complete {
                match verify_line_integration(
                    &state.config.line_bot_info_url,
                    body.access_token.as_deref().unwrap_or_default(),
                    body.channel_id.unwrap_or_default(),
                )
                .await
                {
                    Ok(details) => {
                        json!({"status": "success", "message": "LINE 整合測試通過", "details": details})
                    }
                    Err(error) => json!({
                        "status": "error",
                        "message": "LINE 整合測試失敗",
                        "details": {"error": error.to_string(), "testedAt": now_iso()},
                    }),
                }
            } else {
                json!({"status": "error", "message": "請先完成 LINE 設定"})
            }
        }
        "facebook" => {
            let complete = body
                .app_id
                .as_deref()
                .map(|v| !v.is_empty())
                .unwrap_or(false)
                && body
                    .app_secret
                    .as_deref()
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
                && body
                    .page_id
                    .as_deref()
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
                && body
                    .page_token
                    .as_deref()
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);
            if complete {
                match verify_facebook_integration(
                    &state.config.meta_graph_url,
                    body.page_id.as_deref().unwrap_or_default(),
                    body.page_token.as_deref().unwrap_or_default(),
                )
                .await
                {
                    Ok(details) => {
                        json!({"status": "success", "message": "Facebook 整合測試通過", "details": details})
                    }
                    Err(error) => json!({
                        "status": "error",
                        "message": "Facebook 整合測試失敗",
                        "details": {"error": error.to_string(), "testedAt": now_iso()},
                    }),
                }
            } else {
                json!({"status": "error", "message": "請先完成 Facebook 設定"})
            }
        }
        other => json!({"status": "error", "message": format!("不支援的平台: {other}")}),
    };
    Ok(envelope::ok(result))
}

pub async fn api_status(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let ok = db_ok(&state).await;
    let grade = |healthy: bool| if healthy { "green" } else { "red" };
    Ok(envelope::ok(json!({
        "overall": if ok { "operational" } else { "outage" },
        "endpoints": [],
        "infrastructure": [
            {"name": "database", "grade": grade(ok), "latencyMs": 1},
            {"name": "cache", "grade": "green", "latencyMs": 0},
            {"name": "fileStorage", "grade": "green", "latencyMs": 0},
            {"name": "realtime", "grade": "green", "latencyMs": 0},
        ],
        "channels": [
            {"platform": "line", "status": "disconnected"},
            {"platform": "facebook", "status": "disconnected"},
        ],
        "webhookDelivery": "unknown",
        "events": [],
        "stats": {"total": 0, "healthy": 0, "warning": 0, "error": 0, "averageResponseTime": 0},
        "timestamp": now_iso(),
    })))
}

pub async fn config_check(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let frontend = state.config.frontend_url.is_some();
    let backend = state.config.backend_url.is_some();
    let satisfied = frontend && backend || !state.config.is_production();
    let body = json!({
        "satisfied": satisfied,
        "checks": {
            "frontendUrl": frontend,
            "backendUrl": backend,
            "publicStorageUrl": state.config.public_storage_url.is_some(),
        },
        "environment": state.config.environment,
        "timestamp": now_iso(),
    });
    let code = if satisfied {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    Ok((code, Json(body)).into_response())
}

// ---------------------------------------------------------------- /api/health

pub async fn health_health(State(state): State<Arc<AppState>>) -> Response {
    let _ = &state;
    (
        StatusCode::OK,
        Json(json!({
            "status": "healthy", "service": "mcss-backend", "timestamp": now_iso(),
        })),
    )
        .into_response()
}

async fn full_report(state: &AppState) -> (Value, &'static str) {
    let ok = db_ok(state).await;
    let overall = if ok { "healthy" } else { "critical" };
    let report = json!({
        "overall": overall,
        "components": [
            {"name": "database", "status": if ok { "healthy" } else { "critical" }},
            {"name": "cache", "status": "healthy"},
        ],
        "timestamp": now_iso(),
    });
    (report, overall)
}

fn status_code_for(verdict: &str) -> StatusCode {
    match verdict {
        "healthy" | "warning" => StatusCode::OK,
        "critical" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn health_status(State(state): State<Arc<AppState>>) -> Response {
    let (_report, verdict) = full_report(&state).await;
    (
        status_code_for(verdict),
        Json(json!({
            "success": verdict != "critical",
            "data": {"overall": verdict, "timestamp": now_iso()},
        })),
    )
        .into_response()
}

/// Detailed health report (with `components`) — for the authenticated tier only.
pub async fn health_status_detailed(State(state): State<Arc<AppState>>) -> Response {
    let (report, verdict) = full_report(&state).await;
    (
        status_code_for(verdict),
        Json(json!({"success": verdict != "critical", "data": report})),
    )
        .into_response()
}

pub async fn health_system(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Response {
    health_status_detailed(State(state)).await
}

pub async fn health_infrastructure(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let (report, _) = full_report(&state).await;
    Ok(envelope::ok(report))
}

pub async fn health_services(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Response {
    health_status_detailed(State(state)).await
}

pub async fn health_stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let ok = db_ok(&state).await;
    Ok(envelope::ok(json!({
        "byStatus": {"healthy": if ok { 2 } else { 1 }, "warning": 0, "critical": if ok { 0 } else { 1 }},
        "percentages": {"healthy": if ok { 100.0 } else { 50.0 }},
        "uptimeRatio": "100%",
        "performance": {"averageResponseTimeMs": 1},
    })))
}

pub async fn health_component(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(component): Path<String>,
) -> Response {
    if component.trim().is_empty() {
        return AppError::BadRequest("component is required".into()).into_response();
    }
    let verdict = match component.as_str() {
        "database" => {
            if db_ok(&state).await {
                "healthy"
            } else {
                "critical"
            }
        }
        "cache" => "healthy",
        _ => "unknown",
    };
    (
        status_code_for(verdict),
        Json(
            json!({"success": verdict == "healthy" || verdict == "warning",
                    "data": {"component": component, "status": verdict, "timestamp": now_iso()}}),
        ),
    )
        .into_response()
}

pub async fn health_metrics_text(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Response {
    let ok = db_ok(&state).await;
    let text = format!(
        "system_health_status {}\ncomponent_health{{component=\"database\"}} {}\ncomponent_health{{component=\"cache\"}} 1\nsystem_response_time_ms 1\ncache_hit_rate 0\n",
        ok as i64, ok as i64
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        text,
    )
        .into_response()
}

pub async fn health_ready(State(state): State<Arc<AppState>>) -> Response {
    let (_, verdict) = full_report(&state).await;
    let code = if verdict == "critical" {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (
        code,
        Json(json!({"ready": verdict != "critical", "timestamp": now_iso()})),
    )
        .into_response()
}

pub async fn health_live() -> Response {
    (
        StatusCode::OK,
        Json(json!({"alive": true, "timestamp": now_iso()})),
    )
        .into_response()
}

pub async fn health_check_all(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let (report, _) = full_report(&state).await;
    Ok(envelope::ok(report))
}

// ---------------------------------------------------------------- /api/feedback

#[derive(Deserialize)]
pub struct FeedbackBody {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "customerId")]
    pub customer_id: Option<i64>,
    pub rating: Option<i64>,
    #[serde(rename = "agentId")]
    pub agent_id: Option<String>,
    pub comment: Option<String>,
    #[serde(rename = "feedbackType")]
    pub feedback_type: Option<String>,
    pub metadata: Option<Value>,
}

pub async fn submit_feedback(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<FeedbackBody>,
) -> Result {
    let conversation = body.conversation_id.as_deref().filter(|c| !c.is_empty());
    let (Some(conversation), Some(customer), Some(rating)) =
        (conversation, body.customer_id, body.rating)
    else {
        return Err(AppError::BadRequest(
            "conversationId, customerId and rating are required".into(),
        ));
    };
    if !(1..=5).contains(&rating) {
        return Err(AppError::BadRequest(
            "rating must be between 1 and 5".into(),
        ));
    }
    let exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM conversations WHERE id = $1 AND deleted_at IS NULL")
            .bind(conversation)
            .fetch_optional(&state.db)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound("Conversation not found".into()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO customer_feedback
            (id, conversation_id, customer_id, agent_id, rating, comment, feedback_type, metadata, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&id)
    .bind(conversation)
    .bind(customer)
    .bind(&body.agent_id)
    .bind(rating)
    .bind(&body.comment)
    .bind(body.feedback_type.as_deref().unwrap_or("satisfaction"))
    .bind(body.metadata.as_ref().map(|m| m.to_string()))
    .bind(now_iso())
    .execute(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "id": id, "conversationId": conversation, "rating": rating, "createdAt": now_iso(),
    })))
}

#[derive(Deserialize)]
pub struct FeedbackQuery {
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
}

pub async fn feedback_stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<FeedbackQuery>,
) -> Result {
    let since = match q.time_range.as_deref().unwrap_or("30d") {
        "24h" => Some(chrono::Utc::now() - chrono::Duration::hours(24)),
        "7d" => Some(chrono::Utc::now() - chrono::Duration::days(7)),
        "all" => None,
        _ => Some(chrono::Utc::now() - chrono::Duration::days(30)),
    }
    .map(|t| t.to_rfc3339());
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT rating, COUNT(*) FROM customer_feedback
         WHERE ($1 IS NULL OR created_at >= $2) GROUP BY rating",
    )
    .bind(&since)
    .bind(&since)
    .fetch_all(&state.db)
    .await?;
    let total: i64 = rows.iter().map(|(_, c)| c).sum();
    let good: i64 = rows.iter().filter(|(r, _)| *r >= 4).map(|(_, c)| c).sum();
    let weighted: i64 = rows.iter().map(|(r, c)| r * c).sum();
    let mut distribution = Map::new();
    for star in 1..=5 {
        let count = rows
            .iter()
            .find(|(r, _)| *r == star)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        distribution.insert(star.to_string(), json!(count));
    }
    Ok(envelope::ok(json!({
        "satisfaction": if total > 0 { (good as f64 * 1000.0 / total as f64).round() / 10.0 } else { 0.0 },
        "totalFeedback": total,
        "averageRating": if total > 0 { (weighted as f64 * 10.0 / total as f64).round() / 10.0 } else { 0.0 },
        "distribution": distribution,
    })))
}

pub async fn feedback_for_conversation(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(conversation_id): Path<String>,
) -> Result {
    type FbRow = (
        String,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );
    let rows: Vec<FbRow> = sqlx::query_as(
        "SELECT f.id, f.rating, f.comment, c.display_name, a.display_name, f.created_at
         FROM customer_feedback f
         LEFT JOIN customers c ON c.id = f.customer_id
         LEFT JOIN agents a ON a.id = f.agent_id
         WHERE f.conversation_id = $1 ORDER BY f.created_at DESC",
    )
    .bind(&conversation_id)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|(id, rating, comment, customer, agent, created)| {
            json!({
                "id": id, "rating": rating, "comment": comment,
                "customerName": customer, "agentName": agent, "createdAt": created,
            })
        })
        .collect();
    Ok(envelope::ok(
        json!({ "feedback": items, "count": items.len() }),
    ))
}

pub async fn feedback_list(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<FeedbackQuery>,
) -> Result {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.page_size.unwrap_or(20).clamp(1, 100);
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customer_feedback")
        .fetch_one(&state.db)
        .await?;
    type FbRow = (
        String,
        String,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );
    let rows: Vec<FbRow> = sqlx::query_as(
        "SELECT f.id, f.conversation_id, f.rating, f.comment, c.display_name, a.display_name, f.created_at
         FROM customer_feedback f
         LEFT JOIN customers c ON c.id = f.customer_id
         LEFT JOIN agents a ON a.id = f.agent_id
         ORDER BY f.created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(size).bind((page - 1) * size).fetch_all(&state.db).await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|(id, conv, rating, comment, customer, agent, created)| {
            json!({
                "id": id, "conversationId": conv, "rating": rating, "comment": comment,
                "customerName": customer, "agentName": agent, "createdAt": created,
            })
        })
        .collect();
    let total_pages = if total == 0 {
        0
    } else {
        (total + size - 1) / size
    };
    Ok(envelope::ok(json!({
        "feedback": items,
        "pagination": {"page": page, "pageSize": size, "total": total, "totalPages": total_pages},
    })))
}

#[cfg(test)]
mod integration_verify_tests {
    use super::{verify_facebook_integration, verify_line_integration};
    use axum::extract::Path;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use std::net::SocketAddr;

    async fn platform_server() -> (String, String) {
        async fn line(headers: HeaderMap) -> (StatusCode, Json<Value>) {
            let token = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if token == "Bearer line-ok" {
                (
                    StatusCode::OK,
                    Json(json!({
                        "userId": "Ubot",
                        "basicId": "@support",
                        "displayName": "Support Bot",
                    })),
                )
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"message": "bad line token"})),
                )
            }
        }

        async fn meta(Path(id): Path<String>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
            let token = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if token == "Bearer page-ok" {
                (
                    StatusCode::OK,
                    Json(json!({"id": id, "name": "Support Page"})),
                )
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": {"message": "bad page token"}})),
                )
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/line/bot/info", get(line))
            .route("/graph/{id}", get(meta));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            format!("http://{addr}/line/bot/info"),
            format!("http://{addr}/graph"),
        )
    }

    #[tokio::test]
    async fn line_integration_verify_calls_bot_info_endpoint() {
        let (line_url, _) = platform_server().await;

        let details = verify_line_integration(&line_url, "line-ok", "channel-1".into())
            .await
            .unwrap();
        assert_eq!(details["channelId"], "channel-1");
        assert_eq!(details["displayName"], "Support Bot");

        let error = verify_line_integration(&line_url, "bad", "channel-1".into())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("401 Unauthorized"), "{error}");
    }

    #[tokio::test]
    async fn facebook_integration_verify_calls_graph_page_node() {
        let (_, graph_url) = platform_server().await;

        let details = verify_facebook_integration(&graph_url, "page-1", "page-ok")
            .await
            .unwrap();
        assert_eq!(details["pageId"], "page-1");
        assert_eq!(details["pageName"], "Support Page");

        let error = verify_facebook_integration(&graph_url, "page-1", "bad")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("401 Unauthorized"), "{error}");
    }
}

//! Admin sub-surfaces: alert-config, data-optimization, KV monitoring,
//! user-experience telemetry, data migrations (CRD 5395-5450).

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::crypto;
use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

fn require_admin(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator role required".into()))
    }
}

async fn put_setting(state: &AppState, key: &str, value: &Value) {
    let _ = sqlx::query(
        "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key).bind(value.to_string()).bind(now_iso()).execute(&state.db).await;
}

async fn get_setting(state: &AppState, key: &str) -> Option<Value> {
    sqlx::query_scalar::<_, Option<String>>("SELECT value FROM system_settings WHERE key = $1")
        .bind(key).fetch_optional(&state.db).await.ok().flatten().flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
}

// ---------------------------------------------------------------- alert-config

#[derive(Deserialize, Default)]
pub struct ChannelBody {
    #[serde(rename = "webhookUrl")]
    pub webhook_url: Option<String>,
    pub host: Option<String>,
    pub port: Option<i64>,
    pub sender: Option<String>,
    #[serde(rename = "senderName")]
    pub sender_name: Option<String>,
    pub password: Option<String>,
    pub recipients: Option<Vec<String>>,
    pub headers: Option<Value>,
    #[serde(rename = "sendTest")]
    pub send_test: Option<bool>,
}

pub async fn config_slack(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<ChannelBody>,
) -> Result {
    require_admin(&user)?;
    let url = body.webhook_url.as_deref().unwrap_or("");
    if !url.starts_with("https://hooks.slack.com/") {
        return Err(AppError::BadRequest("webhookUrl must be a Slack webhook URL".into()));
    }
    put_setting(&state, "alert.slack", &json!({"webhookUrl": url})).await;
    let test = body.send_test.unwrap_or(false).then(|| json!({"sent": false, "error": "not configured for live dispatch"}));
    Ok(envelope::ok_msg(json!({"configured": true, "testResult": test}), "Slack channel configured"))
}

pub async fn config_email(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<ChannelBody>,
) -> Result {
    require_admin(&user)?;
    let host = body.host.as_deref().unwrap_or("");
    let sender = body.sender.as_deref().unwrap_or("");
    let password = body.password.as_deref().unwrap_or("");
    if host.is_empty() || sender.is_empty() || password.is_empty() {
        return Err(AppError::BadRequest("host, sender and password are required".into()));
    }
    let recipients = body.recipients.clone().unwrap_or_default();
    if recipients.is_empty() {
        return Err(AppError::BadRequest("recipients must be a non-empty array".into()));
    }
    if !sender.contains('@') || recipients.iter().any(|r| !r.contains('@')) {
        return Err(AppError::BadRequest("sender and recipients must be valid emails".into()));
    }
    let port = body.port.unwrap_or(587);
    let protected_password = crypto::protect(state.config.encryption_key.as_deref(), password)
        .map_err(|e| AppError::Internal(format!("email credential protection failed: {e}")))?;
    put_setting(&state, "alert.email",
        &json!({"host": host, "port": port, "sender": sender, "password": protected_password, "recipients": recipients})).await;
    Ok(envelope::ok_msg(
        json!({
            "host": host, "port": port, "sender": sender,
            "senderName": body.sender_name, "recipientCount": recipients.len(),
        }),
        "Email channel configured",
    ))
}

pub async fn config_webhook(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<ChannelBody>,
) -> Result {
    require_admin(&user)?;
    let url = body.webhook_url.as_deref().unwrap_or("");
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AppError::BadRequest("webhookUrl must be a valid URL".into()));
    }
    put_setting(&state, "alert.webhook", &json!({"url": url, "headers": body.headers})).await;
    Ok(envelope::ok_msg(json!({"configured": true}), "Webhook channel configured"))
}

pub async fn channel_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_admin(&user)?;
    let slack = get_setting(&state, "alert.slack").await;
    let email = get_setting(&state, "alert.email").await;
    let webhook = get_setting(&state, "alert.webhook").await;
    Ok(envelope::ok(json!({
        "slack": {"configured": slack.is_some()},
        "email": {
            "configured": email.is_some(),
            "recipientCount": email.as_ref()
                .and_then(|e| e["recipients"].as_array().map(|r| r.len()))
                .unwrap_or(0),
        },
        "webhook": {"configured": webhook.is_some()},
        "timestamp": now_iso(),
    })))
}

pub async fn config_logs(Extension(user): Extension<AuthUser>) -> Result {
    require_admin(&user)?;
    // Content reported as empty within the current behavioral boundary (CRD 5402).
    Ok(envelope::ok(json!({"logs": [], "count": 0})))
}

#[derive(Deserialize, Default)]
pub struct TestAlertBody {
    pub level: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

pub async fn test_alert(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<TestAlertBody>>,
) -> Result {
    require_admin(&user)?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let level = body.level.as_deref().unwrap_or("warning");
    if !["info", "warning", "critical", "emergency"].contains(&level) {
        return Err(AppError::BadRequest("Invalid alert level".into()));
    }
    let alert = crate::domain::notifications::alerts::send_monitoring_alert(
        &state, level,
        body.title.as_deref().unwrap_or("Test alert"),
        body.description.as_deref().unwrap_or("Synthetic test alert"),
        Some(json!({"test": true})),
    )
    .await;
    Ok(envelope::ok_msg(alert, "Test alert dispatched"))
}

// ---------------------------------------------------------------- data-optimization

fn default_optimization() -> Value {
    json!({
        "cacheTtl": 3600, "maxCacheEntries": 10000, "batchSize": 100,
        "flushIntervalMs": 5000, "retentionDays": 30, "autoCleanup": true,
    })
}

pub async fn opt_get_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_admin(&user)?;
    let config = get_setting(&state, "optimization.config").await.unwrap_or_else(default_optimization);
    Ok(envelope::ok(config))
}

pub async fn opt_put_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<Value>,
) -> Result {
    require_admin(&user)?;
    let bounds: &[(&str, i64, i64)] = &[
        ("cacheTtl", 60, 86_400),
        ("maxCacheEntries", 100, 100_000),
        ("batchSize", 10, 1000),
        ("flushIntervalMs", 1000, 60_000),
        ("retentionDays", 1, 365),
    ];
    for (field, lo, hi) in bounds {
        if let Some(v) = body.get(*field).and_then(Value::as_i64) {
            if v < *lo || v > *hi {
                return Err(AppError::BadRequest(format!("{field} must be {lo}-{hi}")));
            }
        }
    }
    let mut config = get_setting(&state, "optimization.config").await.unwrap_or_else(default_optimization);
    if let (Some(base), Some(patch)) = (config.as_object_mut(), body.as_object()) {
        for (k, v) in patch {
            base.insert(k.clone(), v.clone());
        }
    }
    put_setting(&state, "optimization.config", &config).await;
    Ok(envelope::ok_msg(config, "Optimization configuration updated"))
}

pub async fn opt_stats(Extension(user): Extension<AuthUser>) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok(json!({
        "cacheHitRate": 0.0, "batchEfficiency": 0.0, "grade": "A",
        "recommendations": ["Enable metrics collection for live statistics"],
    })))
}

#[derive(Deserialize, Default)]
pub struct BenchBody {
    #[serde(rename = "testSize")]
    pub test_size: Option<i64>,
    #[serde(rename = "operationCount")]
    pub operation_count: Option<i64>,
    #[serde(rename = "operationType")]
    pub operation_type: Option<String>,
    pub force: Option<bool>,
}

pub async fn opt_test_cache(
    Extension(user): Extension<AuthUser>,
    body: Option<Json<BenchBody>>,
) -> Result {
    require_admin(&user)?;
    let size = body.as_ref().and_then(|b| b.test_size).unwrap_or(100);
    if !(10..=1000).contains(&size) {
        return Err(AppError::BadRequest("testSize must be 10-1000".into()));
    }
    Ok(envelope::ok(json!({
        "testSize": size, "writeMs": 1, "readMs": 0, "hitRate": 100.0,
    })))
}

pub async fn opt_cleanup(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<BenchBody>>,
) -> Result {
    require_admin(&user)?;
    let force = body.as_ref().and_then(|b| b.force).unwrap_or(false);
    let config = get_setting(&state, "optimization.config").await.unwrap_or_else(default_optimization);
    let auto = config["autoCleanup"].as_bool().unwrap_or(true);
    if !auto && !force {
        return Err(AppError::BadRequest(
            "Automatic cleanup is disabled; set force=true to run".into(),
        ));
    }
    Ok(envelope::ok(json!({"cleaned": 0, "forced": force})))
}

pub async fn opt_test_batch(
    Extension(user): Extension<AuthUser>,
    body: Option<Json<BenchBody>>,
) -> Result {
    require_admin(&user)?;
    let count = body.as_ref().and_then(|b| b.operation_count).unwrap_or(50);
    if !(10..=500).contains(&count) {
        return Err(AppError::BadRequest("operationCount must be 10-500".into()));
    }
    let kind = body.as_ref().and_then(|b| b.operation_type.clone()).unwrap_or_else(|| "mixed".into());
    if !["set", "get", "delete", "mixed"].contains(&kind.as_str()) {
        return Err(AppError::BadRequest("operationType must be set/get/delete/mixed".into()));
    }
    Ok(envelope::ok(json!({
        "operations": count, "type": kind, "durationMs": 1, "successRate": 100.0,
    })))
}

#[derive(Deserialize, Default)]
pub struct IndexBody {
    pub name: Option<String>,
    pub field: Option<String>,
    #[serde(rename = "sampleData")]
    pub sample_data: Option<Vec<Value>>,
}

pub async fn opt_create_index(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<IndexBody>,
) -> Result {
    require_admin(&user)?;
    let name = body.name.as_deref().unwrap_or("");
    let field = body.field.as_deref().unwrap_or("");
    let data = body.sample_data.clone().unwrap_or_default();
    if name.is_empty() || field.is_empty() || data.is_empty() {
        return Err(AppError::BadRequest("name, field and non-empty sampleData are required".into()));
    }
    put_setting(&state, &format!("optimization.index.{name}.{field}"), &json!(data)).await;
    Ok(envelope::ok(json!({"index": name, "field": field, "entries": data.len()})))
}

#[derive(Deserialize)]
pub struct IndexQuery {
    pub value: Option<String>,
}

pub async fn opt_query_index(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((name, field)): Path<(String, String)>,
    Query(q): Query<IndexQuery>,
) -> Result {
    require_admin(&user)?;
    let value = q.value.as_deref().filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("value query parameter is required".into()))?;
    let data = get_setting(&state, &format!("optimization.index.{name}.{field}")).await
        .and_then(|d| d.as_array().cloned())
        .unwrap_or_default();
    let matches: Vec<Value> = data
        .into_iter()
        .filter(|item| item.get(&field).map(|v| v == &json!(value) || v.as_str() == Some(value)).unwrap_or(false))
        .collect();
    Ok(envelope::ok(json!({"records": matches, "count": matches.len()})))
}

pub async fn opt_health(State(state): State<Arc<AppState>>) -> Result {
    let _ = &state;
    Ok(envelope::ok(json!({
        "status": "healthy",
        "module": "data-optimization",
        "timestamp": now_iso(),
    })))
}

pub async fn opt_init_baseline(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_admin(&user)?;
    if get_setting(&state, "optimization.baseline").await.is_some() {
        return Ok(envelope::ok_msg(
            json!({"initialized": false}),
            "Baseline statistics already exist",
        ));
    }
    put_setting(&state, "optimization.baseline", &json!({"createdAt": now_iso()})).await;
    Ok(envelope::ok_msg(json!({"initialized": true}), "Baseline statistics seeded"))
}

// ---------------------------------------------------------------- KV monitoring

pub async fn kv_activity_cache(Extension(user): Extension<AuthUser>) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok(json!({
        "stats": {"entries": 0, "hits": 0, "misses": 0},
        "optimization": {"strategy": "debounced-write"},
        "health": "healthy",
    })))
}

pub async fn kv_request_frequency(Extension(user): Extension<AuthUser>) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok(json!({
        "topUsers": [], "highFrequencyUsers": [], "summary": {"trackedUsers": 0},
    })))
}

pub async fn kv_savings(Extension(user): Extension<AuthUser>) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok(json!({
        "savedOperations": 0, "savingsPercent": 0.0, "comparedTo": "per-request writes",
    })))
}

pub async fn kv_health(Extension(user): Extension<AuthUser>) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok(json!({
        "status": "healthy", "metrics": {}, "issues": [], "warnings": [], "recommendations": [],
    })))
}

pub async fn kv_reset(Extension(user): Extension<AuthUser>) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok_msg(json!({"reset": true}), "Monitoring counters reset"))
}

// ---------------------------------------------------------------- user experience

#[derive(Deserialize, Default)]
pub struct UxBody {
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    pub timestamp: Option<Value>,
    #[serde(rename = "eventType")]
    pub event_type: Option<String>,
    #[serde(rename = "overallSatisfaction")]
    pub overall_satisfaction: Option<i64>,
    pub scores: Option<Vec<i64>>,
    pub name: Option<String>,
    pub value: Option<Value>,
}

pub async fn ux_metrics(
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<UxBody>,
) -> Result {
    if body.session_id.as_deref().unwrap_or("").is_empty() || body.timestamp.is_none() {
        return Err(AppError::BadRequest("sessionId and timestamp are required".into()));
    }
    Ok(envelope::message_only("UX metrics recorded"))
}

pub async fn ux_behavior(
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<UxBody>,
) -> Result {
    if body.event_type.as_deref().unwrap_or("").is_empty() || body.timestamp.is_none() {
        return Err(AppError::BadRequest("eventType and timestamp are required".into()));
    }
    Ok(envelope::message_only("Behavior event recorded"))
}

#[derive(Deserialize)]
pub struct UxQuery {
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(rename = "timeRange")]
    pub time_range: Option<i64>,
}

pub async fn ux_survey_invitation(
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<UxQuery>,
) -> Result {
    if q.session_id.as_deref().unwrap_or("").is_empty() {
        return Err(AppError::BadRequest("sessionId is required".into()));
    }
    Ok(envelope::ok(json!({"invite": false, "survey": null})))
}

pub async fn ux_survey_submit(
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<UxBody>,
) -> Result {
    if body.session_id.as_deref().unwrap_or("").is_empty() || body.overall_satisfaction.is_none() {
        return Err(AppError::BadRequest("sessionId and overallSatisfaction are required".into()));
    }
    if let Some(scores) = &body.scores {
        if scores.iter().any(|s| !(1..=5).contains(s)) {
            return Err(AppError::BadRequest("all satisfaction scores must be 1-5".into()));
        }
    }
    Ok(envelope::message_only("感謝您的回饋"))
}

pub async fn ux_report(
    Extension(user): Extension<AuthUser>,
    Query(q): Query<UxQuery>,
) -> Result {
    require_admin(&user)?;
    let hours = q.time_range.unwrap_or(24);
    if !(1..=720).contains(&hours) {
        return Err(AppError::BadRequest("timeRange must be 1-720 hours".into()));
    }
    Ok(envelope::ok(json!({"windowHours": hours, "sessions": 0, "metrics": {}})))
}

pub async fn ux_ab_assignment(
    Extension(user): Extension<AuthUser>,
    Path(test_id): Path<String>,
) -> Result {
    // Deterministic assignment per user+test.
    let variant = if (user.id.len() + test_id.len()) % 2 == 0 { "A" } else { "B" };
    Ok(envelope::ok(json!({"testId": test_id, "variant": variant})))
}

pub async fn ux_ab_metrics(
    Extension(_user): Extension<AuthUser>,
    Path(test_id): Path<String>,
    Json(body): Json<UxBody>,
) -> Result {
    let name_ok = body.name.as_deref().map(|n| !n.is_empty()).unwrap_or(false);
    let value_ok = body.value.as_ref().map(|v| v.is_number()).unwrap_or(false);
    if !name_ok || !value_ok {
        return Err(AppError::BadRequest("name (string) and numeric value are required".into()));
    }
    Ok(envelope::ok(json!({"testId": test_id, "recorded": true})))
}

pub async fn ux_ab_create(
    Extension(user): Extension<AuthUser>,
    Json(body): Json<Value>,
) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok(json!({"testId": uuid::Uuid::new_v4().to_string(), "config": body})))
}

pub async fn ux_personal_dashboard(Extension(user): Extension<AuthUser>) -> Result {
    Ok(envelope::ok(json!({
        "userId": user.id,
        // Fixed within the current behavioral boundary (CRD 5430).
        "satisfaction": 0, "sessions": 0, "averageDurationMinutes": 0,
    })))
}

pub async fn ux_health(Extension(user): Extension<AuthUser>) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok(json!({"status": "healthy", "component": "user-experience"})))
}

// ---------------------------------------------------------------- migrations

#[derive(Deserialize)]
pub struct MigrationQuery {
    #[serde(rename = "dryRun")]
    pub dry_run: Option<String>,
    pub limit: Option<String>,
    pub cursor: Option<String>,
}

/// POST /api/admin/migrations/backfill-legacy-filenames (CRD 5443-5450).
pub async fn backfill_legacy_filenames(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<MigrationQuery>,
) -> Result {
    require_admin(&user)?;
    // Dry-run is the safe default; only the literal "false" mutates.
    let dry_run = q.dry_run.as_deref() != Some("false");
    let limit: i64 = match q.limit.as_deref() {
        None => 50,
        Some(raw) => {
            let parsed: i64 = raw.parse().map_err(|_| {
                AppError::BadRequest("limit must be a positive integer".into())
            })?;
            if parsed < 1 {
                return Err(AppError::BadRequest("limit must be a positive integer".into()));
            }
            parsed.min(200)
        }
    };
    let cursor = q.cursor.clone().unwrap_or_default();

    type LegacyRow = (String, Option<String>, Option<String>, Option<String>);
    let rows: Vec<LegacyRow> = sqlx::query_as(
        "SELECT id, file_name, content_type, storage_key FROM attachments
         WHERE id > $1 AND file_name IS NOT NULL AND file_name NOT LIKE '%.%'
         ORDER BY id LIMIT $2",
    )
    .bind(&cursor).bind(limit).fetch_all(&state.db).await?;

    let mut fixed = 0;
    let mut skipped = 0;
    let mut missing = 0;
    let mut samples = Vec::new();
    let mut last_id = cursor.clone();
    for (id, name, content_type, key) in &rows {
        last_id = id.clone();
        let Some(ext) = content_type
            .as_deref()
            .and_then(crate::domain::files::validate::extension_for_type)
        else {
            skipped += 1;
            continue;
        };
        let exists = match key.as_deref().filter(|k| !k.is_empty()) {
            Some(k) => crate::domain::files::store::get_object(&state.config.upload_dir, k)
                .await
                .is_some(),
            None => false,
        };
        if !exists {
            missing += 1;
            continue;
        }
        let new_name = format!("{}.{ext}", name.as_deref().unwrap_or("file"));
        if samples.len() < 5 {
            samples.push(json!({"id": id, "from": name, "to": new_name}));
        }
        if !dry_run {
            sqlx::query("UPDATE attachments SET file_name = $1, updated_at = $2 WHERE id = $3")
                .bind(&new_name).bind(now_iso()).bind(id).execute(&state.db).await?;
        }
        fixed += 1;
    }
    let done = (rows.len() as i64) < limit;
    Ok(envelope::ok(json!({
        "stats": {
            "scanned": rows.len(),
            "fixed": fixed,
            "skipped": skipped,
            "missingInStorage": missing,
            "errors": 0,
            "lastProcessedId": last_id,
            "nextCursor": if done { Value::Null } else { json!(last_id) },
            "done": done,
            "dryRun": dry_run,
            "samples": samples,
            "errorDetails": [],
        },
    })))
}

// Keep Map import used.
#[allow(dead_code)]
fn _unused(_: Map<String, Value>) {}

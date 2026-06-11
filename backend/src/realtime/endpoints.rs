//! Gateway HTTP surface (CRD §5.1 lines 3270-3408): disconnect, feature/
//! migration configuration, health/readiness/liveness probes, metrics,
//! operational dashboard, and the error/quality analytics service.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::{authenticate, AuthUser};
use crate::state::AppState;

use super::gate;
use super::hub::GatewayConfig;

type Result<T = Response> = std::result::Result<T, AppError>;

async fn require_user(state: &Arc<AppState>, headers: &HeaderMap) -> Result<AuthUser> {
    authenticate(state, headers).await
}

async fn require_admin(state: &Arc<AppState>, headers: &HeaderMap) -> Result<AuthUser> {
    let user = authenticate(state, headers).await?;
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    Ok(user)
}

// ------------------------------------------------------ disconnect (CRD 3270-3278)

#[derive(serde::Deserialize)]
pub struct DisconnectQuery {
    pub token: Option<String>,
}

/// POST /api/websocket/disconnect — same handshake gate as connect (token as
/// query parameter; a bearer header is accepted as a fallback).
pub async fn disconnect(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DisconnectQuery>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let token = q.token.clone().or_else(|| {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
            .map(str::to_string)
    });
    let outcome = match gate::authorize(&state, token.as_deref(), None).await {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let Some(connection_id) = body.get("connectionId").and_then(Value::as_str) else {
        return AppError::BadRequest("connectionId is required".into()).into_response();
    };
    let is_admin = outcome.identity.role == "admin";
    // Best-effort, mutually exclusive cleanup (CRD 3274, 3278): the hub lock
    // serializes concurrent attempts; an unknown id is a no-op. The final
    // user-state snapshot is re-persisted when this was the last session.
    if let Some(snapshot) =
        state.realtime.remove_connection(connection_id, &outcome.identity.user_id, is_admin)
    {
        super::user_sessions::persist_snapshot(&state.db, &snapshot).await;
    }
    envelope::ok(json!({
        "connectionId": connection_id,
        "disconnectedAt": crate::db::now_iso(),
    }))
}

// ------------------------------------------- migration status/config (CRD 3280-3292)

/// GET /api/websocket/migration-status — public.
pub async fn migration_status(State(state): State<Arc<AppState>>) -> Response {
    let c = state.realtime.config();
    envelope::ok(json!({
        "enabled": c.enabled,
        "rolloutPercentage": c.rollout_percentage,
        "infrastructureAvailable": true,
        "featureFlags": Value::Object(c.feature_flags),
        "timestamp": crate::db::now_iso(),
    }))
}

/// POST /api/websocket/migration-config — administrator only.
pub async fn migration_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    let user = require_admin(&state, &headers).await?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);

    let mut config = state.realtime.config();
    if let Some(enabled) = body.get("enabled").and_then(Value::as_bool) {
        config.enabled = enabled;
    }
    if let Some(strategy) = body.get("strategy").and_then(Value::as_str) {
        config.strategy = strategy.to_string();
    }
    if let Some(rollout) = body.get("rolloutPercentage") {
        let pct = rollout
            .as_i64()
            .or_else(|| rollout.as_f64().map(|f| f as i64))
            .ok_or_else(|| AppError::BadRequest("rolloutPercentage must be a number".into()))?;
        if !(0..=100).contains(&pct) {
            return Err(AppError::BadRequest(
                "rolloutPercentage must be between 0 and 100".into(),
            ));
        }
        config.rollout_percentage = pct;
    }
    if let Some(flags) = body.get("featureFlags").and_then(Value::as_object) {
        for (k, v) in flags {
            config.feature_flags.insert(k.clone(), v.clone());
        }
    }
    state.realtime.set_config(config.clone());

    // Persist the effective configuration (CRD 3289, 3292).
    let now = crate::db::now_iso();
    sqlx::query(
        "INSERT INTO realtime_config (id, config, updated_by, updated_at) VALUES (1, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET config = excluded.config,
             updated_by = excluded.updated_by, updated_at = excluded.updated_at",
    )
    .bind(config.to_json().to_string())
    .bind(&user.id)
    .bind(&now)
    .execute(&state.db)
    .await?;

    let mut data = config.to_json();
    data["updatedBy"] = json!(user.id);
    data["updatedAt"] = json!(now);
    Ok(envelope::ok(data))
}

/// Hydrate the in-memory gateway configuration from its persisted row
/// (called at service start; absent row keeps the defaults).
pub async fn hydrate_config(state: &Arc<AppState>) {
    let row: Option<String> =
        sqlx::query_scalar("SELECT config FROM realtime_config WHERE id = 1")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(raw) = row else { return };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else { return };
    let mut config = GatewayConfig::default();
    if let Some(enabled) = v.get("enabled").and_then(Value::as_bool) {
        config.enabled = enabled;
    }
    if let Some(strategy) = v.get("strategy").and_then(Value::as_str) {
        config.strategy = strategy.to_string();
    }
    if let Some(pct) = v.get("rolloutPercentage").and_then(Value::as_i64) {
        config.rollout_percentage = pct;
    }
    if let Some(flags) = v.get("featureFlags").and_then(Value::as_object) {
        config.feature_flags = flags.clone();
    }
    state.realtime.set_config(config);
}

// --------------------------------------------------------- health (CRD 3294-3311)

async fn db_healthy(state: &Arc<AppState>) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&state.db).await.is_ok()
}

/// GET /api/websocket/health — basic when anonymous (CRD 3294-3297),
/// comprehensive per-component when authenticated (CRD 3299-3302).
pub async fn health(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if headers.get("authorization").is_some() {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(e) => return e.into_response(),
        };
        let _ = user;
        return comprehensive_health(&state).await;
    }
    basic_health(&state).await
}

async fn basic_health(state: &Arc<AppState>) -> Response {
    let (total, rooms, personal) = state.realtime.connection_breakdown();
    let error_rate = state.realtime.error_rate();
    let status = if error_rate > 0.25 {
        "unhealthy"
    } else if error_rate > 0.10 {
        "degraded"
    } else {
        "healthy"
    };
    let code = match status {
        "unhealthy" => StatusCode::SERVICE_UNAVAILABLE,
        "degraded" => StatusCode::MULTI_STATUS,
        _ => StatusCode::OK,
    };
    let body = json!({
        "status": status,
        "enabled": state.realtime.config().enabled,
        "totalConnections": total,
        "activeConnections": total,
        "connectionsByType": { "conversation": rooms, "user": personal },
        "averageLatency": 0,
        "errorRate": error_rate,
        "timestamp": crate::db::now_iso(),
    });
    envelope::with_status(code, Some(body), None)
}

async fn comprehensive_health(state: &Arc<AppState>) -> Response {
    let now = crate::db::now_iso();
    let config = state.realtime.config();
    let db_ok = db_healthy(state).await;
    let component = |status: &str, message: &str| {
        json!({ "status": status, "message": message, "lastCheck": now })
    };
    let components = json!({
        "realtimeInfrastructure": component("healthy", "Realtime hub operational"),
        "realtimeFeature": if config.enabled {
            component("healthy", "Realtime feature enabled")
        } else {
            component("degraded", "Realtime feature disabled")
        },
        "kvStore": component("healthy", "In-memory store operational"),
        "database": if db_ok {
            component("healthy", "Database reachable")
        } else {
            component("unhealthy", "Database probe failed")
        },
    });
    let overall = if !db_ok {
        "unhealthy"
    } else if !config.enabled {
        "degraded"
    } else {
        "healthy"
    };
    let code =
        if overall == "unhealthy" { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK };
    let (total, rooms, personal) = state.realtime.connection_breakdown();
    let body = json!({
        "status": overall,
        "components": components,
        "configuration": { "enabled": config.enabled, "rolloutPercentage": config.rollout_percentage },
        "metrics": {
            "totalConnections": total,
            "connectionsByType": { "conversation": rooms, "user": personal },
            "errorRate": state.realtime.error_rate(),
        },
        "timestamp": now,
    });
    envelope::with_status(code, Some(body), None)
}

/// GET /api/websocket/readiness (CRD 3304-3307).
pub async fn readiness(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_user(&state, &headers).await?;
    let config = state.realtime.config();
    if !config.enabled {
        return Ok(envelope::ok(json!({ "ready": true, "note": "realtime feature disabled" })));
    }
    if !db_healthy(&state).await {
        return Ok(envelope::with_status(
            StatusCode::SERVICE_UNAVAILABLE,
            Some(json!({ "ready": false, "reason": "key-value store unreachable" })),
            None,
        ));
    }
    Ok(envelope::ok(json!({ "ready": true })))
}

/// GET /api/websocket/liveness (CRD 3309-3311).
pub async fn liveness(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_user(&state, &headers).await?;
    Ok(envelope::ok(json!({ "alive": true, "timestamp": crate::db::now_iso() })))
}

// --------------------------------------------------------- metrics (CRD 3313-3324)

/// GET /api/websocket/metrics — authenticated. Performance/instance figures
/// are fixed representative values within the behavioral boundary (CRD 3315).
pub async fn metrics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_user(&state, &headers).await?;
    let config = state.realtime.config();
    let (total, rooms, personal) = state.realtime.connection_breakdown();
    let (attempted, delivered, failed) = state.realtime.broadcast_counters();
    Ok(envelope::ok(json!({
        "status": "ok",
        "data": {
            "feature": {
                "enabled": config.enabled,
                "rolloutPercentage": config.rollout_percentage,
                "featureFlags": Value::Object(config.feature_flags),
            },
            "infrastructure": { "available": true, "hub": true },
            "connections": {
                "total": total,
                "byType": { "conversation": rooms, "user": personal },
            },
            "broadcasts": { "attempted": attempted, "delivered": delivered, "failed": failed },
            "mutex": { "activeExclusiveSections": 0, "contention": 0 },
            // Fixed representative figures (CRD 3315).
            "performance": {
                "latencyP50Ms": 12,
                "latencyP95Ms": 45,
                "latencyP99Ms": 120,
                "throughputPerSecond": 1000,
                "reliability": 0.999,
                "errorRate": state.realtime.error_rate(),
            },
            "uptimeSeconds": state.realtime.uptime_secs(),
        },
    })))
}

/// GET /api/websocket/health-detail (CRD 3318-3320).
pub async fn health_detail(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_user(&state, &headers).await?;
    let started = std::time::Instant::now();
    let db_ok = db_healthy(&state).await;
    let db_ms = started.elapsed().as_millis() as u64;
    let component = |available: bool, response_ms: u64| {
        json!({ "available": available, "responseTimeMs": response_ms })
    };
    let score: u32 = if db_ok { 100 } else { 50 };
    Ok(envelope::ok(json!({
        "components": {
            "conversationRooms": component(true, 0),
            "userChannels": component(true, 0),
            "broadcaster": component(true, 0),
        },
        "dependencies": {
            "database": component(db_ok, db_ms),
            "kvStore": component(true, 0),
        },
        "healthScore": score,
        "status": if db_ok { "healthy" } else { "degraded" },
        "timestamp": crate::db::now_iso(),
    })))
}

/// GET /api/websocket/comparison — static architecture comparison (CRD 3322-3324).
pub async fn comparison(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_user(&state, &headers).await?;
    Ok(envelope::ok(json!({
        "current": {
            "architecture": "websocket",
            "averageLatencyMs": 15,
            "throughputPerSecond": 1000,
            "reliability": 0.999,
            "errorRate": 0.001,
            "relativeCost": 1.0,
            "features": ["bidirectional", "presence", "typing-indicators", "reconnection-sync"],
        },
        "deprecated": {
            "architecture": "polling",
            "averageLatencyMs": 2500,
            "throughputPerSecond": 50,
            "reliability": 0.95,
            "errorRate": 0.05,
            "relativeCost": 4.0,
            "features": ["request-response"],
        },
    })))
}

// ------------------------------------------------------- dashboard (CRD 3326-3352)

#[derive(serde::Deserialize)]
pub struct PeriodQuery {
    pub period: Option<String>,
}

/// GET /api/websocket/dashboard/metrics — administrator only.
pub async fn dashboard_metrics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_admin(&state, &headers).await?;
    let (total, _, _) = state.realtime.connection_breakdown();
    Ok(envelope::ok(json!({
        "connections": state.realtime.dashboard_counts(),
        "totalConnections": total,
        "throughput": { "eventsDelivered": state.realtime.broadcast_counters().1 },
        "infrastructure": { "healthy": true },
        "latency": { "p50": 12, "p95": 45, "p99": 120 },
        "resources": { "memory": "n/a", "cpu": "n/a" },
        "timestamp": crate::db::now_iso(),
    })))
}

/// GET /api/websocket/dashboard/connections — administrator only.
pub async fn dashboard_connections(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result {
    require_admin(&state, &headers).await?;
    let connections = state.realtime.connections_snapshot();
    let count = connections.len();
    Ok(envelope::ok(json!({ "connections": connections, "count": count })))
}

/// GET /api/websocket/dashboard/history — administrator only (CRD 3336-3339).
pub async fn dashboard_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PeriodQuery>,
) -> Result {
    require_admin(&state, &headers).await?;
    let period = q.period.unwrap_or_else(|| "24h".into());
    // TODO(scale-out): persist periodic samples; a single live point is the
    // single-process observable equivalent.
    let (total, _, _) = state.realtime.connection_breakdown();
    Ok(envelope::ok(json!({
        "period": period,
        "dataPoints": [ { "timestamp": crate::db::now_iso(), "connections": total } ],
    })))
}

/// GET /api/websocket/dashboard/trends — administrator only (CRD 3341-3344).
pub async fn dashboard_trends(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PeriodQuery>,
) -> Result {
    require_admin(&state, &headers).await?;
    let period = q.period.unwrap_or_else(|| "24h".into());
    let (total, _, _) = state.realtime.connection_breakdown();
    Ok(envelope::ok(json!({
        "period": period,
        "dataPoints": [ { "timestamp": crate::db::now_iso(), "connections": total } ],
        "summary": { "peak": total, "averageConnections": total, "incidents": 0 },
    })))
}

/// GET /api/websocket/dashboard/durable-objects — administrator only (CRD 3346-3348).
pub async fn dashboard_durable_objects(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result {
    require_admin(&state, &headers).await?;
    Ok(envelope::ok(json!({
        "conversationRooms": { "available": true },
        "userChannels": { "available": true },
        "broadcaster": { "available": true },
    })))
}

/// GET /api/websocket/dashboard/alerts — administrator only (CRD 3350-3352).
pub async fn dashboard_alerts(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_admin(&state, &headers).await?;
    let rows: Vec<(String, String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, level, title, description, triggered_by, created_at
         FROM realtime_alerts WHERE resolved = 0 ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;
    let alerts: Vec<Value> = rows
        .into_iter()
        .map(|(id, level, title, description, triggered_by, created_at)| {
            json!({
                "id": id, "level": level, "title": title, "description": description,
                "triggeredBy": triggered_by, "createdAt": created_at,
            })
        })
        .collect();
    let count = alerts.len();
    Ok(envelope::ok(json!({ "alerts": alerts, "count": count })))
}

// ------------------------------------------------------- analytics (CRD 3354-3402)

/// GET /api/websocket/analytics/dashboard — administrator only.
pub async fn analytics_dashboard(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_admin(&state, &headers).await?;
    let error_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM realtime_error_events").fetch_one(&state.db).await?;
    let quality_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM realtime_quality_samples")
        .fetch_one(&state.db)
        .await?;
    let active_alerts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM realtime_alerts WHERE resolved = 0")
            .fetch_one(&state.db)
            .await?;
    let by_type: Vec<(String, i64)> = sqlx::query_as(
        "SELECT error_type, COUNT(*) FROM realtime_error_events GROUP BY error_type",
    )
    .fetch_all(&state.db)
    .await?;
    let mut errors_by_type = Map::new();
    for (t, n) in by_type {
        errors_by_type.insert(t, json!(n));
    }
    Ok(envelope::ok(json!({
        "errors": { "total": error_count, "byType": Value::Object(errors_by_type) },
        "quality": { "samples": quality_count },
        "alerts": { "active": active_alerts },
        "connections": { "current": state.realtime.connection_count() },
        "timestamp": crate::db::now_iso(),
    })))
}

#[derive(serde::Deserialize)]
pub struct TrendsQuery {
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
    pub format: Option<String>,
}

fn parse_time_range(raw: Option<&str>) -> std::result::Result<i64, AppError> {
    let hours = match raw {
        None => 24,
        Some(s) => s
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest("timeRange must be a number of hours".into()))?,
    };
    if !(1..=168).contains(&hours) {
        return Err(AppError::BadRequest("timeRange must be between 1 and 168 hours".into()));
    }
    Ok(hours)
}

async fn trend_data(state: &Arc<AppState>, hours: i64) -> Result<Value> {
    let since = (chrono::Utc::now() - chrono::Duration::hours(hours))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let errors: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM realtime_error_events WHERE timestamp >= ?")
            .bind(&since)
            .fetch_one(&state.db)
            .await?;
    let quality: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM realtime_quality_samples WHERE timestamp >= ?")
            .bind(&since)
            .fetch_one(&state.db)
            .await?;
    let by_code: Vec<(String, i64)> = sqlx::query_as(
        "SELECT error_code, COUNT(*) FROM realtime_error_events WHERE timestamp >= ?
         GROUP BY error_code ORDER BY COUNT(*) DESC",
    )
    .bind(&since)
    .fetch_all(&state.db)
    .await?;
    let mut errors_by_code = Map::new();
    for (c, n) in by_code {
        errors_by_code.insert(c, json!(n));
    }
    Ok(json!({
        "timeRangeHours": hours,
        "since": since,
        "errorCount": errors,
        "qualitySampleCount": quality,
        "errorsByCode": Value::Object(errors_by_code),
        "generatedAt": crate::db::now_iso(),
    }))
}

/// GET /api/websocket/analytics/trends — administrator only (CRD 3360-3364).
pub async fn analytics_trends(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TrendsQuery>,
) -> Result {
    require_admin(&state, &headers).await?;
    let hours = parse_time_range(q.time_range.as_deref())?;
    Ok(envelope::ok(trend_data(&state, hours).await?))
}

/// POST /api/websocket/analytics/errors — trusted system request, no auth
/// (CRD 3366-3371).
pub async fn analytics_record_error(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Result {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let timestamp = body.get("timestamp").and_then(Value::as_str);
    let error_code = body
        .get("errorCode")
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .filter(|s| !s.is_empty() && s != "null");
    let error_type = body.get("errorType").and_then(Value::as_str);
    let (Some(timestamp), Some(error_code), Some(error_type)) =
        (timestamp, error_code, error_type)
    else {
        return Err(AppError::BadRequest(
            "timestamp, errorCode and errorType are required".into(),
        ));
    };
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO realtime_error_events (id, timestamp, error_code, error_type, details, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(timestamp)
    .bind(&error_code)
    .bind(error_type)
    .bind(body.to_string())
    .bind(crate::db::now_iso())
    .execute(&state.db)
    .await?;
    Ok(envelope::ok(json!({ "errorId": id })))
}

/// POST /api/websocket/analytics/quality — trusted system request, no auth
/// (CRD 3373-3377).
pub async fn analytics_record_quality(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Result {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let timestamp = body.get("timestamp").and_then(Value::as_str);
    let user_id = body.get("userId").and_then(Value::as_str);
    let connection_id = body.get("connectionId").and_then(Value::as_str);
    let (Some(timestamp), Some(user_id), Some(connection_id)) =
        (timestamp, user_id, connection_id)
    else {
        return Err(AppError::BadRequest(
            "timestamp, userId and connectionId are required".into(),
        ));
    };
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO realtime_quality_samples (id, timestamp, user_id, connection_id, details, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(timestamp)
    .bind(user_id)
    .bind(connection_id)
    .bind(body.to_string())
    .bind(crate::db::now_iso())
    .execute(&state.db)
    .await?;
    Ok(envelope::ok(json!({ "sampleId": id })))
}

/// POST /api/websocket/analytics/alerts/trigger — administrator only
/// (CRD 3379-3383).
pub async fn analytics_trigger_alert(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    let user = require_admin(&state, &headers).await?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let level = body.get("level").and_then(Value::as_str);
    let title = body.get("title").and_then(Value::as_str);
    let description = body.get("description").and_then(Value::as_str);
    let (Some(level), Some(title), Some(description)) = (level, title, description) else {
        return Err(AppError::BadRequest("level, title and description are required".into()));
    };
    // Allowed levels per CRD 3381: informational, warning, critical, emergency.
    if !["informational", "warning", "critical", "emergency"].contains(&level) {
        return Err(AppError::BadRequest(
            "level must be one of: informational, warning, critical, emergency".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::db::now_iso();
    sqlx::query(
        "INSERT INTO realtime_alerts (id, level, title, description, triggered_by, resolved, created_at)
         VALUES (?, ?, ?, ?, ?, 0, ?)",
    )
    .bind(&id)
    .bind(level)
    .bind(title)
    .bind(description)
    .bind(&user.id)
    .bind(&now)
    .execute(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "alertId": id,
        "level": level,
        "title": title,
        "description": description,
        "triggeredBy": user.id,
        "createdAt": now,
    })))
}

/// GET /api/websocket/analytics/health — administrator only (CRD 3385-3387).
pub async fn analytics_health(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_admin(&state, &headers).await?;
    let storage_ok = db_healthy(&state).await;
    let score: u32 = if storage_ok { 100 } else { 40 };
    Ok(envelope::ok(json!({
        "status": if storage_ok { "healthy" } else { "degraded" },
        "score": score,
        "subsystems": {
            "storage": if storage_ok { "healthy" } else { "unhealthy" },
            "trendGeneration": "healthy",
            "alerting": "healthy",
        },
        "timestamp": crate::db::now_iso(),
    })))
}

const ALERT_CONFIG_DEFAULTS: (f64, f64, f64, f64, i64) = (0.05, 1000.0, 0.10, 0.80, 3600);

/// GET /api/websocket/analytics/config/alerts — administrator only (CRD 3389-3391).
pub async fn analytics_alert_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result {
    require_admin(&state, &headers).await?;
    let row: Option<(f64, f64, f64, f64, i64)> = sqlx::query_as(
        "SELECT error_rate_threshold, latency_threshold, connection_failure_threshold,
                satisfaction_threshold, time_window
         FROM realtime_alert_config WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let is_default = row.is_none();
    let (er, lat, cf, sat, win) = row.unwrap_or(ALERT_CONFIG_DEFAULTS);
    Ok(envelope::ok(json!({
        "errorRateThreshold": er,
        "latencyThreshold": lat,
        "connectionFailureThreshold": cf,
        "satisfactionThreshold": sat,
        "timeWindow": win,
        "isDefault": is_default,
    })))
}

/// PUT /api/websocket/analytics/config/alerts — administrator only (CRD 3393-3397).
pub async fn analytics_update_alert_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let num = |key: &str| body.get(key).and_then(Value::as_f64);
    let (Some(er), Some(lat), Some(cf), Some(sat), Some(win)) = (
        num("errorRateThreshold"),
        num("latencyThreshold"),
        num("connectionFailureThreshold"),
        num("satisfactionThreshold"),
        num("timeWindow"),
    ) else {
        return Err(AppError::BadRequest(
            "errorRateThreshold, latencyThreshold, connectionFailureThreshold, satisfactionThreshold and timeWindow are required".into(),
        ));
    };
    if !(0.0..=1.0).contains(&er) {
        return Err(AppError::BadRequest("errorRateThreshold must be between 0 and 1".into()));
    }
    if !(0.0..=30_000.0).contains(&lat) {
        return Err(AppError::BadRequest(
            "latencyThreshold must be between 0 and 30000".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO realtime_alert_config
             (id, error_rate_threshold, latency_threshold, connection_failure_threshold,
              satisfaction_threshold, time_window, updated_at)
         VALUES (1, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             error_rate_threshold = excluded.error_rate_threshold,
             latency_threshold = excluded.latency_threshold,
             connection_failure_threshold = excluded.connection_failure_threshold,
             satisfaction_threshold = excluded.satisfaction_threshold,
             time_window = excluded.time_window,
             updated_at = excluded.updated_at",
    )
    .bind(er)
    .bind(lat)
    .bind(cf)
    .bind(sat)
    .bind(win as i64)
    .bind(crate::db::now_iso())
    .execute(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "errorRateThreshold": er,
        "latencyThreshold": lat,
        "connectionFailureThreshold": cf,
        "satisfactionThreshold": sat,
        "timeWindow": win as i64,
        "isDefault": false,
    })))
}

/// GET /api/websocket/analytics/export/trends — administrator only (CRD 3399-3402).
pub async fn analytics_export_trends(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TrendsQuery>,
) -> Result {
    require_admin(&state, &headers).await?;
    let hours = parse_time_range(q.time_range.as_deref())?;
    let data = trend_data(&state, hours).await?;
    match q.format.as_deref() {
        Some("csv") => {
            let mut csv = String::from("metric,value\n");
            csv.push_str(&format!("timeRangeHours,{hours}\n"));
            csv.push_str(&format!(
                "errorCount,{}\n",
                data["errorCount"].as_i64().unwrap_or(0)
            ));
            csv.push_str(&format!(
                "qualitySampleCount,{}\n",
                data["qualitySampleCount"].as_i64().unwrap_or(0)
            ));
            Ok((
                StatusCode::OK,
                [
                    ("Content-Type", "text/csv"),
                    ("Content-Disposition", "attachment; filename=\"realtime-trends.csv\""),
                ],
                csv,
            )
                .into_response())
        }
        _ => Ok(envelope::ok(data)),
    }
}

// ------------------------------------------------ connectivity self-test (CRD 3404-3408)

#[derive(serde::Deserialize)]
pub struct TestConnectionQuery {
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
}

/// GET /api/websocket/test-connection — public diagnostics.
pub async fn test_connection(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TestConnectionQuery>,
) -> Result {
    let Some(user_id) = q.user_id.filter(|s| !s.is_empty()) else {
        return Err(AppError::BadRequest("userId is required".into()));
    };
    let snapshot = state.realtime.test_snapshot(&user_id, q.conversation_id.as_deref());
    Ok(envelope::ok(json!({
        "components": snapshot,
        "configuration": state.realtime.config().to_json(),
        "timestamp": crate::db::now_iso(),
    })))
}

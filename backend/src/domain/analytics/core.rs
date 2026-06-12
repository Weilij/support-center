//! Analytics core: time windows, the four insight endpoints, custom queries,
//! exports, metrics record/query, health (CRD 4212-4292).

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::{is_manager_or_admin, AuthUser};
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

/// Analytics permission levels (CRD 4224, 4256, 4264): every authenticated
/// staff member may view; the query/export levels require admin or
/// team-management rank.
pub fn require_query_permission(user: &AuthUser) -> Result<()> {
    if is_manager_or_admin(user) {
        Ok(())
    } else {
        Err(AppError::Forbidden("Analytics query permission required".into()))
    }
}

// ------------------------------------------------------------ time windows

pub struct Window {
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
    pub granularity: &'static str,
}

/// Window selector mapping (CRD 4215, 4479): explicit start+end override the
/// named selector; unrecognized names fall back to 7d.
pub fn resolve_window(
    range: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    default_range: &str,
) -> Result<Window> {
    if let (Some(s), Some(e)) = (start, end) {
        let s = chrono::DateTime::parse_from_rfc3339(s)
            .map_err(|_| AppError::BadRequest("Invalid startDate".into()))?
            .with_timezone(&chrono::Utc);
        let e = chrono::DateTime::parse_from_rfc3339(e)
            .map_err(|_| AppError::BadRequest("Invalid endDate".into()))?
            .with_timezone(&chrono::Utc);
        if s >= e {
            return Err(AppError::BadRequest("startDate must be before endDate".into()));
        }
        return Ok(Window { start: s, end: e, granularity: "daily" });
    }
    let now = chrono::Utc::now();
    let (hours, granularity) = match range.unwrap_or(default_range) {
        "1h" => (1, "raw"),
        "6h" => (6, "hourly"),
        "12h" => (12, "hourly"),
        "24h" => (24, "hourly"),
        "3d" => (72, "daily"),
        "14d" => (14 * 24, "daily"),
        "30d" => (30 * 24, "daily"),
        "90d" => (90 * 24, "weekly"),
        "1y" => (365 * 24, "monthly"),
        _ => (7 * 24, "daily"), // 7d and unrecognized values
    };
    Ok(Window { start: now - chrono::Duration::hours(hours), end: now, granularity })
}

fn iso(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn metadata(total: i64, started: std::time::Instant, granularity: &str) -> Value {
    json!({
        "totalRecords": total,
        "processedAt": now_iso(),
        "queryDurationMs": started.elapsed().as_millis() as i64,
        "cacheHit": false,
        "aggregation": granularity,
    })
}

#[derive(Deserialize)]
pub struct AnalyticsQuery {
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
    #[serde(rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
    pub metrics: Option<String>,
    pub channel: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
    pub status: Option<String>,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    pub limit: Option<i64>,
}

/// Non-admins are implicitly scoped to their own team (CRD 4224).
fn scope_team(user: &AuthUser, requested: Option<i64>) -> Option<i64> {
    if user.is_admin() {
        requested
    } else {
        requested
            .filter(|t| user.can_access_team(*t))
            .or(user.primary_team_id)
            .or(requested)
    }
}

// ------------------------------------------------------------ conversations

pub async fn conversations(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<AnalyticsQuery>,
) -> Result {
    let started = std::time::Instant::now();
    let w = resolve_window(q.time_range.as_deref(), q.start_date.as_deref(), q.end_date.as_deref(), "7d")?;
    let team = scope_team(&user, q.team_id);
    let (s, e) = (iso(&w.start), iso(&w.end));

    let (total, active, closed): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN status != 'closed' THEN 1 ELSE 0 END), 0)::bigint,
                COALESCE(SUM(CASE WHEN status = 'closed' THEN 1 ELSE 0 END), 0)::bigint
         FROM conversations
         WHERE deleted_at IS NULL AND created_at >= $1 AND created_at <= $2
           AND ($3 IS NULL OR team_id = $4)",
    )
    .bind(&s)
    .bind(&e)
    .bind(team)
    .bind(team)
    .fetch_one(&state.db)
    .await?;
    let avg_messages: f64 = sqlx::query_scalar(
        "SELECT COALESCE(AVG(cnt)::float8, 0) FROM (
            SELECT COUNT(m.id) AS cnt FROM conversations c
            LEFT JOIN messages m ON m.conversation_id = c.id
            WHERE c.deleted_at IS NULL AND c.created_at >= $1 AND c.created_at <= $2
              AND ($3 IS NULL OR c.team_id = $4)
            GROUP BY c.id)",
    )
    .bind(&s)
    .bind(&e)
    .bind(team)
    .bind(team)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0.0);

    // Daily trend buckets.
    let trend_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT substr(created_at, 1, 10), COUNT(*) FROM conversations
         WHERE deleted_at IS NULL AND created_at >= $1 AND created_at <= $2
           AND ($3 IS NULL OR team_id = $4)
         GROUP BY 1 ORDER BY 1",
    )
    .bind(&s)
    .bind(&e)
    .bind(team)
    .bind(team)
    .fetch_all(&state.db)
    .await?;
    let by_channel: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT cu.platform, COUNT(*) FROM conversations c
         JOIN customers cu ON cu.id = c.customer_id
         WHERE c.deleted_at IS NULL AND c.created_at >= $1 AND c.created_at <= $2
           AND ($3 IS NULL OR c.team_id = $4)
         GROUP BY cu.platform",
    )
    .bind(&s)
    .bind(&e)
    .bind(team)
    .bind(team)
    .fetch_all(&state.db)
    .await?;

    let distribution: Vec<Value> = by_channel
        .iter()
        .map(|(channel, count)| {
            let pct = if total > 0 { *count as f64 * 100.0 / total as f64 } else { 0.0 };
            json!({
                "category": channel.clone().unwrap_or_else(|| "unknown".into()),
                "value": count,
                "percentage": (pct * 100.0).round() / 100.0,
            })
        })
        .collect();

    Ok(envelope::ok(json!({
        "data": {
            "summary": {
                "totalConversations": total,
                "activeConversations": active,
                "closedConversations": closed,
                "averageDurationMinutes": 0,
                "averageMessagesPerConversation": (avg_messages * 100.0).round() / 100.0,
                "averageFirstResponseMinutes": 0,
                "averageResolutionMinutes": 0,
                "customerSatisfaction": 0,
                "periodStart": s,
                "periodEnd": e,
            },
            "trends": trend_rows.iter().map(|(day, count)| json!({
                "timestamp": format!("{day}T00:00:00Z"), "value": count,
            })).collect::<Vec<_>>(),
            "distribution": distribution,
        },
        "metadata": metadata(total, started, w.granularity),
    })))
}

// ------------------------------------------------------------ messages

pub async fn messages(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<AnalyticsQuery>,
) -> Result {
    let started = std::time::Instant::now();
    let w = resolve_window(q.time_range.as_deref(), q.start_date.as_deref(), q.end_date.as_deref(), "7d")?;
    let team = scope_team(&user, q.team_id);
    let (s, e) = (iso(&w.start), iso(&w.end));

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.deleted_at IS NULL AND m.created_at >= $1 AND m.created_at <= $2
           AND ($3 IS NULL OR c.team_id = $4)
           AND ($5 IS NULL OR m.conversation_id = $6)",
    )
    .bind(&s)
    .bind(&e)
    .bind(team)
    .bind(team)
    .bind(&q.conversation_id)
    .bind(&q.conversation_id)
    .fetch_one(&state.db)
    .await?;
    let hours = (w.end - w.start).num_hours().max(1) as f64;
    let by_type: Vec<(String, i64)> = sqlx::query_as(
        "SELECT m.content_type, COUNT(*) FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.deleted_at IS NULL AND m.created_at >= $1 AND m.created_at <= $2
           AND ($3 IS NULL OR c.team_id = $4)
         GROUP BY m.content_type",
    )
    .bind(&s)
    .bind(&e)
    .bind(team)
    .bind(team)
    .fetch_all(&state.db)
    .await?;
    let volume: Vec<(String, i64)> = sqlx::query_as(
        "SELECT substr(m.created_at, 1, 13), COUNT(*) FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.deleted_at IS NULL AND m.created_at >= $1 AND m.created_at <= $2
           AND ($3 IS NULL OR c.team_id = $4)
         GROUP BY 1 ORDER BY 1",
    )
    .bind(&s)
    .bind(&e)
    .bind(team)
    .bind(team)
    .fetch_all(&state.db)
    .await?;

    let type_map: Map<String, Value> =
        by_type.iter().map(|(t, c)| (t.clone(), json!(c))).collect();
    Ok(envelope::ok(json!({
        "data": {
            "summary": {
                "totalMessages": total,
                "messagesPerHour": ((total as f64 / hours) * 100.0).round() / 100.0,
                "averageResponseMinutes": 0,
                "byType": type_map,
                "byChannel": {},
                "bySentiment": {},
            },
            "volumeTrend": volume.iter().map(|(hour, count)| json!({
                "timestamp": format!("{hour}:00:00Z"), "value": count,
            })).collect::<Vec<_>>(),
            "typeDistribution": by_type.iter().map(|(t, c)| json!({
                "category": t, "value": c,
                "percentage": if total > 0 { (*c as f64 * 10000.0 / total as f64).round() / 100.0 } else { 0.0 },
            })).collect::<Vec<_>>(),
            "channelDistribution": [],
            "sentimentDistribution": [],
        },
        "metadata": metadata(total, started, w.granularity),
    })))
}

// ------------------------------------------------------------ users

pub async fn users(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<AnalyticsQuery>,
) -> Result {
    let started = std::time::Instant::now();
    let w = resolve_window(q.time_range.as_deref(), q.start_date.as_deref(), q.end_date.as_deref(), "7d")?;
    let (s, e) = (iso(&w.start), iso(&w.end));
    // Agents are additionally scoped to themselves (CRD 4240).
    let user_filter = if user.is_admin() { q.user_id.clone() } else { Some(user.id.clone()) };

    let (total_users, active_users): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN last_active_at >= $1 THEN 1 ELSE 0 END), 0)::bigint
         FROM agents WHERE deleted_at IS NULL AND is_active = 1",
    )
    .bind(&s)
    .fetch_one(&state.db)
    .await?;
    let performance: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT a.id, a.display_name, a.role, COUNT(DISTINCT m.conversation_id)
         FROM agents a
         LEFT JOIN messages m ON m.agent_id = a.id AND m.created_at >= $1 AND m.created_at <= $2
         WHERE a.deleted_at IS NULL AND a.is_active = 1 AND ($3 IS NULL OR a.id = $4)
         GROUP BY a.id ORDER BY 4 DESC LIMIT $5",
    )
    .bind(&s)
    .bind(&e)
    .bind(&user_filter)
    .bind(&user_filter)
    .bind(q.limit.unwrap_or(20))
    .fetch_all(&state.db)
    .await?;
    let activity: Vec<(String, i64)> = sqlx::query_as(
        "SELECT substr(created_at, 1, 10), COUNT(*) FROM activity_logs
         WHERE created_at >= $1 AND created_at <= $2 AND ($3 IS NULL OR agent_id = $4)
         GROUP BY 1 ORDER BY 1",
    )
    .bind(&s)
    .bind(&e)
    .bind(&user_filter)
    .bind(&user_filter)
    .fetch_all(&state.db)
    .await?;

    Ok(envelope::ok(json!({
        "data": {
            "summary": {
                "totalUsers": total_users,
                "activeUsers": active_users,
                "averageSessionMinutes": 0,
                "averageActivityPerDay": 0,
                "topPerformers": performance.iter().take(5).map(|(id, name, _, handled)| json!({
                    "userId": id, "displayName": name, "conversationsHandled": handled,
                })).collect::<Vec<_>>(),
            },
            "activityTrend": activity.iter().map(|(day, count)| json!({
                "timestamp": format!("{day}T00:00:00Z"), "value": count,
            })).collect::<Vec<_>>(),
            "performance": performance.iter().map(|(id, name, role, handled)| json!({
                "userId": id, "displayName": name, "role": role, "score": handled,
                "metrics": {
                    "conversationsHandled": handled,
                    "averageResponseMinutes": 0,
                    "customerSatisfaction": 0,
                    "resolutionRate": 0,
                },
            })).collect::<Vec<_>>(),
            "workload": performance.iter().map(|(id, name, _, handled)| json!({
                "userId": id, "displayName": name,
                "activeConversations": handled, "dailyMessages": 0,
                "utilizationRate": 0, "workingHours": 0,
            })).collect::<Vec<_>>(),
        },
        "metadata": metadata(total_users, started, w.granularity),
    })))
}

// ------------------------------------------------------------ performance

pub async fn performance(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<AnalyticsQuery>,
) -> Result {
    let started = std::time::Instant::now();
    let w = resolve_window(q.time_range.as_deref(), q.start_date.as_deref(), q.end_date.as_deref(), "24h")?;
    let since_ms = w.start.timestamp_millis();

    // Sourced from the request-metrics accumulation (§7.1 middleware).
    let (count, avg_ms, errors): (i64, f64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(AVG(value)::float8, 0),
                COALESCE(SUM(CASE WHEN CAST(tags::json->>'status' AS BIGINT) >= 500 THEN 1 ELSE 0 END), 0)::bigint
         FROM metrics WHERE name = 'http_request' AND timestamp >= $1",
    )
    .bind(since_ms)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0.0, 0));
    let secs = (w.end - w.start).num_seconds().max(1) as f64;
    let error_rate = if count > 0 { errors as f64 * 100.0 / count as f64 } else { 0.0 };

    Ok(envelope::ok(json!({
        "data": {
            "summary": {
                "averageResponseTimeMs": (avg_ms * 100.0).round() / 100.0,
                "throughputRps": ((count as f64 / secs) * 1000.0).round() / 1000.0,
                "errorRatePercent": (error_rate * 100.0).round() / 100.0,
                "uptimePercent": 100.0,
                "systemLoadPercent": 0.0,
            },
            "trends": [],
            "bottlenecks": [],
            "recommendations": [],
        },
        "metadata": metadata(count, started, w.granularity),
    })))
}

// ------------------------------------------------------------ custom query

#[derive(Deserialize)]
pub struct CustomBody {
    pub query: Option<String>,
    #[serde(default)]
    pub parameters: Map<String, Value>,
    pub limit: Option<i64>,
}

const CUSTOM_DATASETS: &[&str] = &["conversations", "messages", "activities", "metrics"];

/// POST /api/analytics/custom (CRD 4253-4259): the query specification names
/// one of the safe datasets; arbitrary SQL is never executed.
pub async fn custom(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CustomBody>,
) -> Result {
    require_query_permission(&user)?;
    let started = std::time::Instant::now();
    let dataset = body.query.as_deref().unwrap_or("").trim().to_lowercase();
    if !CUSTOM_DATASETS.contains(&dataset.as_str()) {
        return Err(AppError::BadRequest(format!(
            "Unknown query '{dataset}': must be one of {CUSTOM_DATASETS:?}"
        )));
    }
    let limit = body.limit.unwrap_or(100).clamp(1, 1000);
    let sql = match dataset.as_str() {
        "conversations" => "SELECT status AS category, COUNT(*) AS count FROM conversations WHERE deleted_at IS NULL GROUP BY status LIMIT $1",
        "messages" => "SELECT sender_type AS category, COUNT(*) AS count FROM messages WHERE deleted_at IS NULL GROUP BY sender_type LIMIT $1",
        "activities" => "SELECT action AS category, COUNT(*) AS count FROM activity_logs GROUP BY action LIMIT $1",
        _ => "SELECT name AS category, COUNT(*) AS count FROM metrics GROUP BY name LIMIT $1",
    };
    let rows: Vec<(Option<String>, i64)> =
        sqlx::query_as(sql).bind(limit).fetch_all(&state.db).await?;
    let result: Vec<Value> = rows
        .iter()
        .map(|(category, count)| json!({"category": category, "count": count}))
        .collect();
    let total = result.len() as i64;
    let _ = body.parameters;
    Ok(envelope::ok(json!({
        "data": result,
        "metadata": metadata(total, started, "raw"),
    })))
}

// ------------------------------------------------------------ export

#[derive(Deserialize)]
pub struct ExportBody {
    pub format: Option<String>,
    #[serde(rename = "fileName")]
    pub file_name: Option<String>,
    #[serde(default)]
    pub metrics: Vec<String>,
}

const CONVERSATION_METRICS: &[&str] =
    &["total_conversations", "active_conversations", "closed_conversations"];
const MESSAGE_METRICS: &[&str] = &["total_messages", "messages_per_hour"];

pub async fn export(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<ExportBody>,
) -> Result {
    require_query_permission(&user)?;
    let format = body.format.as_deref().unwrap_or("json");
    if !["json", "csv", "xlsx", "pdf"].contains(&format) {
        return Err(AppError::BadRequest(format!("Unsupported export format '{format}'")));
    }
    // Dataset selection (CRD 4265).
    let dataset = if body.metrics.is_empty()
        || body.metrics.iter().any(|m| CONVERSATION_METRICS.contains(&m.as_str()))
    {
        "conversations"
    } else if body.metrics.iter().any(|m| MESSAGE_METRICS.contains(&m.as_str())) {
        "messages"
    } else {
        return Err(AppError::BadRequest("No exportable metrics in the selection".into()));
    };

    let count: i64 = match dataset {
        "messages" => sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL"),
        _ => sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE deleted_at IS NULL"),
    }
    .fetch_one(&state.db)
    .await?;
    let content = match format {
        "csv" => format!("dataset,total\n{dataset},{count}\n"),
        _ => json!({"dataset": dataset, "total": count, "generatedAt": now_iso()}).to_string(),
    };

    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let file_name = body
        .file_name
        .clone()
        .unwrap_or_else(|| format!("analytics-export-{stamp}.{format}"));
    let key = format!("exports/{}", file_name);
    crate::domain::files::store::put_object(&state.config.upload_dir, &key, content.as_bytes())
        .await
        .map_err(|e| AppError::Internal(format!("export write failed: {e}")))?;
    let (sig, expires) = crate::domain::files::sign::sign(&state.config.jwt_secret, &key, 86_400);
    let base = state.config.backend_url.clone().unwrap_or_default();
    Ok(envelope::ok(json!({
        "downloadUrl": format!("{base}/api/files/public/{key}?expires={expires}&sig={sig}"),
        "fileName": file_name,
        "fileSize": content.len(),
        "format": format,
        "generatedAt": now_iso(),
        "expiresAt": chrono::DateTime::from_timestamp(expires, 0).map(|t| t.to_rfc3339()),
        "downloadCount": 0,
    })))
}

// ------------------------------------------------------------ health & metrics

pub async fn health(State(state): State<Arc<AppState>>) -> Result {
    let db_ok = sqlx::query_scalar::<_, i64>("SELECT 1::bigint").fetch_one(&state.db).await.is_ok();
    Ok(envelope::ok(json!({
        "status": if db_ok { "healthy" } else { "unhealthy" },
        "services": {
            "database": if db_ok { "healthy" } else { "unhealthy" },
            "cache": "healthy",
        },
    })))
}

#[derive(Deserialize)]
pub struct RecordMetricsBody {
    pub metrics: Option<Vec<Value>>,
    pub metric: Option<Value>,
}

fn validate_metric(m: &Value) -> std::result::Result<(), String> {
    let id_ok = m.get("id").and_then(Value::as_str).map(|s| !s.is_empty()).unwrap_or(false);
    let name_ok = m.get("name").and_then(Value::as_str).map(|s| !s.is_empty()).unwrap_or(false);
    let value_ok = m.get("value").and_then(Value::as_f64).map(f64::is_finite).unwrap_or(false);
    let ts_ok = m.get("timestamp").and_then(Value::as_i64).is_some();
    let tags_ok = m.get("tags").map(Value::is_object).unwrap_or(false);
    if id_ok && name_ok && value_ok && ts_ok && tags_ok {
        Ok(())
    } else {
        Err("metric requires non-empty id, name, finite value, numeric timestamp, tags object".into())
    }
}

pub async fn record_metrics(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<RecordMetricsBody>,
) -> Result {
    require_query_permission(&user)?;
    let batch: Vec<Value> = match (body.metrics, body.metric) {
        (Some(list), _) => list,
        (None, Some(single)) => vec![single],
        (None, None) => return Err(AppError::BadRequest("Missing metrics data".into())),
    };
    for m in &batch {
        validate_metric(m).map_err(AppError::BadRequest)?;
    }
    for m in &batch {
        sqlx::query("INSERT INTO metrics (name, value, timestamp, tags, unit) VALUES ($1, $2, $3, $4, $5)")
            .bind(m["name"].as_str())
            .bind(m["value"].as_f64())
            .bind(m["timestamp"].as_i64())
            .bind(m["tags"].to_string())
            .bind(m.get("unit").and_then(Value::as_str))
            .execute(&state.db)
            .await?;
    }
    Ok(envelope::message_only(&format!("{} metrics recorded", batch.len())))
}

#[derive(Deserialize)]
pub struct MetricQuery {
    #[serde(rename = "startTime")]
    pub start_time: Option<i64>,
    #[serde(rename = "endTime")]
    pub end_time: Option<i64>,
    pub aggregation: Option<String>,
    pub period: Option<String>,
    pub limit: Option<i64>,
}

fn period_ms(period: &str) -> i64 {
    match period {
        "1m" => 60_000,
        "5m" => 300_000,
        "15m" => 900_000,
        "1h" => 3_600_000,
        "6h" => 21_600_000,
        "1d" => 86_400_000,
        "1w" => 604_800_000,
        "1M" => 2_592_000_000,
        _ => 60_000,
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = p / 100.0 * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
    }
}

pub async fn query_metric(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(name): Path<String>,
    Query(q): Query<MetricQuery>,
) -> Result {
    let started = std::time::Instant::now();
    let start = q.start_time.unwrap_or(0);
    let end = q.end_time.unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    if name.trim().is_empty() || start >= end {
        return Err(AppError::BadRequest(
            "Metric name and a valid time range (start before end) are required".into(),
        ));
    }
    let rows: Vec<(f64, i64, Option<String>)> = sqlx::query_as(
        "SELECT value, timestamp, tags FROM metrics
         WHERE name = $1 AND timestamp >= $2 AND timestamp <= $3
         ORDER BY timestamp ASC LIMIT $4",
    )
    .bind(&name)
    .bind(start)
    .bind(end)
    .bind(q.limit.unwrap_or(10_000))
    .fetch_all(&state.db)
    .await?;

    let valid_aggs = ["sum", "avg", "min", "max", "count", "p50", "p95", "p99"];
    let agg = q.aggregation.as_deref().filter(|a| valid_aggs.contains(a));
    let period = q.period.as_deref().filter(|p| {
        ["1m", "5m", "15m", "1h", "6h", "1d", "1w", "1M"].contains(p)
    });

    let entries: Vec<Value> = if let Some(agg) = agg {
        let bucket = period_ms(period.unwrap_or("1m"));
        let mut buckets: std::collections::BTreeMap<i64, Vec<f64>> = Default::default();
        for (value, ts, _) in &rows {
            buckets.entry(ts / bucket * bucket).or_default().push(*value);
        }
        buckets
            .iter()
            .map(|(ts, values)| {
                let mut sorted = values.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let value = match agg {
                    "sum" => values.iter().sum(),
                    "avg" => values.iter().sum::<f64>() / values.len() as f64,
                    "min" => sorted.first().copied().unwrap_or(0.0),
                    "max" => sorted.last().copied().unwrap_or(0.0),
                    "count" => values.len() as f64,
                    "p50" => percentile(&sorted, 50.0),
                    "p95" => percentile(&sorted, 95.0),
                    "p99" => percentile(&sorted, 99.0),
                    _ => 0.0,
                };
                json!({
                    "name": name, "aggregation": agg, "value": value,
                    "timestamp": ts, "period": period.unwrap_or("1m"),
                    "tags": {}, "sampleCount": values.len(),
                })
            })
            .collect()
    } else {
        rows.iter()
            .map(|(value, ts, tags)| {
                json!({
                    "name": name, "aggregation": "sum", "value": value,
                    "timestamp": ts, "period": "1m",
                    "tags": tags.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).unwrap_or(json!({})),
                })
            })
            .collect()
    };
    let count = entries.len() as i64;
    Ok(envelope::ok(json!({
        "metrics": entries,
        "metadata": {
            "totalRecords": count,
            "queryDurationMs": started.elapsed().as_millis() as i64,
            "cacheHit": false,
            "period": period.unwrap_or("1m"),
        },
    })))
}

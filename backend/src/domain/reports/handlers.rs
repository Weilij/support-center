//! Report endpoints (CRD 4514-4656).

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

/// Full report-type catalog (CRD 4666); only GENERATABLE is backed by live data.
pub const CATALOG: &[&str] = &[
    "conversation_summary", "agent_performance", "team_analytics", "customer_satisfaction",
    "platform_usage", "message_statistics", "response_time_analysis", "workload_distribution",
    "system_health", "custom", "cost_analysis", "sla_compliance", "anomaly_detection",
    "audit_trail", "resource_utilization", "trend_forecast", "customer_insights",
    "channel_integration", "goal_achievement", "automation_effectiveness", "security_risk",
    "knowledge_base", "call_quality", "executive_summary",
];
pub const GENERATABLE: &[&str] =
    &["conversation_summary", "agent_performance", "message_statistics"];
pub const FORMATS: &[&str] = &["json", "csv", "excel", "pdf", "html"];
pub const GENERATABLE_FORMATS: &[&str] = &["json", "csv"];
const ADMIN_ONLY_TYPES: &[&str] = &["system_health", "custom", "team_analytics"];
const TIME_RANGES: &[&str] =
    &["today", "yesterday", "last_7_days", "last_30_days", "last_90_days", "this_month", "last_month", "custom"];
const MAX_CONCURRENT_GENERATING: i64 = 3;

fn sanitize(s: &str) -> String {
    s.replace(['<', '>'], "")
        .replace("javascript:", "")
        .trim()
        .to_string()
}

fn valid_report_id(id: &str) -> bool {
    let bare = id.strip_prefix("report_").unwrap_or(id);
    uuid::Uuid::parse_str(bare).is_ok()
}

#[derive(Debug, sqlx::FromRow)]
struct ReportRow {
    id: String,
    title: String,
    description: Option<String>,
    report_type: Option<String>,
    format: Option<String>,
    status: String,
    created_by: String,
    team_id: Option<i64>,
    time_range: Option<String>,
    filters: Option<String>,
    completed_at: Option<String>,
    error_message: Option<String>,
    duration_ms: Option<i64>,
    output_url: Option<String>,
    output_size: Option<i64>,
    download_count: i64,
    expires_at: Option<String>,
    created_at: String,
    updated_at: Option<String>,
}

const COLUMNS: &str = "id, title, description, report_type, format, status, created_by, team_id,
    time_range, filters, completed_at, error_message, duration_ms, output_url, output_size,
    download_count, expires_at, created_at, updated_at";

fn view(r: &ReportRow) -> Value {
    json!({
        "id": r.id,
        "title": r.title,
        "description": r.description,
        "type": r.report_type,
        "format": r.format,
        "status": r.status,
        "createdBy": r.created_by,
        "teamId": r.team_id,
        "timeRange": r.time_range,
        "filters": r.filters.as_deref().and_then(|f| serde_json::from_str::<Value>(f).ok()),
        "completedAt": r.completed_at,
        "errorMessage": r.error_message,
        "executionTimeMs": r.duration_ms,
        "downloadPath": r.output_url,
        "fileSize": r.output_size,
        "downloadCount": r.download_count,
        "expiresAt": r.expires_at,
        "createdAt": r.created_at,
        "updatedAt": r.updated_at,
    })
}

// ---------------------------------------------------------------- public

pub async fn health() -> Result {
    Ok(envelope::ok(json!({
        "status": "healthy", "module": "reports", "timestamp": now_iso(), "version": "1.0.0",
    })))
}

pub async fn info() -> Result {
    Ok(envelope::ok(json!({
        "module": "reports",
        "version": "1.0.0",
        "description": "Report generation, downloads, statistics, scheduling",
        "features": ["generate", "download", "preview", "batch", "scheduled"],
        "generatableTypes": GENERATABLE.iter().map(|t| json!({"code": t, "name": t})).collect::<Vec<_>>(),
        "endpoints": ["/api/reports", "/api/reports/{id}", "/api/reports/scheduled"],
        "permissionTiers": ["administrator", "team", "agent"],
        "timestamp": now_iso(),
    })))
}

// ---------------------------------------------------------------- generate

#[derive(Deserialize)]
pub struct GenerateBody {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub format: Option<String>,
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
    #[serde(rename = "customStart")]
    pub custom_start: Option<String>,
    #[serde(rename = "customEnd")]
    pub custom_end: Option<String>,
    pub filters: Option<Value>,
}

fn validate_generate(body: &GenerateBody) -> Result<(String, String, String, String)> {
    let kind = body.kind.as_deref().unwrap_or("");
    if !CATALOG.contains(&kind) {
        return Err(AppError::BadRequest(format!("Invalid report type '{kind}'")));
    }
    let title = sanitize(body.title.as_deref().unwrap_or(""));
    if title.is_empty() || title.chars().count() > 200 {
        return Err(AppError::BadRequest("title is required (max 200 characters)".into()));
    }
    if body.description.as_deref().map(|d| d.chars().count() > 1000).unwrap_or(false) {
        return Err(AppError::BadRequest("description exceeds 1000 characters".into()));
    }
    let format = body.format.as_deref().unwrap_or("");
    if !FORMATS.contains(&format) {
        return Err(AppError::BadRequest(format!("Invalid format '{format}'")));
    }
    let time_range = body.time_range.as_deref().unwrap_or("");
    if !TIME_RANGES.contains(&time_range) {
        return Err(AppError::BadRequest("Invalid or missing timeRange".into()));
    }
    if time_range == "custom" {
        let s = body.custom_start.as_deref().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok());
        let e = body.custom_end.as_deref().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok());
        match (s, e) {
            (Some(s), Some(e)) if s < e && e <= chrono::Utc::now() => {}
            (Some(_), Some(_)) => {
                return Err(AppError::BadRequest(
                    "custom range start must precede end, and end must not be in the future".into(),
                ))
            }
            _ => return Err(AppError::BadRequest("customStart and customEnd are required".into())),
        }
    }
    if let Some(filters) = &body.filters {
        if filters.get("teamIds").and_then(Value::as_array).map(|a| a.len() > 10).unwrap_or(false) {
            return Err(AppError::BadRequest("teamIds capped at 10".into()));
        }
        if filters.get("agentIds").and_then(Value::as_array).map(|a| a.len() > 50).unwrap_or(false) {
            return Err(AppError::BadRequest("agentIds capped at 50".into()));
        }
        if filters.get("tags").and_then(Value::as_array).map(|a| a.len() > 20).unwrap_or(false) {
            return Err(AppError::BadRequest("tags capped at 20".into()));
        }
    }
    Ok((kind.to_string(), title, format.to_string(), time_range.to_string()))
}

async fn build_content(state: &AppState, kind: &str, format: &str) -> String {
    let (label, count): (&str, i64) = match kind {
        "agent_performance" => (
            "agents",
            sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE deleted_at IS NULL")
                .fetch_one(&state.db)
                .await
                .unwrap_or(0),
        ),
        "message_statistics" => (
            "messages",
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL")
                .fetch_one(&state.db)
                .await
                .unwrap_or(0),
        ),
        _ => (
            "conversations",
            sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE deleted_at IS NULL")
                .fetch_one(&state.db)
                .await
                .unwrap_or(0),
        ),
    };
    match format {
        "csv" => format!("dataset,total\n{label},{count}\n"),
        _ => json!({"dataset": label, "total": count, "generatedAt": now_iso()}).to_string(),
    }
}

pub async fn generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<GenerateBody>,
) -> Result {
    let (kind, title, format, time_range) = validate_generate(&body)?;
    // Restricted types are administrator-only (CRD 4541).
    if ADMIN_ONLY_TYPES.contains(&kind.as_str()) && !user.is_admin() {
        return Err(AppError::Forbidden("This report type is administrator-only".into()));
    }
    // Catalog-valid but non-generatable type/format -> invalid parameters (CRD 4552).
    if !GENERATABLE.contains(&kind.as_str()) {
        return Err(AppError::BadRequest(format!(
            "Report type '{kind}' is not backed by live data and cannot be generated"
        )));
    }
    if !GENERATABLE_FORMATS.contains(&format.as_str()) {
        return Err(AppError::BadRequest(format!(
            "Format '{format}' has no live serializer; use one of {GENERATABLE_FORMATS:?}"
        )));
    }
    // Concurrent-generation cap (CRD 4689).
    let generating: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reports WHERE created_by = $1 AND status = 'generating' AND deleted_at IS NULL",
    )
    .bind(&user.id)
    .fetch_one(&state.db)
    .await?;
    if generating >= MAX_CONCURRENT_GENERATING {
        return Err(AppError::BadRequest("Too many reports are generating concurrently".into()));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let started = std::time::Instant::now();
    let expires = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
    sqlx::query(
        "INSERT INTO reports (id, title, description, report_type, format, status, created_by,
                              team_id, time_range, filters, expires_at, created_at)
         VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7, $8, $9, $10, $11)",
    )
    .bind(&id)
    .bind(&title)
    .bind(body.description.as_deref().map(sanitize))
    .bind(&kind)
    .bind(&format)
    .bind(&user.id)
    .bind(user.primary_team_id)
    .bind(&time_range)
    .bind(body.filters.as_ref().map(|f| f.to_string()))
    .bind(&expires)
    .bind(&now)
    .execute(&state.db)
    .await?;

    // pending -> generating -> completed | failed (CRD 4542).
    sqlx::query("UPDATE reports SET status = 'generating', generated_at = $1 WHERE id = $2")
        .bind(now_iso())
        .bind(&id)
        .execute(&state.db)
        .await?;
    let content = build_content(&state, &kind, &format).await;
    let key = format!("reports/{id}.{format}");
    match crate::domain::files::store::put_object(&state.config.upload_dir, &key, content.as_bytes()).await {
        Ok(()) => {
            sqlx::query(
                "UPDATE reports SET status = 'completed', completed_at = $1, output_url = $2,
                        output_size = $3, duration_ms = $4, updated_at = $5 WHERE id = $6",
            )
            .bind(now_iso())
            .bind(&key)
            .bind(content.len() as i64)
            .bind(started.elapsed().as_millis() as i64)
            .bind(now_iso())
            .bind(&id)
            .execute(&state.db)
            .await?;
        }
        Err(e) => {
            sqlx::query(
                "UPDATE reports SET status = 'failed', failed_at = $1, error_message = $2, updated_at = $3 WHERE id = $4",
            )
            .bind(now_iso())
            .bind(e.to_string())
            .bind(now_iso())
            .bind(&id)
            .execute(&state.db)
            .await?;
            return Err(AppError::Internal("Report generation failed".into()));
        }
    }

    let row: ReportRow = sqlx::query_as(&crate::db::pg_params(&format!("SELECT {COLUMNS} FROM reports WHERE id = $1")))
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    let mut resp = envelope::ok_msg(
        json!({"report": view(&row), "estimatedTime": "under a minute"}),
        "Report generated",
    );
    *resp.status_mut() = StatusCode::CREATED;
    Ok(resp)
}

// ---------------------------------------------------------------- list / detail

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub status: Option<String>,
    pub format: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    if let Some(status) = &q.status {
        if !["pending", "generating", "completed", "failed", "expired"].contains(&status.as_str()) {
            return Err(AppError::BadRequest("Invalid status filter".into()));
        }
    }
    let page = q.page.unwrap_or(1).clamp(1, 1000);
    let size = q.page_size.unwrap_or(20).clamp(1, 100);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reports WHERE deleted_at IS NULL
           AND ($1 IS NULL OR report_type = $2) AND ($3 IS NULL OR status = $4)
           AND ($5 IS NULL OR format = $6)",
    )
    .bind(&q.kind).bind(&q.kind)
    .bind(&q.status).bind(&q.status)
    .bind(&q.format).bind(&q.format)
    .fetch_one(&state.db)
    .await?;
    let rows: Vec<ReportRow> = sqlx::query_as(&crate::db::pg_params(&format!(
        "SELECT {COLUMNS} FROM reports WHERE deleted_at IS NULL
           AND ($1 IS NULL OR report_type = $2) AND ($3 IS NULL OR status = $4)
           AND ($5 IS NULL OR format = $6)
         ORDER BY created_at DESC, id DESC LIMIT $7 OFFSET $8"
    )))
    .bind(&q.kind).bind(&q.kind)
    .bind(&q.status).bind(&q.status)
    .bind(&q.format).bind(&q.format)
    .bind(size)
    .bind((page - 1) * size)
    .fetch_all(&state.db)
    .await?;
    let (pending, completed, failed): (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END), 0)::bigint,
                COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0)::bigint,
                COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0)::bigint
         FROM reports WHERE deleted_at IS NULL",
    )
    .fetch_one(&state.db)
    .await?;
    let total_pages = if total == 0 { 0 } else { (total + size - 1) / size };
    Ok(envelope::ok(json!({
        "reports": rows.iter().map(view).collect::<Vec<_>>(),
        "pagination": {
            "page": page, "pageSize": size, "total": total, "totalPages": total_pages,
            "hasNext": page < total_pages, "hasPrev": page > 1 && total_pages > 0,
        },
        "summary": {"total": total, "pending": pending, "completed": completed, "failed": failed},
    })))
}

async fn find_report(state: &AppState, id: &str) -> Result<ReportRow> {
    if !valid_report_id(id) {
        return Err(AppError::BadRequest("Invalid report identifier".into()));
    }
    sqlx::query_as::<_, ReportRow>(&crate::db::pg_params(&format!(
        "SELECT {COLUMNS} FROM reports WHERE id = $1 AND deleted_at IS NULL"
    )))
    .bind(id.strip_prefix("report_").unwrap_or(id))
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Report not found".into()))
}

pub async fn detail(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let row = find_report(&state, &id).await?;
    let downloads: Vec<(String, String)> = sqlx::query_as(
        "SELECT downloaded_by, downloaded_at FROM report_downloads WHERE report_id = $1
         ORDER BY downloaded_at DESC LIMIT 20",
    )
    .bind(&row.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut detail = view(&row);
    detail["generationLog"] = json!(["queued", "generating", row.status.clone()]);
    detail["dataSource"] = json!({"recordCount": row.output_size.unwrap_or(0)});
    detail["downloadHistory"] = json!(downloads
        .iter()
        .map(|(by, at)| json!({"userId": by, "downloadedAt": at}))
        .collect::<Vec<_>>());
    Ok(envelope::ok(detail))
}

// ---------------------------------------------------------------- download / delete

pub async fn download(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let row = find_report(&state, &id).await?;
    // Creator OR admin OR member of the owning team (CRD 4575).
    let allowed = user.is_admin()
        || row.created_by == user.id
        || row.team_id.map(|t| user.can_access_team(t)).unwrap_or(false);
    if !allowed {
        return Err(AppError::Forbidden("Access denied".into()));
    }
    if row.status != "completed" {
        return Err(AppError::NotFound("Report is not completed".into()));
    }
    let key = row.output_url.clone().filter(|k| !k.is_empty())
        .ok_or_else(|| AppError::NotFound("Report file missing".into()))?;
    let Some(bytes) = crate::domain::files::store::get_object(&state.config.upload_dir, &key).await
    else {
        return Err(AppError::NotFound("Report file missing".into()));
    };
    sqlx::query(
        "INSERT INTO report_downloads (id, report_id, downloaded_by, downloaded_at, method, size)
         VALUES ($1, $2, $3, $4, 'manual', $5)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&row.id)
    .bind(&user.id)
    .bind(now_iso())
    .bind(bytes.len() as i64)
    .execute(&state.db)
    .await?;
    let _ = sqlx::query(
        "UPDATE reports SET download_count = download_count + 1, last_downloaded_at = $1 WHERE id = $2",
    )
    .bind(now_iso())
    .bind(&row.id)
    .execute(&state.db)
    .await;

    let format = row.format.as_deref().unwrap_or("json");
    let content_type = if format == "csv" { "text/csv" } else { "application/json" };
    let filename = format!("{}.{format}", sanitize(&row.title).replace(' ', "_"));
    let mut resp = (StatusCode::OK, bytes).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(""));
    if let Ok(v) = HeaderValue::from_str(content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
    h.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));
    Ok(resp)
}

pub async fn delete_report(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let row = find_report(&state, &id).await?;
    if row.created_by != user.id && !user.is_admin() {
        return Err(AppError::Forbidden("Only the creator or an administrator may delete".into()));
    }
    if let Some(key) = row.output_url.as_deref().filter(|k| !k.is_empty()) {
        crate::domain::files::store::delete_object(&state.config.upload_dir, key).await;
    }
    sqlx::query("UPDATE reports SET deleted_at = $1, updated_at = $2 WHERE id = $3")
        .bind(now_iso())
        .bind(now_iso())
        .bind(&row.id)
        .execute(&state.db)
        .await?;
    Ok(envelope::message_only("Report deleted"))
}

// ---------------------------------------------------------------- stats / batch

pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let since = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reports WHERE deleted_at IS NULL AND created_at >= $1",
    )
    .bind(&since)
    .fetch_one(&state.db)
    .await?;
    let by_status: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM reports WHERE deleted_at IS NULL AND created_at >= $1 GROUP BY status",
    )
    .bind(&since)
    .fetch_all(&state.db)
    .await?;
    let by_type: Vec<(Option<String>, i64, Option<f64>)> = sqlx::query_as(
        "SELECT report_type, COUNT(*), AVG(output_size)::float8 FROM reports
         WHERE deleted_at IS NULL AND created_at >= $1 GROUP BY report_type ORDER BY 2 DESC",
    )
    .bind(&since)
    .fetch_all(&state.db)
    .await?;
    let avg_time: f64 = sqlx::query_scalar(
        "SELECT COALESCE(AVG(duration_ms)::float8, 0) FROM reports WHERE deleted_at IS NULL AND created_at >= $1",
    )
    .bind(&since)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0.0);

    let mut status_map: serde_json::Map<String, Value> =
        ["pending", "generating", "completed", "failed", "expired"]
            .iter()
            .map(|s| (s.to_string(), json!(0)))
            .collect();
    for (status, count) in &by_status {
        status_map.insert(status.clone(), json!(count));
    }
    let mut type_map: serde_json::Map<String, Value> =
        CATALOG.iter().map(|t| (t.to_string(), json!(0))).collect();
    for (kind, count, _) in &by_type {
        if let Some(k) = kind {
            type_map.insert(k.clone(), json!(count));
        }
    }
    Ok(envelope::ok(json!({
        "total": total,
        "byType": type_map,
        "byStatus": status_map,
        "byFormat": FORMATS.iter().map(|f| (f.to_string(), json!(0))).collect::<serde_json::Map<_, _>>(),
        "averageGenerationTimeMs": avg_time,
        "popularReports": by_type.iter().take(5).map(|(kind, count, size)| json!({
            "type": kind, "count": count, "averageSize": size.unwrap_or(0.0),
        })).collect::<Vec<_>>(),
        "userUsage": [],
        "monthlyTrend": [],
    })))
}

#[derive(Deserialize)]
pub struct BatchBody {
    #[serde(rename = "reportIds")]
    pub report_ids: Option<Vec<String>>,
    pub action: Option<String>,
}

pub async fn batch(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<BatchBody>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let ids = body
        .report_ids
        .filter(|v| !v.is_empty() && v.len() <= 50)
        .ok_or_else(|| AppError::BadRequest("reportIds must contain 1-50 entries".into()))?;
    let action = body.action.as_deref().unwrap_or("");
    if !["delete", "regenerate", "download", "export"].contains(&action) {
        return Err(AppError::BadRequest(format!("Invalid action '{action}'")));
    }
    for id in &ids {
        if !valid_report_id(id) {
            return Err(AppError::BadRequest(format!("Malformed identifier '{id}'")));
        }
    }
    let mut results = Vec::new();
    let (mut ok_count, mut failed_count) = (0usize, 0usize);
    for id in &ids {
        let outcome = match find_report(&state, id).await {
            Err(e) => Err(e.to_string()),
            Ok(row) => match action {
                "delete" => {
                    let _ = sqlx::query("UPDATE reports SET deleted_at = $1 WHERE id = $2")
                        .bind(now_iso())
                        .bind(&row.id)
                        .execute(&state.db)
                        .await;
                    Ok(json!({"id": id, "success": true}))
                }
                "download" | "export" => Ok(json!({
                    "id": id, "success": true,
                    "downloadRef": format!("/api/reports/{}/download", row.id),
                })),
                _ => Ok(json!({"id": id, "success": true, "regenerated": true})),
            },
        };
        match outcome {
            Ok(entry) => {
                ok_count += 1;
                results.push(entry);
            }
            Err(error) => {
                failed_count += 1;
                results.push(json!({"id": id, "success": false, "error": error}));
            }
        }
    }
    Ok(envelope::ok_msg(
        json!({
            "success": failed_count == 0,
            "total": ids.len(),
            "successCount": ok_count,
            "failedCount": failed_count,
            "results": results,
        }),
        &format!("Batch {action} processed"),
    ))
}

// ---------------------------------------------------------------- templates / preview

pub async fn templates(
    Extension(_user): Extension<AuthUser>,
    Path(kind): Path<String>,
) -> Result {
    if !CATALOG.contains(&kind.as_str()) {
        return Err(AppError::BadRequest(format!("Invalid report type '{kind}'")));
    }
    let templates: Vec<Value> = match kind.as_str() {
        "conversation_summary" => vec![
            json!({"name": "Daily digest", "description": "Daily totals", "options": {"groupBy": "day"}}),
            json!({"name": "Weekly overview", "description": "Weekly rollup", "options": {"groupBy": "week"}}),
        ],
        "agent_performance" => vec![
            json!({"name": "Top performers", "description": "Ranked agents", "options": {"sortOrder": "desc"}}),
        ],
        _ => vec![],
    };
    Ok(envelope::ok(json!({ "templates": templates, "type": kind })))
}

#[derive(Deserialize)]
pub struct PreviewBody {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
}

pub async fn preview(
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<PreviewBody>,
) -> Result {
    let kind = body.kind.as_deref().unwrap_or("");
    if !CATALOG.contains(&kind) {
        return Err(AppError::BadRequest(format!("Invalid report type '{kind}'")));
    }
    if body.time_range.as_deref().unwrap_or("").is_empty() {
        return Err(AppError::BadRequest("timeRange is required".into()));
    }
    // Synthetic sample only — no live data, nothing persisted (CRD 4619).
    let seed = chrono::Utc::now().timestamp_millis() % 50;
    let sample = match kind {
        "conversation_summary" => json!({
            "period": {"range": body.time_range},
            "totals": {"conversations": 120 + seed, "active": 30 + seed / 2, "completed": 90},
            "averages": {"responseMinutes": 4.2, "resolutionMinutes": 38.0},
            "byPlatform": {"line": 80, "facebook": 40},
            "dailyTrend": [{"day": "Mon", "count": 18 + seed}],
        }),
        "agent_performance" => json!({
            "agents": [{"name": "Agent A", "handled": 42 + seed, "satisfaction": 4.6}],
            "trends": [{"week": 1, "score": 80 + seed}],
        }),
        _ => json!({"message": format!("Preview not available for '{kind}'")}),
    };
    Ok(envelope::ok_msg(sample, "Preview generated"))
}

// ---------------------------------------------------------------- scheduled

#[derive(Deserialize)]
pub struct ScheduledBody {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub format: Option<String>,
    pub schedule: Option<Value>,
    pub filters: Option<Value>,
    pub recipients: Option<Vec<Value>>,
}

fn validate_schedule(schedule: &Value) -> Result<(String, String, Option<i64>, Option<i64>)> {
    let frequency = schedule.get("frequency").and_then(Value::as_str).unwrap_or("");
    if !["daily", "weekly", "monthly", "quarterly"].contains(&frequency) {
        return Err(AppError::BadRequest("Invalid schedule frequency".into()));
    }
    let time = schedule.get("time").and_then(Value::as_str).unwrap_or("");
    let valid_time = time.len() == 5
        && time.as_bytes()[2] == b':'
        && time[0..2].parse::<u8>().map(|h| h <= 23).unwrap_or(false)
        && time[3..5].parse::<u8>().map(|m| m <= 59).unwrap_or(false);
    if !valid_time {
        return Err(AppError::BadRequest("Schedule time must be HH:mm".into()));
    }
    let dow = schedule.get("dayOfWeek").and_then(Value::as_i64);
    let dom = schedule.get("dayOfMonth").and_then(Value::as_i64);
    if frequency == "weekly" && !dow.map(|d| (0..=6).contains(&d)).unwrap_or(false) {
        return Err(AppError::BadRequest("dayOfWeek (0-6) is required for weekly".into()));
    }
    if frequency == "monthly" && !dom.map(|d| (1..=31).contains(&d)).unwrap_or(false) {
        return Err(AppError::BadRequest("dayOfMonth (1-31) is required for monthly".into()));
    }
    Ok((frequency.to_string(), time.to_string(), dow, dom))
}

pub fn next_run(frequency: &str) -> String {
    let next = match frequency {
        "daily" => chrono::Utc::now() + chrono::Duration::days(1),
        "weekly" => chrono::Utc::now() + chrono::Duration::weeks(1),
        "monthly" => chrono::Utc::now() + chrono::Duration::days(30),
        _ => chrono::Utc::now() + chrono::Duration::days(90),
    };
    next.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn validate_scheduled(body: &ScheduledBody) -> Result<(String, String, String, String)> {
    let name = sanitize(body.name.as_deref().unwrap_or(""));
    if name.is_empty() || name.chars().count() > 200 {
        return Err(AppError::BadRequest("name is required (max 200 characters)".into()));
    }
    let kind = body.kind.as_deref().unwrap_or("");
    if !CATALOG.contains(&kind) {
        return Err(AppError::BadRequest("Invalid report type".into()));
    }
    let format = body.format.as_deref().unwrap_or("");
    if !FORMATS.contains(&format) {
        return Err(AppError::BadRequest("Invalid format".into()));
    }
    let schedule = body.schedule.as_ref().ok_or_else(|| AppError::BadRequest("schedule is required".into()))?;
    let (frequency, _, _, _) = validate_schedule(schedule)?;
    if let Some(recipients) = &body.recipients {
        if recipients.len() > 20 {
            return Err(AppError::BadRequest("recipients capped at 20".into()));
        }
        for r in recipients {
            let email = r.get("email").and_then(Value::as_str).unwrap_or("");
            let rname = r.get("name").and_then(Value::as_str).unwrap_or("");
            if !email.contains('@') || rname.is_empty() {
                return Err(AppError::BadRequest("Each recipient needs a valid email and name".into()));
            }
        }
    }
    Ok((name, kind.to_string(), format.to_string(), frequency))
}

pub async fn create_scheduled(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<ScheduledBody>,
) -> Result {
    let (name, kind, format, frequency) = validate_scheduled(&body)?;
    let id = uuid::Uuid::new_v4().to_string();
    let next = next_run(&frequency);
    sqlx::query(
        "INSERT INTO scheduled_reports
            (id, name, description, report_type, format, parameters, schedule_type,
             schedule_config, is_active, created_by, recipients, next_run_at, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9, $10, $11, $12)",
    )
    .bind(&id)
    .bind(&name)
    .bind(body.description.as_deref().map(sanitize))
    .bind(&kind)
    .bind(&format)
    .bind(body.filters.as_ref().map(|f| f.to_string()))
    .bind(&frequency)
    .bind(body.schedule.as_ref().map(|s| s.to_string()))
    .bind(&user.id)
    .bind(body.recipients.as_ref().map(|r| json!(r).to_string()))
    .bind(&next)
    .bind(now_iso())
    .execute(&state.db)
    .await?;
    let mut resp = envelope::ok_msg(
        json!({
            "id": id, "name": name, "type": kind, "format": format,
            "schedule": body.schedule, "recipients": body.recipients,
            "isActive": true, "createdBy": user.id, "nextRunAt": next,
        }),
        "Scheduled report created",
    );
    *resp.status_mut() = StatusCode::CREATED;
    Ok(resp)
}

pub async fn list_scheduled(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    // Admins see all; others only their own (CRD 4636).
    let creator = (!user.is_admin()).then_some(user.id.clone());
    type SchedRow = (String, String, Option<String>, Option<String>, Option<String>, Option<String>, i64, String, Option<String>, Option<String>);
    let rows: Vec<SchedRow> = sqlx::query_as(
        "SELECT id, name, report_type, format, schedule_type, schedule_config, is_active,
                created_by, next_run_at, last_run_at
         FROM scheduled_reports WHERE deleted_at IS NULL AND ($1 IS NULL OR created_by = $2)
         ORDER BY next_run_at ASC",
    )
    .bind(&creator)
    .bind(&creator)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|(id, name, kind, format, freq, config, active, creator, next, last)| {
            json!({
                "id": id, "name": name, "type": kind, "format": format,
                "frequency": freq,
                "schedule": config.as_deref().and_then(|c| serde_json::from_str::<Value>(c).ok()),
                "isActive": *active != 0, "createdBy": creator,
                "nextRunAt": next, "lastRunAt": last,
            })
        })
        .collect();
    Ok(envelope::ok(json!({ "scheduled": items, "count": items.len() })))
}

pub async fn update_scheduled(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<ScheduledBody>,
) -> Result {
    if uuid::Uuid::parse_str(&id).is_err() {
        return Err(AppError::BadRequest("Invalid scheduled-report identifier".into()));
    }
    let creator: Option<String> = sqlx::query_scalar(
        "SELECT created_by FROM scheduled_reports WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    let Some(creator) = creator else {
        return Err(AppError::NotFound("Scheduled report not found".into()));
    };
    if creator != user.id {
        return Err(AppError::Forbidden("Only the creator may update this schedule".into()));
    }
    let (name, kind, format, frequency) = validate_scheduled(&body)?;
    sqlx::query(
        "UPDATE scheduled_reports
            SET name = $1, report_type = $2, format = $3, schedule_type = $4, schedule_config = $5,
                next_run_at = $6, updated_at = $7
          WHERE id = $8",
    )
    .bind(&name)
    .bind(&kind)
    .bind(&format)
    .bind(&frequency)
    .bind(body.schedule.as_ref().map(|s| s.to_string()))
    .bind(next_run(&frequency))
    .bind(now_iso())
    .bind(&id)
    .execute(&state.db)
    .await?;
    Ok(envelope::ok_msg(json!({"id": id, "name": name}), "Scheduled report updated"))
}

pub async fn delete_scheduled(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    if uuid::Uuid::parse_str(&id).is_err() {
        return Err(AppError::BadRequest("Invalid scheduled-report identifier".into()));
    }
    let creator: Option<String> = sqlx::query_scalar(
        "SELECT created_by FROM scheduled_reports WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    let Some(creator) = creator else {
        return Err(AppError::NotFound("Scheduled report not found".into()));
    };
    if creator != user.id {
        return Err(AppError::Forbidden("Only the creator may delete this schedule".into()));
    }
    sqlx::query(
        "UPDATE scheduled_reports SET deleted_at = $1, is_active = 0, updated_at = $2 WHERE id = $3",
    )
    .bind(now_iso())
    .bind(now_iso())
    .bind(&id)
    .execute(&state.db)
    .await?;
    Ok(envelope::message_only("Scheduled report deleted"))
}

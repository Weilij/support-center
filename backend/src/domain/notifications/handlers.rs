//! In-app notification inbox endpoints (CRD 4890-5002).

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::{AppError, FieldProblem};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::service::{self, NewNotification, CHANNELS, PRIORITIES, TYPES};

type Result<T = Response> = std::result::Result<T, AppError>;

fn problem(field: &str, message: &str) -> FieldProblem {
    FieldProblem { field: field.into(), message: message.into(), value: None }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct NotificationRow {
    id: String,
    agent_id: String,
    #[sqlx(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    content: Option<String>,
    data: Option<String>,
    priority: String,
    is_read: i64,
    read_at: Option<String>,
    expires_at: Option<String>,
    created_at: String,
    updated_at: Option<String>,
}

fn view(row: &NotificationRow) -> Value {
    json!({
        "id": row.id,
        "userId": row.agent_id,
        "type": row.kind,
        "title": row.title,
        "content": row.content,
        "data": row.data.as_deref().and_then(|d| serde_json::from_str::<Value>(d).ok()),
        "priority": row.priority,
        "isRead": row.is_read != 0,
        "readAt": row.read_at,
        "expiresAt": row.expires_at,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
    })
}

const COLUMNS: &str = "id, agent_id, type, title, content, data, priority, is_read, read_at,
    expires_at, created_at, updated_at";

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub priority: Option<String>,
    #[serde(rename = "isRead")]
    pub is_read: Option<String>,
    #[serde(rename = "dateFrom")]
    pub date_from: Option<String>,
    #[serde(rename = "dateTo")]
    pub date_to: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
    pub limit: Option<i64>,
}

/// GET /api/notifications (CRD 4892-4899).
pub async fn list(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    let mut problems = Vec::new();
    if let Some(t) = &q.kind {
        if !TYPES.contains(&t.as_str()) {
            problems.push(problem("type", "invalid notification type"));
        }
    }
    if let Some(p) = &q.priority {
        if !PRIORITIES.contains(&p.as_str()) {
            problems.push(problem("priority", "invalid priority"));
        }
    }
    let is_read: Option<bool> = match q.is_read.as_deref() {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => {
            problems.push(problem("isRead", "must be \"true\" or \"false\""));
            None
        }
    };
    for (field, value) in [("dateFrom", &q.date_from), ("dateTo", &q.date_to)] {
        if let Some(d) = value {
            if chrono::DateTime::parse_from_rfc3339(d).is_err()
                && chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err()
            {
                problems.push(problem(field, "invalid date"));
            }
        }
    }
    if q.page.is_some_and(|p| p < 1) {
        problems.push(problem("page", "must be a positive integer"));
    }
    if q.page_size.is_some_and(|s| s < 1) {
        problems.push(problem("pageSize", "must be between 1 and 100"));
    }
    if !problems.is_empty() {
        return Err(AppError::Validation("Invalid filters".into(), problems));
    }
    let page = q.page.unwrap_or(1);
    let size = q.page_size.or(q.limit).unwrap_or(20).min(100);
    let now = now_iso();

    let read_bind = is_read.map(|b| b as i64);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications
         WHERE agent_id = ? AND (expires_at IS NULL OR expires_at > ?)
           AND (? IS NULL OR type = ?) AND (? IS NULL OR priority = ?)
           AND (? IS NULL OR is_read = ?)
           AND (? IS NULL OR created_at >= ?) AND (? IS NULL OR created_at <= ?)",
    )
    .bind(&user.id)
    .bind(&now)
    .bind(&q.kind)
    .bind(&q.kind)
    .bind(&q.priority)
    .bind(&q.priority)
    .bind(read_bind)
    .bind(read_bind)
    .bind(&q.date_from)
    .bind(&q.date_from)
    .bind(&q.date_to)
    .bind(&q.date_to)
    .fetch_one(&state.db)
    .await?;
    let rows: Vec<NotificationRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM notifications
         WHERE agent_id = ? AND (expires_at IS NULL OR expires_at > ?)
           AND (? IS NULL OR type = ?) AND (? IS NULL OR priority = ?)
           AND (? IS NULL OR is_read = ?)
           AND (? IS NULL OR created_at >= ?) AND (? IS NULL OR created_at <= ?)
         ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
    ))
    .bind(&user.id)
    .bind(&now)
    .bind(&q.kind)
    .bind(&q.kind)
    .bind(&q.priority)
    .bind(&q.priority)
    .bind(read_bind)
    .bind(read_bind)
    .bind(&q.date_from)
    .bind(&q.date_from)
    .bind(&q.date_to)
    .bind(&q.date_to)
    .bind(size)
    .bind((page - 1) * size)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "notifications": rows.iter().map(view).collect::<Vec<_>>(),
        "page": page,
        "limit": size,
        "total": total,
    })))
}

async fn find_owned(
    state: &AppState,
    user_id: &str,
    id: &str,
) -> Result<NotificationRow> {
    sqlx::query_as::<_, NotificationRow>(&format!(
        "SELECT {COLUMNS} FROM notifications WHERE id = ? AND agent_id = ?"
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Notification not found".into()))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let row = find_owned(&state, &user.id, &id).await?;
    Ok(envelope::ok(view(&row)))
}

#[derive(Deserialize, Clone)]
pub struct CreateBody {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub data: Option<Value>,
    pub priority: Option<String>,
    pub channels: Option<Vec<String>>,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
}

fn validate_create(body: &CreateBody) -> Vec<FieldProblem> {
    let mut problems = Vec::new();
    let title = body.title.as_deref().unwrap_or("").trim();
    let content = body.content.as_deref().unwrap_or("").trim();
    if title.is_empty() || title.chars().count() > 200 {
        problems.push(problem("title", "required, max 200 characters"));
    }
    if content.is_empty() || content.chars().count() > 1000 {
        problems.push(problem("content", "required, max 1000 characters"));
    }
    match body.kind.as_deref() {
        Some(t) if TYPES.contains(&t) => {}
        _ => problems.push(problem("type", "missing or invalid notification type")),
    }
    if let Some(p) = &body.priority {
        if !PRIORITIES.contains(&p.as_str()) {
            problems.push(problem("priority", "invalid priority"));
        }
    }
    if let Some(d) = &body.data {
        if !d.is_object() {
            problems.push(problem("data", "must be an object"));
        }
    }
    if let Some(channels) = &body.channels {
        for c in channels {
            if !CHANNELS.contains(&c.as_str()) {
                problems.push(problem("channels", &format!("invalid channel '{c}'")));
            }
        }
    }
    if let Some(e) = &body.expires_at {
        match chrono::DateTime::parse_from_rfc3339(e) {
            Ok(t) if t.timestamp() > chrono::Utc::now().timestamp() => {}
            _ => problems.push(problem("expiresAt", "must be a future timestamp")),
        }
    }
    problems
}

/// POST /api/notifications (CRD 4908-4914).
pub async fn create(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateBody>,
) -> Result {
    let problems = validate_create(&body);
    if !problems.is_empty() {
        return Err(AppError::Validation("Validation failed".into(), problems));
    }
    // Non-admins may only target themselves (CRD 4910).
    let recipient = if user.is_admin() {
        body.user_id.clone().unwrap_or_else(|| user.id.clone())
    } else {
        user.id.clone()
    };
    let id = service::create(
        &state,
        NewNotification {
            recipient: &recipient,
            kind: body.kind.as_deref().unwrap_or("system"),
            title: body.title.as_deref().unwrap_or("").trim(),
            content: body.content.as_deref().unwrap_or("").trim(),
            data: body.data.clone(),
            priority: body.priority.as_deref().unwrap_or("normal"),
            expires_at: body.expires_at.clone(),
        },
    )
    .await?;
    Ok(envelope::ok(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct BulkBody {
    pub notifications: Option<Vec<CreateBody>>,
    #[serde(rename = "batchId")]
    pub batch_id: Option<String>,
}

/// POST /api/notifications/bulk (CRD 4916-4921): admin only, resilient.
pub async fn bulk_create(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<BulkBody>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let items = body
        .notifications
        .filter(|v| !v.is_empty() && v.len() <= 1000)
        .ok_or_else(|| AppError::BadRequest("notifications must be an array of 1-1000 items".into()))?;
    let mut validation = Vec::new();
    for (i, item) in items.iter().enumerate() {
        for p in validate_create(item) {
            validation.push(problem(&format!("notifications[{i}].{}", p.field), &p.message));
        }
    }
    if !validation.is_empty() {
        return Err(AppError::Validation("Batch validation failed".into(), validation));
    }
    let mut created = Vec::new();
    let mut failures = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let recipient = item.user_id.clone().unwrap_or_else(|| user.id.clone());
        match service::create(
            &state,
            NewNotification {
                recipient: &recipient,
                kind: item.kind.as_deref().unwrap_or("system"),
                title: item.title.as_deref().unwrap_or("").trim(),
                content: item.content.as_deref().unwrap_or("").trim(),
                data: item.data.clone(),
                priority: item.priority.as_deref().unwrap_or("normal"),
                expires_at: item.expires_at.clone(),
            },
        )
        .await
        {
            Ok(id) => created.push(id),
            Err(e) => failures.push(json!({"index": i, "error": e.to_string()})),
        }
    }
    Ok(envelope::ok_msg(
        json!({
            "successCount": created.len(),
            "failedCount": failures.len(),
            "createdIds": created,
            "failures": failures,
            "batchId": body.batch_id,
        }),
        &format!("Created {} notifications", created.len()),
    ))
}

pub async fn mark_read(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    find_owned(&state, &user.id, &id).await?;
    sqlx::query(
        "UPDATE notifications SET is_read = 1, read_at = COALESCE(read_at, ?), updated_at = ? WHERE id = ?",
    )
    .bind(now_iso())
    .bind(now_iso())
    .bind(&id)
    .execute(&state.db)
    .await?;
    Ok(envelope::message_only("Notification marked as read"))
}

#[derive(Deserialize, Default)]
pub struct MarkAllBody {
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

pub async fn mark_all_read(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<MarkAllBody>>,
) -> Result {
    let kind = body.and_then(|Json(b)| b.kind);
    let updated = sqlx::query(
        "UPDATE notifications SET is_read = 1, read_at = ?, updated_at = ?
         WHERE agent_id = ? AND is_read = 0 AND (? IS NULL OR type = ?)",
    )
    .bind(now_iso())
    .bind(now_iso())
    .bind(&user.id)
    .bind(&kind)
    .bind(&kind)
    .execute(&state.db)
    .await?
    .rows_affected();
    Ok(envelope::ok_msg(
        json!({"updated": updated}),
        &format!("{updated} notifications marked as read"),
    ))
}

pub async fn delete_one(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    find_owned(&state, &user.id, &id).await?;
    sqlx::query("DELETE FROM notifications WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(envelope::message_only("Notification deleted"))
}

/// GET /api/notifications/stats (CRD 4943-4946).
pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    let now = now_iso();
    let (total, unread): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN is_read = 0 THEN 1 ELSE 0 END), 0)
         FROM notifications WHERE agent_id = ? AND (expires_at IS NULL OR expires_at > ?)",
    )
    .bind(&user.id)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;
    let mut by_type = serde_json::Map::new();
    for t in TYPES {
        // Only message/assignment/mention/system carry real per-type counts
        // (CRD 4946); the remaining types report zeros.
        let counts = if ["new_message", "conversation_assigned", "mention", "system"].contains(t) {
            let (tt, tu): (i64, i64) = sqlx::query_as(
                "SELECT COUNT(*), COALESCE(SUM(CASE WHEN is_read = 0 THEN 1 ELSE 0 END), 0)
                 FROM notifications WHERE agent_id = ? AND type = ? AND (expires_at IS NULL OR expires_at > ?)",
            )
            .bind(&user.id)
            .bind(t)
            .bind(&now)
            .fetch_one(&state.db)
            .await
            .unwrap_or((0, 0));
            json!({"total": tt, "unread": tu})
        } else {
            json!({"total": 0, "unread": 0})
        };
        by_type.insert(t.to_string(), counts);
    }
    let mut by_priority = serde_json::Map::new();
    for p in PRIORITIES {
        let count: i64 = if ["high", "urgent"].contains(p) {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM notifications
                 WHERE agent_id = ? AND priority = ? AND (expires_at IS NULL OR expires_at > ?)",
            )
            .bind(&user.id)
            .bind(p)
            .bind(&now)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0)
        } else {
            0
        };
        by_priority.insert(p.to_string(), json!(count));
    }
    let day_start = chrono::Utc::now().format("%Y-%m-%dT00:00:00").to_string();
    let week_start = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
    let month_start = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    let mut ranges = serde_json::Map::new();
    for (label, since) in [("today", day_start), ("thisWeek", week_start), ("thisMonth", month_start)] {
        let c: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notifications WHERE agent_id = ? AND created_at >= ?",
        )
        .bind(&user.id)
        .bind(&since)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
        ranges.insert(label.to_string(), json!(c));
    }
    let channel_stats: serde_json::Map<String, Value> = CHANNELS
        .iter()
        .map(|c| (c.to_string(), json!({"sent": 0, "delivered": 0, "failed": 0})))
        .collect();
    Ok(envelope::ok(json!({
        "total": total,
        "unread": unread,
        "byType": by_type,
        "byPriority": by_priority,
        "timeRanges": ranges,
        "channelStats": channel_stats,
    })))
}

#[derive(Deserialize)]
pub struct CountQuery {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub limit: Option<i64>,
}

pub async fn unread_count(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<CountQuery>,
) -> Result {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications
         WHERE agent_id = ? AND is_read = 0 AND (expires_at IS NULL OR expires_at > ?)
           AND (? IS NULL OR type = ?)",
    )
    .bind(&user.id)
    .bind(now_iso())
    .bind(&q.kind)
    .bind(&q.kind)
    .fetch_one(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "count": count,
        "type": q.kind.unwrap_or_else(|| "all".into()),
    })))
}

pub async fn recent(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<CountQuery>,
) -> Result {
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    let rows: Vec<NotificationRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM notifications
         WHERE agent_id = ? AND is_read = 0 AND (expires_at IS NULL OR expires_at > ?)
         ORDER BY created_at DESC, id DESC LIMIT ?"
    ))
    .bind(&user.id)
    .bind(now_iso())
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "notifications": rows.iter().map(view).collect::<Vec<_>>(),
        "count": rows.len(),
        "limit": limit,
    })))
}

/// DELETE /api/notifications/cleanup (CRD 4960-4963): admin, system-wide.
pub async fn cleanup(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let deleted = sqlx::query("DELETE FROM notifications WHERE expires_at IS NOT NULL AND expires_at <= ?")
        .bind(now_iso())
        .execute(&state.db)
        .await?
        .rows_affected();
    Ok(envelope::ok_msg(json!({"deleted": deleted}), &format!("{deleted} expired notifications removed")))
}

pub async fn channel_stats(
    Extension(user): Extension<AuthUser>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let channels: serde_json::Map<String, Value> = CHANNELS
        .iter()
        .map(|c| {
            let enabled = matches!(*c, "database" | "realtime");
            (c.to_string(), json!({"enabled": enabled, "type": c, "stats": null}))
        })
        .collect();
    Ok(envelope::ok(json!({ "channels": channels })))
}

#[derive(Deserialize, Default)]
pub struct TestChannelBody {
    pub message: Option<String>,
}

pub async fn test_channel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(channel): Path<String>,
    body: Option<Json<TestChannelBody>>,
) -> Result {
    if !CHANNELS.contains(&channel.as_str()) {
        return Err(AppError::BadRequest(format!(
            "Invalid channel '{channel}': must be one of {CHANNELS:?}"
        )));
    }
    let message = body
        .and_then(|Json(b)| b.message)
        .unwrap_or_else(|| "Test notification".into());
    let result = match channel.as_str() {
        "database" | "realtime" => {
            let id = service::create(
                &state,
                NewNotification {
                    recipient: &user.id,
                    kind: "system",
                    title: "Channel test",
                    content: &message,
                    data: Some(json!({"test": true, "channel": channel})),
                    priority: "low",
                    expires_at: service::expiry(1),
                },
            )
            .await?;
            json!({"success": true, "messageId": id})
        }
        // External channels require their own configuration; unconfigured
        // attempts fail gracefully (CRD 4975, 5072).
        other => json!({
            "success": false,
            "error": format!("Channel '{other}' is not configured"),
        }),
    };
    Ok(envelope::ok_msg(result, &format!("Channel test: {channel}")))
}

// ------------------------------------------------ internal trigger endpoints

#[derive(Deserialize)]
pub struct NewMessageBody {
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "senderName")]
    pub sender_name: Option<String>,
    pub content: Option<String>,
}

/// POST /api/notifications/new-message (CRD 4977-4981).
pub async fn trigger_new_message(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<NewMessageBody>,
) -> Result {
    let recipient = body
        .user_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("userId is required".into()))?;
    let sender = body.sender_name.as_deref().unwrap_or("Unknown");
    let preview: String = body.content.as_deref().unwrap_or("").chars().take(100).collect();
    let id = service::create(
        &state,
        NewNotification {
            recipient,
            kind: "new_message",
            title: "新訊息",
            content: &format!("{sender}: {preview}"),
            data: Some(json!({"conversationId": body.conversation_id, "senderName": sender})),
            priority: "normal",
            expires_at: service::expiry(24),
        },
    )
    .await?;
    Ok(envelope::ok(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct AssignedBody {
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "customerName")]
    pub customer_name: Option<String>,
    #[serde(rename = "assignedBy")]
    pub assigned_by: Option<String>,
}

/// POST /api/notifications/conversation-assigned (CRD 4983-4986).
pub async fn trigger_assigned(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<AssignedBody>,
) -> Result {
    let recipient = body
        .user_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("userId is required".into()))?;
    let customer = body.customer_name.as_deref().unwrap_or("客戶");
    let assigner = body.assigned_by.as_deref().unwrap_or("System");
    let id = service::create(
        &state,
        NewNotification {
            recipient,
            kind: "conversation_assigned",
            title: "對話指派",
            content: &format!("{assigner} 將 {customer} 的對話指派給您"),
            data: Some(json!({"conversationId": body.conversation_id, "assignedBy": assigner})),
            priority: "high",
            expires_at: service::expiry(24 * 7),
        },
    )
    .await?;
    Ok(envelope::ok(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct SystemBody {
    #[serde(rename = "userIds")]
    pub user_ids: Option<Vec<String>>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub data: Option<Value>,
    pub priority: Option<String>,
    #[serde(rename = "broadcastToAll")]
    pub broadcast_to_all: Option<bool>,
}

/// POST /api/notifications/system (CRD 4988-4992).
pub async fn trigger_system(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<SystemBody>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let title = body.title.as_deref().unwrap_or("").trim();
    let content = body.content.as_deref().unwrap_or("").trim();
    if title.is_empty() || content.is_empty() {
        return Err(AppError::BadRequest("title and content are required".into()));
    }
    let broadcast = body.broadcast_to_all.unwrap_or(false)
        || body.user_ids.as_ref().map(|v| v.is_empty()).unwrap_or(true);
    let recipients = if broadcast {
        service::active_staff(&state.db).await?
    } else {
        body.user_ids.clone().unwrap_or_default()
    };
    if recipients.is_empty() {
        return Err(AppError::BadRequest("No recipients resolved".into()));
    }
    let mut ids = Vec::new();
    for recipient in &recipients {
        if let Ok(id) = service::create(
            &state,
            NewNotification {
                recipient,
                kind: "system",
                title,
                content,
                data: body.data.clone(),
                priority: "normal",
                expires_at: service::expiry(24 * 30),
            },
        )
        .await
        {
            ids.push(id);
        }
    }
    Ok(envelope::ok(json!({
        "ids": ids,
        "count": ids.len(),
        "broadcastToAll": broadcast,
    })))
}

/// POST /api/notifications/broadcast (CRD 4994-4998).
pub async fn broadcast(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<SystemBody>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let title = body.title.as_deref().unwrap_or("").trim();
    let content = body.content.as_deref().unwrap_or("").trim();
    if title.is_empty() || content.is_empty() {
        return Err(AppError::BadRequest("title and content are required".into()));
    }
    let recipients = service::active_staff(&state.db).await?;
    if recipients.is_empty() {
        return Err(AppError::BadRequest("No active users".into()));
    }
    let mut data = body.data.clone().unwrap_or_else(|| json!({}));
    data["broadcastBy"] = json!(user.id);
    data["priority"] = json!(body.priority.clone().unwrap_or_else(|| "normal".into()));
    let mut ids = Vec::new();
    for recipient in &recipients {
        if let Ok(id) = service::create(
            &state,
            NewNotification {
                recipient,
                kind: "system",
                title,
                content,
                data: Some(data.clone()),
                priority: "normal",
                expires_at: service::expiry(24 * 30),
            },
        )
        .await
        {
            ids.push(id);
        }
    }
    Ok(envelope::ok(json!({
        "ids": ids,
        "recipientCount": recipients.len(),
        "broadcastBy": user.id,
        "timestamp": now_iso(),
    })))
}

// ------------------------------------------------ health / info (public)

pub async fn health() -> Result {
    Ok(envelope::ok(json!({
        "module": "notifications",
        "status": "healthy",
        "timestamp": now_iso(),
        "version": "1.0.0",
    })))
}

pub async fn module_info() -> Result {
    Ok(envelope::ok(json!({
        "module": "notifications",
        "description": "In-app notification inbox, internal triggers, task reminders, alerting",
        "capabilities": ["inbox", "bulk", "stats", "channels", "triggers", "reminders"],
        "types": TYPES,
        "priorities": PRIORITIES,
        "channels": CHANNELS,
    })))
}

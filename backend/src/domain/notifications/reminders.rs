//! Personal task reminders (CRD 5006-5051), base path /api/reminders.

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::service::{self, NewNotification};

type Result<T = Response> = std::result::Result<T, AppError>;

#[derive(Debug, Clone, sqlx::FromRow)]
struct ReminderRow {
    id: String,
    agent_id: String,
    title: String,
    content: Option<String>,
    remind_at: Option<String>,
    conversation_id: Option<String>,
    repeat_type: String,
    repeat_interval: Option<i64>,
    is_completed: i64,
    is_sent: i64,
    completed_at: Option<String>,
    sent_at: Option<String>,
    created_at: String,
}

const COLUMNS: &str = "id, agent_id, title, content, remind_at, conversation_id, repeat_type,
    repeat_interval, is_completed, is_sent, completed_at, sent_at, created_at";

fn view(r: &ReminderRow) -> Value {
    json!({
        "id": r.id,
        "userId": r.agent_id,
        "title": r.title,
        "content": r.content,
        "remindAt": r.remind_at,
        "conversationId": r.conversation_id,
        "repeatType": r.repeat_type,
        "repeatInterval": r.repeat_interval,
        "isCompleted": r.is_completed != 0,
        "isSent": r.is_sent != 0,
        "completedAt": r.completed_at,
        "sentAt": r.sent_at,
        "createdAt": r.created_at,
    })
}

#[derive(Deserialize)]
pub struct ReminderBody {
    pub title: Option<String>,
    pub content: Option<String>,
    #[serde(rename = "remindAt")]
    pub remind_at: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "repeatType")]
    pub repeat_type: Option<String>,
    #[serde(rename = "repeatInterval")]
    pub repeat_interval: Option<i64>,
}

/// POST /api/reminders (CRD 5009-5013).
pub async fn create(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<ReminderBody>,
) -> Result {
    let title = body.title.as_deref().unwrap_or("").trim();
    let remind_at_raw = body.remind_at.as_deref().unwrap_or("");
    if title.is_empty() || remind_at_raw.is_empty() {
        return Err(AppError::BadRequest("Title and remindAt are required".into()));
    }
    let remind_at = chrono::DateTime::parse_from_rfc3339(remind_at_raw)
        .map_err(|_| AppError::BadRequest("Invalid remindAt date format".into()))?;
    if remind_at.timestamp() <= chrono::Utc::now().timestamp() {
        return Err(AppError::BadRequest("remindAt must be in the future".into()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO task_reminders
            (id, agent_id, title, content, remind_at, conversation_id, repeat_type, repeat_interval, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(title)
    .bind(&body.content)
    .bind(remind_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    .bind(&body.conversation_id)
    .bind(body.repeat_type.as_deref().unwrap_or("none"))
    .bind(body.repeat_interval.unwrap_or(1))
    .bind(now_iso())
    .execute(&state.db)
    .await?;
    Ok(envelope::created(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(rename = "includeCompleted")]
    pub include_completed: Option<String>,
    pub minutes: Option<i64>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    let include_completed = q.include_completed.as_deref() == Some("true");
    let rows: Vec<ReminderRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM task_reminders
         WHERE agent_id = ? AND (? OR is_completed = 0)
         ORDER BY remind_at ASC"
    ))
    .bind(&user.id)
    .bind(include_completed)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "reminders": rows.iter().map(view).collect::<Vec<_>>(),
        "count": rows.len(),
    })))
}

/// GET /api/reminders/upcoming (CRD 5020-5023).
pub async fn upcoming(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    let minutes = q.minutes.unwrap_or(30);
    let until = (chrono::Utc::now() + chrono::Duration::minutes(minutes)).to_rfc3339();
    let rows: Vec<ReminderRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM task_reminders
         WHERE agent_id = ? AND is_completed = 0 AND is_sent = 0 AND remind_at <= ?
         ORDER BY remind_at ASC"
    ))
    .bind(&user.id)
    .bind(&until)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "reminders": rows.iter().map(view).collect::<Vec<_>>(),
        "count": rows.len(),
        "windowMinutes": minutes,
    })))
}

/// GET /api/reminders/stats (CRD 5025-5027).
pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    let now = now_iso();
    let (total, pending, completed, overdue): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN is_completed = 0 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN is_completed = 1 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN is_completed = 0 AND is_sent = 0 AND remind_at < ? THEN 1 ELSE 0 END), 0)
         FROM task_reminders WHERE agent_id = ?",
    )
    .bind(&now)
    .bind(&user.id)
    .fetch_one(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "total": total, "pending": pending, "completed": completed, "overdue": overdue,
    })))
}

async fn find_owned(state: &AppState, user_id: &str, id: &str) -> Result<ReminderRow> {
    sqlx::query_as::<_, ReminderRow>(&format!(
        "SELECT {COLUMNS} FROM task_reminders WHERE id = ? AND agent_id = ?"
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Reminder not found".into()))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let row = find_owned(&state, &user.id, &id).await?;
    Ok(envelope::ok(view(&row)))
}

/// PUT /api/reminders/{id} (CRD 5032-5035): changing the time re-arms it.
pub async fn update(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<ReminderBody>,
) -> Result {
    find_owned(&state, &user.id, &id).await?;
    if let Some(title) = &body.title {
        sqlx::query("UPDATE task_reminders SET title = ? WHERE id = ?")
            .bind(title.trim())
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    if let Some(content) = &body.content {
        sqlx::query("UPDATE task_reminders SET content = ? WHERE id = ?")
            .bind(content)
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    if let Some(raw) = &body.remind_at {
        let at = chrono::DateTime::parse_from_rfc3339(raw)
            .map_err(|_| AppError::BadRequest("Invalid remindAt date format".into()))?;
        sqlx::query("UPDATE task_reminders SET remind_at = ?, is_sent = 0, sent_at = NULL WHERE id = ?")
            .bind(at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    if let Some(rt) = &body.repeat_type {
        sqlx::query("UPDATE task_reminders SET repeat_type = ? WHERE id = ?")
            .bind(rt)
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    if let Some(ri) = body.repeat_interval {
        sqlx::query("UPDATE task_reminders SET repeat_interval = ? WHERE id = ?")
            .bind(ri)
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    sqlx::query("UPDATE task_reminders SET updated_at = ? WHERE id = ?")
        .bind(now_iso())
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(envelope::message_only("Reminder updated"))
}

pub async fn complete(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    find_owned(&state, &user.id, &id).await?;
    sqlx::query("UPDATE task_reminders SET is_completed = 1, completed_at = ?, updated_at = ? WHERE id = ?")
        .bind(now_iso())
        .bind(now_iso())
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(envelope::message_only("Reminder completed"))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    find_owned(&state, &user.id, &id).await?;
    sqlx::query("DELETE FROM task_reminders WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(envelope::message_only("Reminder deleted"))
}

/// POST /api/reminders/process (CRD 5043-5046): admin-only manual pass.
pub async fn process(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let processed = process_due(&state).await;
    Ok(envelope::ok_msg(
        json!({"processed": processed}),
        &format!("{processed} reminders processed"),
    ))
}

/// The due-reminder pass (CRD 5048-5051): fire notification, mark sent,
/// spawn the next occurrence for repeating reminders. Individual failures
/// are isolated.
pub async fn process_due(state: &AppState) -> usize {
    let now = now_iso();
    let due: Vec<ReminderRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM task_reminders
         WHERE remind_at <= ? AND is_completed = 0 AND is_sent = 0"
    ))
    .bind(&now)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut processed = 0;
    for r in &due {
        let preview: String = r.content.as_deref().unwrap_or("").chars().take(50).collect();
        let content = if preview.is_empty() {
            r.title.clone()
        } else {
            format!("{}: {preview}", r.title)
        };
        let _ = service::create(
            state,
            NewNotification {
                recipient: &r.agent_id,
                kind: "task_reminder",
                title: "任務提醒",
                content: &content,
                data: Some(json!({"reminderId": r.id, "conversationId": r.conversation_id})),
                priority: "high",
                expires_at: service::expiry(24),
            },
        )
        .await;
        let _ = sqlx::query("UPDATE task_reminders SET is_sent = 1, sent_at = ? WHERE id = ?")
            .bind(&now)
            .bind(&r.id)
            .execute(&state.db)
            .await;

        // Spawn the next occurrence for repeating reminders.
        if r.repeat_type != "none" {
            if let Some(at) = r
                .remind_at
                .as_deref()
                .and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok())
            {
                let interval = r.repeat_interval.unwrap_or(1).max(1);
                let next = match r.repeat_type.as_str() {
                    "daily" => at + chrono::Duration::days(interval),
                    "weekly" => at + chrono::Duration::weeks(interval),
                    "monthly" => at + chrono::Duration::days(30 * interval),
                    _ => at,
                };
                let _ = sqlx::query(
                    "INSERT INTO task_reminders
                        (id, agent_id, title, content, remind_at, conversation_id, repeat_type, repeat_interval, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&r.agent_id)
                .bind(&r.title)
                .bind(&r.content)
                .bind(next.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                .bind(&r.conversation_id)
                .bind(&r.repeat_type)
                .bind(interval)
                .bind(&now)
                .execute(&state.db)
                .await;
            }
        }
        processed += 1;
    }
    processed
}

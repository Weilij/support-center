//! Delayed-message handlers (CRD §2.4).

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::domain::auth::store::log_activity;
use crate::domain::messaging::service::{self, ScheduleParams};
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

const MIN_DELAY: i64 = 1;
const MAX_DELAY: i64 = 120;

fn epoch_ms(iso: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

/// Send permission scoped to the conversation: admin, unassigned pool, or a
/// member of the assigned team (the §1.3 capability rule).
async fn check_send_permission(
    state: &AppState,
    user: &AuthUser,
    conversation_id: &str,
) -> Result<()> {
    let team: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT team_id FROM conversations WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(conversation_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(team) = team else {
        return Err(AppError::NotFound("Conversation not found".into()));
    };
    let allowed = user.is_admin() || team.map(|t| user.can_access_team(t)).unwrap_or(true);
    if !allowed {
        return Err(AppError::Forbidden(
            "You do not have permission to send messages in this conversation".into(),
        ));
    }
    Ok(())
}

/// Countdown broadcast parameters (CRD 1327).
struct Countdown<'a> {
    conversation_id: &'a str,
    message_id: &'a str,
    content: &'a str,
    message_type: &'a str,
    platform: &'a str,
    delay_seconds: i64,
    scheduled_at: &'a str,
}

/// Countdown broadcast (CRD 1327), best-effort: conversation + acting agent.
fn broadcast_countdown(state: &AppState, user: &AuthUser, c: Countdown<'_>) {
    let Countdown { conversation_id, message_id, content, message_type, platform, delay_seconds, scheduled_at } = c;
    let payload = json!({
        "conversationId": conversation_id,
        "messageId": message_id,
        "agentId": user.id,
        "preview": content.chars().take(100).collect::<String>(),
        "messageType": message_type,
        "platform": platform,
        "delaySeconds": delay_seconds,
        "scheduledSendTime": scheduled_at,
        "recallDeadline": scheduled_at,
        "countdownStarted": true,
        "remainingSeconds": delay_seconds,
        "canRecall": true,
        "scheduledBy": user.display_name,
        "priority": "normal",
    });
    state.realtime.to_conversation(conversation_id, "delayed_message_countdown", payload.clone());
    state.realtime.to_user(&user.id, "delayed_message_countdown", payload);
}

// ================================================================ v2 family

#[derive(Deserialize)]
pub struct V2SendBody {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    pub content: Option<String>,
    pub platform: Option<String>,
    #[serde(rename = "userId", alias = "recipientId")]
    pub user_id: Option<String>,
    #[serde(rename = "delaySeconds", alias = "delay")]
    pub delay_seconds: Option<i64>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
}

pub async fn v2_send(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
    Json(body): Json<V2SendBody>,
) -> Result {
    let conversation_id = body.conversation_id.as_deref().unwrap_or("");
    let content = body.content.as_deref().unwrap_or("");
    let platform = body.platform.as_deref().unwrap_or("");
    let recipient = body.user_id.as_deref().unwrap_or("");
    if conversation_id.is_empty() || content.is_empty() || platform.is_empty() || recipient.is_empty()
    {
        return Err(AppError::BadRequest(
            "conversationId, content, platform and userId are required".into(),
        ));
    }
    let delay = body.delay_seconds.unwrap_or(5);
    if !(MIN_DELAY..=MAX_DELAY).contains(&delay) {
        return Err(AppError::BadRequest(format!(
            "delaySeconds must be between {MIN_DELAY} and {MAX_DELAY}"
        )));
    }
    check_send_permission(&state, &user, conversation_id).await?;

    let result = service::schedule_delayed(
        &state,
        &user.id,
        ScheduleParams {
            conversation_id: conversation_id.to_string(),
            content: content.to_string(),
            delay_seconds: delay,
            message_type: body.message_type.clone(),
            recipient_id: Some(recipient.to_string()),
            platform: Some(platform.to_string()),
            media_url: None,
            metadata: None,
        },
    )
    .await;
    if result["success"] != json!(true) {
        let reason = result["error"].as_str().unwrap_or("Scheduling failed").to_string();
        if reason.contains("not found") {
            return Err(AppError::NotFound(reason));
        }
        return Err(AppError::Internal(reason));
    }
    let message_id = result["delayedMessageId"].as_str().unwrap_or_default().to_string();
    let scheduled_iso = result["scheduledSendTime"].as_str().unwrap_or_default().to_string();
    let fire_ms = epoch_ms(&scheduled_iso);

    // Best-effort audit (CRD 1192) and countdown broadcast (CRD 1194).
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "delayed_message_scheduled", "delayed_message", Some(&message_id),
        Some(json!({"conversationId": conversation_id, "delaySeconds": delay, "platform": platform})),
        ip.as_deref(), None,
    )
    .await;
    broadcast_countdown(&state, &user, Countdown {
        conversation_id, message_id: &message_id, content,
        message_type: body.message_type.as_deref().unwrap_or("text"),
        platform, delay_seconds: delay, scheduled_at: &scheduled_iso,
    });

    Ok(envelope::ok(json!({
        "messageId": message_id,
        "scheduledSendTime": fire_ms,
        "canCancelUntil": fire_ms,
        "delaySeconds": delay,
        "conversationId": conversation_id,
    })))
}

#[derive(Deserialize, Default)]
pub struct V2CancelBody {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    pub reason: Option<String>,
}

pub async fn v2_cancel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(message_id): Path<String>,
    body: Option<Json<V2CancelBody>>,
) -> Result {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    if message_id.is_empty() {
        return Err(AppError::BadRequest("messageId is required".into()));
    }
    if body.conversation_id.as_deref().unwrap_or("").is_empty() {
        return Err(AppError::BadRequest("conversationId is required".into()));
    }
    let reason = body.reason.as_deref().unwrap_or("User cancelled");

    let result = service::cancel_delayed(&state, &message_id, &user.id, Some(reason), false).await;
    if result["success"] != json!(true) {
        let err = result["error"].as_str().unwrap_or("Cancellation failed");
        let mapped = if err.contains("not found") {
            "Message not found or already processed".to_string()
        } else if let Some(status) = result["status"].as_str() {
            format!("Message already {status}")
        } else if err.contains("after scheduled send time") {
            "Message send time has passed".to_string()
        } else {
            err.to_string()
        };
        return Err(AppError::BadRequest(mapped));
    }

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "delayed_message_cancelled", "delayed_message", Some(&message_id),
        Some(json!({"conversationId": body.conversation_id, "reason": reason})),
        None, None,
    )
    .await;

    Ok(envelope::ok(json!({
        "messageId": message_id,
        "cancelledAt": epoch_ms(result["cancelledAt"].as_str().unwrap_or_default()),
        "cancelledBy": user.display_name,
    })))
}

#[derive(Deserialize)]
pub struct ConversationQuery {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
}

pub async fn v2_status(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(message_id): Path<String>,
    Query(q): Query<ConversationQuery>,
) -> Result {
    let conversation_id = q
        .conversation_id
        .filter(|c| !c.is_empty())
        .ok_or_else(|| AppError::BadRequest("conversationId is required".into()))?;

    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT status, scheduled_at FROM scheduled_messages WHERE id = ? AND conversation_id = ?",
    )
    .bind(&message_id)
    .bind(&conversation_id)
    .fetch_optional(&state.db)
    .await?;

    let Some((status, scheduled_at)) = row else {
        return Ok(envelope::ok(json!({
            "exists": false,
            "status": "not_found",
            "remainingSeconds": 0,
            "canCancel": false,
            "scheduledSendTime": null,
        })));
    };
    let fire_ms = scheduled_at.as_deref().map(epoch_ms).unwrap_or(0);
    let remaining_ms = (fire_ms - chrono::Utc::now().timestamp_millis()).max(0);
    let remaining_secs = (remaining_ms + 999) / 1000; // rounded up, floored at 0
    Ok(envelope::ok(json!({
        "exists": true,
        "status": status,
        "remainingSeconds": remaining_secs,
        "canCancel": status == "pending" && remaining_secs > 0,
        "scheduledSendTime": fire_ms,
    })))
}

pub async fn v2_pending(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ConversationQuery>,
) -> Result {
    let conversation_id = q
        .conversation_id
        .filter(|c| !c.is_empty())
        .ok_or_else(|| AppError::BadRequest("conversationId is required".into()))?;
    let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, content, scheduled_at FROM scheduled_messages
         WHERE conversation_id = ? AND status = 'pending' ORDER BY scheduled_at ASC",
    )
    .bind(&conversation_id)
    .fetch_all(&state.db)
    .await?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let messages: Vec<Value> = rows
        .iter()
        .map(|(id, content, at)| {
            let fire = at.as_deref().map(epoch_ms).unwrap_or(0);
            json!({
                "messageId": id,
                "preview": content.as_deref().unwrap_or("").chars().take(100).collect::<String>(),
                "scheduledSendTime": fire,
                "remainingMs": (fire - now_ms).max(0),
            })
        })
        .collect();
    Ok(envelope::ok(json!({
        "conversationId": conversation_id,
        "count": messages.len(),
        "messages": messages,
    })))
}

/// Permanently failed scheduled messages, newest-failure-first (CRD 1289-1291).
pub async fn v2_failed(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ConversationQuery>,
) -> Result {
    let conversation_id = q
        .conversation_id
        .filter(|c| !c.is_empty())
        .ok_or_else(|| AppError::BadRequest("conversationId is required".into()))?;
    type FailedRow = (String, Option<String>, Option<String>, Option<String>, Option<String>);
    let rows: Vec<FailedRow> =
        sqlx::query_as(
            "SELECT id, content, metadata, updated_at, scheduled_at FROM scheduled_messages
             WHERE conversation_id = ? AND status = 'failed'
             ORDER BY updated_at DESC, id DESC",
        )
        .bind(&conversation_id)
        .fetch_all(&state.db)
        .await?;
    let failed: Vec<Value> = rows
        .iter()
        .map(|(id, content, metadata, updated, scheduled)| {
            let meta: Value = metadata
                .as_deref()
                .and_then(|m| serde_json::from_str(m).ok())
                .unwrap_or(Value::Null);
            json!({
                "messageId": id,
                "preview": content.as_deref().unwrap_or("").chars().take(100).collect::<String>(),
                "platform": meta.get("platform"),
                "failedAt": updated,
                "failureReason": meta.get("failureReason"),
                "retryCount": meta.get("retryCount").and_then(Value::as_i64).unwrap_or(0),
                "originalSendTime": scheduled,
                "conversationId": conversation_id,
            })
        })
        .collect();
    Ok(envelope::ok(json!({ "count": failed.len(), "failed": failed })))
}

/// Scheduler operational metrics for one conversation (CRD 1293-1295).
pub async fn v2_metrics(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ConversationQuery>,
) -> Result {
    let conversation_id = q
        .conversation_id
        .filter(|c| !c.is_empty())
        .ok_or_else(|| AppError::BadRequest("conversationId is required".into()))?;
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM scheduled_messages WHERE conversation_id = ? GROUP BY status",
    )
    .bind(&conversation_id)
    .fetch_all(&state.db)
    .await?;
    let count_of = |s: &str| counts.iter().find(|(k, _)| k == s).map(|(_, c)| *c).unwrap_or(0);
    let (pending, sent, failed, cancelled) =
        (count_of("pending"), count_of("sent"), count_of("failed"), count_of("cancelled"));
    let total = pending + sent + failed + cancelled;
    let processed = sent + failed;
    let next_fire: Option<String> = sqlx::query_scalar(
        "SELECT MIN(scheduled_at) FROM scheduled_messages
         WHERE conversation_id = ? AND status = 'pending'",
    )
    .bind(&conversation_id)
    .fetch_one(&state.db)
    .await?;
    let success_rate = if processed > 0 { sent as f64 * 100.0 / processed as f64 } else { 0.0 };
    Ok(envelope::ok(json!({
        "conversationId": conversation_id,
        "totals": {
            "scheduled": total, "sent": sent, "failed": failed,
            "cancelled": cancelled, "pending": pending,
        },
        "deadLetter": { "size": failed, "writeSuccesses": failed, "writeFailures": 0 },
        "nextScheduledTime": next_fire,
        "successRate": (success_rate * 100.0).round() / 100.0,
        "totalProcessed": processed,
        "timestamp": now_iso(),
    })))
}

pub async fn health() -> Result {
    Ok(envelope::ok(json!({
        "service": "delayed-messages-v2",
        "status": "healthy",
        "features": {
            "instantCancel": true,
            "preciseScheduling": true,
            "durablePersistence": true,
        },
        "timestamp": now_iso(),
    })))
}

// ================================================================ legacy family

#[derive(Deserialize)]
pub struct LegacySendBody {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    pub content: Option<String>,
    #[serde(rename = "delaySeconds")]
    pub delay_seconds: Option<i64>,
    #[serde(rename = "senderId")]
    pub sender_id: Option<String>,
    #[serde(rename = "recipientId", alias = "userId")]
    pub recipient_id: Option<String>,
    pub platform: Option<String>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    #[serde(rename = "mediaUrl")]
    pub media_url: Option<String>,
}

pub async fn legacy_send(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<LegacySendBody>,
) -> Result {
    let conversation_id = body.conversation_id.as_deref().unwrap_or("");
    let content = body.content.as_deref().unwrap_or("");
    let platform = body.platform.as_deref().unwrap_or("");
    let mut problems = Vec::new();
    if conversation_id.is_empty() {
        problems.push("conversationId is required");
    }
    if content.is_empty() {
        problems.push("content is required");
    } else if content.chars().count() > 5000 {
        problems.push("content exceeds the 5000 character limit");
    }
    match body.delay_seconds {
        Some(d) if (MIN_DELAY..=MAX_DELAY).contains(&d) => {}
        Some(_) => problems.push("delaySeconds must be between 1 and 120"),
        None => problems.push("delaySeconds is required"),
    }
    if body.sender_id.as_deref().unwrap_or("").is_empty() {
        problems.push("senderId is required");
    }
    if body.recipient_id.as_deref().unwrap_or("").is_empty() {
        problems.push("recipientId is required");
    }
    if !["line", "facebook"].contains(&platform) {
        problems.push("platform must be one of: line, facebook");
    }
    if let Some(url) = body.media_url.as_deref().filter(|u| !u.is_empty()) {
        if !url.starts_with("https://") {
            problems.push("mediaUrl must be a valid HTTPS URL");
        }
    }
    if !problems.is_empty() {
        return Err(AppError::BadRequest(problems.join("; ")));
    }
    check_send_permission(&state, &user, conversation_id).await?;

    let delay = body.delay_seconds.unwrap_or(5);
    let result = service::schedule_delayed(
        &state,
        &user.id,
        ScheduleParams {
            conversation_id: conversation_id.to_string(),
            content: content.to_string(),
            delay_seconds: delay,
            message_type: body.message_type.clone(),
            recipient_id: body.recipient_id.clone(),
            platform: Some(platform.to_string()),
            media_url: body.media_url.clone(),
            metadata: None,
        },
    )
    .await;
    if result["success"] != json!(true) {
        return Err(AppError::BadRequest(
            result["error"].as_str().unwrap_or("Scheduling failed").to_string(),
        ));
    }
    let message_id = result["delayedMessageId"].as_str().unwrap_or_default().to_string();
    let scheduled = result["scheduledSendTime"].as_str().unwrap_or_default().to_string();
    broadcast_countdown(&state, &user, Countdown {
        conversation_id, message_id: &message_id, content,
        message_type: body.message_type.as_deref().unwrap_or("text"),
        platform, delay_seconds: delay, scheduled_at: &scheduled,
    });
    Ok(envelope::ok(json!({
        "messageId": message_id,
        "scheduledSendTime": scheduled,
        "recallDeadline": result["recallDeadline"],
    })))
}

pub async fn legacy_recall(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(message_id): Path<String>,
) -> Result {
    if message_id.trim().is_empty() {
        return Err(AppError::BadRequest("Message ID is required".into()));
    }
    // Marker, ownership, deadline checks (CRD 1257-1260).
    if !state.recallable_messages.is_recallable(&message_id) {
        broadcast_recall_failed(&state, &message_id, &user, "Message not found or already processed");
        return Err(AppError::BadRequest("Message not found or already processed".into()));
    }
    let row: Option<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT agent_id, scheduled_at, conversation_id, content
         FROM scheduled_messages WHERE id = ?",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((sender, scheduled_at, conversation_id, content)) = row else {
        return Err(AppError::BadRequest("Message not found or already processed".into()));
    };
    if sender != user.id {
        broadcast_recall_failed(&state, &message_id, &user, "Permission denied");
        return Err(AppError::BadRequest(
            "Permission denied: only the sender can recall the message".into(),
        ));
    }
    if let Some(at) = &scheduled_at {
        if now_iso().as_str() >= at.as_str() {
            broadcast_recall_failed(&state, &message_id, &user, "Recall deadline has passed");
            return Err(AppError::BadRequest("Recall deadline has passed".into()));
        }
    }
    let result = service::cancel_delayed(&state, &message_id, &user.id, Some("manual recall"), true).await;
    if result["success"] != json!(true) {
        let err = result["error"].as_str().unwrap_or("Recall failed").to_string();
        broadcast_recall_failed(&state, &message_id, &user, &err);
        return Err(AppError::BadRequest(err));
    }
    state.realtime.to_conversation(
        &conversation_id,
        "delayed_message_recalled",
        json!({
            "conversationId": conversation_id,
            "messageId": message_id,
            "agentId": user.id,
            "recalledBy": user.display_name,
            "recalledAt": now_iso(),
            "originalContent": content.as_deref().unwrap_or("").chars().take(100).collect::<String>(),
            "success": true,
            "reason": "manual recall",
            "priority": "high",
        }),
    );
    Ok(envelope::ok(json!({ "messageId": message_id })))
}

fn broadcast_recall_failed(state: &AppState, message_id: &str, user: &AuthUser, reason: &str) {
    state.realtime.to_user(
        &user.id,
        "delayed_message_failed",
        json!({
            "messageId": message_id,
            "agentId": user.id,
            "reason": reason,
            "operation": "recall",
            "failedAt": now_iso(),
            "deliveryStatus": "failed",
            "priority": "high",
        }),
    );
}

#[derive(Deserialize)]
pub struct LegacyPendingQuery {
    pub page: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
}

pub async fn legacy_pending(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<LegacyPendingQuery>,
) -> Result {
    let page = q.page.unwrap_or(1);
    let size = q.page_size.unwrap_or(20);
    if page < 1 || !(1..=100).contains(&size) {
        return Err(AppError::BadRequest("Invalid pagination parameters".into()));
    }
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM scheduled_messages WHERE agent_id = ? AND status = 'pending'",
    )
    .bind(&user.id)
    .fetch_one(&state.db)
    .await?;
    type PendingRow = (String, String, String, Option<String>, String, Option<String>, String, Option<String>, Option<String>, Option<String>);
    let rows: Vec<PendingRow> = sqlx::query_as(
        "SELECT sm.id, sm.conversation_id, sm.agent_id, sm.content, sm.content_type,
                sm.scheduled_at, sm.status, sm.metadata, sm.created_at, cu.display_name
         FROM scheduled_messages sm
         LEFT JOIN conversations c ON c.id = sm.conversation_id
         LEFT JOIN customers cu ON cu.id = c.customer_id
         WHERE sm.agent_id = ? AND sm.status = 'pending'
         ORDER BY sm.scheduled_at ASC LIMIT ? OFFSET ?",
    )
    .bind(&user.id)
    .bind(size)
    .bind((page - 1) * size)
    .fetch_all(&state.db)
    .await?;
    let now = now_iso();
    let items: Vec<Value> = rows
        .iter()
        .map(|(id, conv, sender, content, mtype, at, status, meta, created, customer)| {
            json!({
                "messageId": id,
                "conversationId": conv,
                "senderId": sender,
                "content": content,
                "messageType": mtype,
                "scheduledSendTime": at,
                "status": status,
                "metadata": meta.as_deref().and_then(|m| serde_json::from_str::<Value>(m).ok()),
                "createdAt": created,
                "customerName": customer,
                "canRecall": at.as_deref().map(|a| now.as_str() < a).unwrap_or(false),
            })
        })
        .collect();
    Ok(envelope::ok(json!({
        "messages": items,
        "total": total,
        "page": page,
        "pageSize": size,
    })))
}

#[derive(Deserialize)]
pub struct RescheduleBody {
    #[serde(rename = "delaySeconds")]
    pub delay_seconds: Option<i64>,
}

pub async fn legacy_reschedule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(message_id): Path<String>,
    Json(body): Json<RescheduleBody>,
) -> Result {
    let delay = body
        .delay_seconds
        .ok_or_else(|| AppError::BadRequest("delaySeconds is required".into()))?;
    if !(MIN_DELAY..=MAX_DELAY).contains(&delay) {
        return Err(AppError::BadRequest("delaySeconds must be between 1 and 120".into()));
    }
    type RescheduleRow = (String, String, String, Option<String>, String, Option<String>);
    let row: Option<RescheduleRow> =
        sqlx::query_as(
            "SELECT agent_id, status, conversation_id, content, content_type, metadata
             FROM scheduled_messages WHERE id = ?",
        )
        .bind(&message_id)
        .fetch_optional(&state.db)
        .await?;
    let Some((sender, status, conversation_id, content, mtype, metadata)) = row else {
        return Err(AppError::BadRequest("Message not found".into()));
    };
    if status != "pending" {
        return Err(AppError::BadRequest("Message cannot be rescheduled".into()));
    }
    if sender != user.id {
        return Err(AppError::BadRequest("Permission denied".into()));
    }
    let mut meta: serde_json::Map<String, Value> = metadata
        .as_deref()
        .and_then(|m| serde_json::from_str(m).ok())
        .unwrap_or_default();
    let platform = meta.get("platform").and_then(Value::as_str).unwrap_or("").to_string();
    if !["line", "facebook"].contains(&platform.as_str()) {
        return Err(AppError::BadRequest(
            "Message platform does not support rescheduling".into(),
        ));
    }
    let new_fire = (chrono::Utc::now() + chrono::Duration::seconds(delay))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    meta.insert("rescheduledBy".into(), json!(user.id));
    meta.insert("rescheduledDelaySeconds".into(), json!(delay));
    sqlx::query(
        "UPDATE scheduled_messages SET scheduled_at = ?, metadata = ?, updated_at = ?
         WHERE id = ? AND status = 'pending'",
    )
    .bind(&new_fire)
    .bind(Value::Object(meta).to_string())
    .bind(now_iso())
    .bind(&message_id)
    .execute(&state.db)
    .await?;
    state
        .recallable_messages
        .mark(&message_id, std::time::Duration::from_secs(delay as u64 + 60));
    broadcast_countdown(&state, &user, Countdown {
        conversation_id: &conversation_id, message_id: &message_id,
        content: content.as_deref().unwrap_or(""), message_type: &mtype,
        platform: &platform, delay_seconds: delay, scheduled_at: &new_fire,
    });
    Ok(envelope::ok(json!({ "messageId": message_id, "newSendTime": new_fire })))
}

//! Delayed-send, recall, and offline-buffer service capabilities
//! (CRD 983-1006, 1014-1018, 1038).
//!
//! These are invoked programmatically (not HTTP routes); every operation
//! returns a JSON result object with a `success` flag so callers (and tests)
//! observe the documented outcome shapes. Validation failures and runtime
//! failures both surface as `success: false` with a distinguishing `error`.

use serde_json::{json, Value};
use sqlx::PgPool;

use crate::domain::conversations::channels::{OutboundGateway, OutboundItem};
use crate::state::AppState;

use super::store::RECALL_PLACEHOLDER;

pub const MIN_DELAY_SECS: i64 = 1;
pub const MAX_DELAY_SECS: i64 = 120;
/// Recallability marker lives slightly beyond the delay (CRD 987).
const MARKER_GRACE_SECS: u64 = 5;
/// Recall window granted to a delayed webchat message once sent (CRD 993).
const SENT_RECALL_WINDOW_MINS: i64 = 30;

fn iso_in(seconds: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(seconds))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

async fn write_recall_log(db: &PgPool, message_id: &str, user_id: &str, action: &str) {
    if let Err(error) = sqlx::query(
        "INSERT INTO message_recall_logs (message_id, agent_id, action, created_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(message_id)
    .bind(user_id)
    .bind(action)
    .bind(crate::db::now_iso())
    .execute(db)
    .await
    {
        tracing::warn!(error = %error, message_id, user_id, action, "message recall log write failed");
    }
}

// ------------------------------------------------- Schedule delayed message (CRD 983-989)

pub struct ScheduleParams {
    pub conversation_id: String,
    pub content: String,
    pub delay_seconds: i64,
    pub message_type: Option<String>,
    pub recipient_id: Option<String>,
    pub platform: Option<String>,
    pub media_url: Option<String>,
    pub metadata: Option<Value>,
}

pub async fn schedule_delayed(state: &AppState, agent_id: &str, p: ScheduleParams) -> Value {
    if !(MIN_DELAY_SECS..=MAX_DELAY_SECS).contains(&p.delay_seconds) {
        return json!({
            "success": false,
            "error": format!("Delay must be between {MIN_DELAY_SECS} and {MAX_DELAY_SECS} seconds"),
        });
    }
    let exists: Option<String> = match sqlx::query_scalar(
        "SELECT id FROM conversations WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(&p.conversation_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(e) => return json!({ "success": false, "error": e.to_string() }),
    };
    if exists.is_none() {
        return json!({ "success": false, "error": "Conversation not found" });
    }

    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::db::now_iso();
    let scheduled_at = iso_in(p.delay_seconds);
    let mut metadata = match p.metadata {
        Some(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    metadata.insert("recipientId".into(), json!(p.recipient_id));
    metadata.insert(
        "platform".into(),
        json!(p.platform.as_deref().unwrap_or("webchat")),
    );
    metadata.insert("delaySeconds".into(), json!(p.delay_seconds));
    metadata.insert("mediaUrl".into(), json!(p.media_url));

    let inserted = sqlx::query(
        "INSERT INTO scheduled_messages
             (id, conversation_id, agent_id, content, content_type, scheduled_at, status,
              metadata, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8)",
    )
    .bind(&id)
    .bind(&p.conversation_id)
    .bind(agent_id)
    .bind(&p.content)
    .bind(p.message_type.as_deref().unwrap_or("text"))
    .bind(&scheduled_at)
    .bind(Value::Object(metadata).to_string())
    .bind(&now)
    .execute(&state.db)
    .await;
    if let Err(e) = inserted {
        return json!({ "success": false, "error": e.to_string() });
    }

    // Fast-lookup recallability marker, expiring slightly past the delay
    // (CRD 987); the scheduled time doubles as the recall deadline.
    state.recallable_messages.mark(
        &id,
        std::time::Duration::from_secs(p.delay_seconds as u64 + MARKER_GRACE_SECS),
    );
    json!({
        "success": true,
        "delayedMessageId": id,
        "scheduledSendTime": scheduled_at,
        "recallDeadline": scheduled_at,
    })
}

// ------------------------------------------- Process / dispatch when due (CRD 991-994)

#[derive(sqlx::FromRow)]
struct DelayedRow {
    id: String,
    conversation_id: String,
    agent_id: String,
    content: Option<String>,
    content_type: String,
    scheduled_at: Option<String>,
    status: String,
    metadata: Option<String>,
}

async fn mark_delayed_failed(db: &PgPool, row: &DelayedRow, reason: &str) {
    let mut metadata = super::store::metadata_map(&row.metadata);
    metadata.insert("failureReason".into(), json!(reason));
    if let Err(error) = sqlx::query(
        "UPDATE scheduled_messages SET status = 'failed', metadata = $1, updated_at = $2 WHERE id = $3",
    )
    .bind(Value::Object(metadata).to_string())
    .bind(crate::db::now_iso())
    .bind(&row.id)
    .execute(db)
    .await
    {
        tracing::warn!(error = %error, delayed_message_id = %row.id, "scheduled message failure-state update failed");
    }
}

pub async fn process_delayed(state: &AppState, delayed_id: &str) -> Value {
    let row: Option<DelayedRow> = match sqlx::query_as(
        "SELECT id, conversation_id, agent_id, content, content_type, scheduled_at, status, metadata
         FROM scheduled_messages WHERE id = $1",
    )
    .bind(delayed_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(e) => return json!({ "success": false, "error": e.to_string() }),
    };
    let Some(row) = row else {
        return json!({ "success": false, "error": "Delayed message not found" });
    };
    if row.status != "pending" {
        return json!({
            "success": false,
            "error": format!("Delayed message is not pending (status: {})", row.status),
            "status": row.status,
        });
    }
    let now = crate::db::now_iso();
    if let Some(at) = &row.scheduled_at {
        if now.as_str() < at.as_str() {
            // Too early relative to the scheduled time: signal a re-schedule
            // rather than sending (CRD 993).
            return json!({ "success": false, "reschedule": true, "scheduledSendTime": at });
        }
    }

    let metadata = super::store::metadata_map(&row.metadata);
    let platform = metadata
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or("webchat")
        .to_string();
    let recipient = metadata
        .get("recipientId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let content = row.content.clone().unwrap_or_default();

    let result = match platform.as_str() {
        // External platforms dispatch through the platform gateway (CRD 993).
        // Missing credentials or gateway errors mark the item failed, matching
        // the documented pending -> failed outcome.
        "line" | "facebook" | "instagram" | "shopee" => {
            let gateway = OutboundGateway::from_state(state);
            match gateway
                .send_batch(&platform, &recipient, &[OutboundItem::text(content)])
                .await
            {
                Ok(platform_message_id) => Ok(json!({
                    "success": true,
                    "delayedMessageId": row.id,
                    "platform": platform,
                    "platformMessageId": platform_message_id,
                })),
                Err(e) => Err(e.to_string()),
            }
        }
        // In-app webchat: create the real persisted message, sent with a
        // 30-minute recall window (CRD 993).
        "webchat" => {
            let message_id = super::store::new_message_id();
            let recall_deadline = iso_in(SENT_RECALL_WINDOW_MINS * 60);
            let insert = sqlx::query(
                "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content,
                                       content_type, is_sent, sent_at, delivery_status,
                                       recall_deadline, sender_name, created_at)
                 SELECT $1, $2, 'agent', $3, $4, $5, 1, $6, 'sent', $7, a.display_name, $8
                 FROM agents a WHERE a.id = $9",
            )
            .bind(&message_id)
            .bind(&row.conversation_id)
            .bind(&row.agent_id)
            .bind(&content)
            .bind(&row.content_type)
            .bind(&now)
            .bind(&recall_deadline)
            .bind(&now)
            .bind(&row.agent_id)
            .execute(&state.db)
            .await;
            match insert {
                Ok(_) => {
                    let _ = super::store::touch_conversation(&state.db, &row.conversation_id, &now)
                        .await;
                    // Realtime: push the created webchat message to the
                    // conversation's live audience (CRD 3450, 3452);
                    // best-effort only.
                    let payload = json!({
                        "messageId": message_id,
                        "conversationId": row.conversation_id,
                        "content": content,
                        "messageType": row.content_type,
                        "senderType": "agent",
                        "senderId": row.agent_id,
                        "deliveryStatus": "sent",
                        "delayed": true,
                        "timestamp": now,
                    });
                    state.realtime.to_conversation_message(
                        &row.conversation_id,
                        "message_sent",
                        payload.clone(),
                    );
                    crate::realtime::broadcaster::publish_remote_event(
                        state,
                        "message_sent",
                        payload,
                        vec![json!({ "type": "conversation", "ids": [&row.conversation_id] })],
                        "high",
                    )
                    .await;
                    // Delayed-message terminal events also reach the
                    // originating agent's personal channel (CRD 3452).
                    state.realtime.to_user(
                        &row.agent_id,
                        "delayed_message_sent",
                        json!({
                            "delayedMessageId": row.id,
                            "messageId": message_id,
                            "conversationId": row.conversation_id,
                            "persistent": true,
                            "timestamp": now,
                        }),
                    );
                    Ok(json!({
                        "success": true,
                        "delayedMessageId": row.id,
                        "messageId": message_id,
                        "recallDeadline": recall_deadline,
                    }))
                }
                Err(e) => Err(e.to_string()),
            }
        }
        other => Err(format!("Unsupported platform: {other}")),
    };

    match result {
        Ok(payload) => {
            let updated = sqlx::query(
                "UPDATE scheduled_messages SET status = 'sent', sent_at = $1, updated_at = $2 WHERE id = $3",
            )
            .bind(&now)
            .bind(&now)
            .bind(&row.id)
            .execute(&state.db)
            .await;
            if let Err(e) = updated {
                return json!({ "success": false, "error": e.to_string() });
            }
            state.recallable_messages.clear(&row.id);
            payload
        }
        Err(reason) => {
            mark_delayed_failed(&state.db, &row, &reason).await;
            json!({ "success": false, "error": reason })
        }
    }
}

// --------------------------------------- Cancel / recall a pending item (CRD 996-1000)

/// `as_recall = true` records a recall-log entry alongside the cancellation
/// (CRD 999).
pub async fn cancel_delayed(
    state: &AppState,
    delayed_id: &str,
    user_id: &str,
    reason: Option<&str>,
    as_recall: bool,
) -> Value {
    let row: Option<DelayedRow> = match sqlx::query_as(
        "SELECT id, conversation_id, agent_id, content, content_type, scheduled_at, status, metadata
         FROM scheduled_messages WHERE id = $1",
    )
    .bind(delayed_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(e) => return json!({ "success": false, "error": e.to_string() }),
    };
    let Some(row) = row else {
        return json!({ "success": false, "error": "Delayed message not found" });
    };
    if row.status != "pending" {
        return json!({
            "success": false,
            "error": format!("Cannot cancel: message is {}", row.status),
            "status": row.status,
        });
    }
    let now = crate::db::now_iso();
    if let Some(at) = &row.scheduled_at {
        if now.as_str() >= at.as_str() {
            return json!({
                "success": false,
                "error": "Cannot recall after scheduled send time",
            });
        }
    }

    let mut metadata = super::store::metadata_map(&row.metadata);
    metadata.insert(
        if as_recall {
            "recallReason"
        } else {
            "cancelReason"
        }
        .into(),
        json!(reason.unwrap_or("Cancelled by user")),
    );
    let updated = sqlx::query(
        "UPDATE scheduled_messages
            SET status = 'cancelled', cancelled_at = $1, metadata = $2, updated_at = $3
          WHERE id = $4 AND status = 'pending'",
    )
    .bind(&now)
    .bind(Value::Object(metadata).to_string())
    .bind(&now)
    .bind(&row.id)
    .execute(&state.db)
    .await;
    if let Err(e) = updated {
        return json!({ "success": false, "error": e.to_string() });
    }
    state.recallable_messages.clear(&row.id);
    if as_recall {
        write_recall_log(&state.db, &row.id, user_id, "successful").await;
    }
    json!({ "success": true, "delayedMessageId": row.id, "cancelledAt": now })
}

// ------------------------------------- Recall an already-sent message (CRD 1002-1006)

pub async fn recall_sent_message(state: &AppState, message_id: &str, user_id: &str) -> Value {
    // (conversationId, isRecalled, recallDeadline, platformMessageId, platform, recipient)
    type RecallTarget = (
        String,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<RecallTarget> = match sqlx::query_as(
        "SELECT m.conversation_id, m.is_recalled, m.recall_deadline, m.platform_message_id,
                    cu.platform, cu.platform_user_id
             FROM messages m
             LEFT JOIN conversations c ON c.id = m.conversation_id
             LEFT JOIN customers cu ON cu.id = c.customer_id
             WHERE m.id = $1 AND m.deleted_at IS NULL",
    )
    .bind(message_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(e) => return json!({ "success": false, "error": e.to_string(), "canRecall": false }),
    };
    let Some((_conversation_id, is_recalled, deadline, platform_message_id, platform, recipient)) =
        row
    else {
        return json!({ "success": false, "error": "Message not found", "canRecall": false });
    };
    if is_recalled != 0 {
        return json!({ "success": false, "error": "Message already recalled", "canRecall": false });
    }
    let now = crate::db::now_iso();
    if let Some(deadline) = &deadline {
        if now.as_str() > deadline.as_str() {
            write_recall_log(&state.db, message_id, user_id, "failed").await;
            return json!({
                "success": false,
                "error": "Recall deadline exceeded",
                "canRecall": false,
            });
        }
    }

    // Observable order (CRD 1005): mark recalled first, log success, then
    // best-effort platform notification that never reverts the recall.
    let updated = sqlx::query(
        "UPDATE messages
            SET is_recalled = 1, recalled_at = $1, content = $2, delivery_status = 'recalled',
                updated_at = $3
          WHERE id = $4",
    )
    .bind(&now)
    .bind(RECALL_PLACEHOLDER)
    .bind(&now)
    .bind(message_id)
    .execute(&state.db)
    .await;
    if let Err(e) = updated {
        write_recall_log(&state.db, message_id, user_id, "failed").await;
        return json!({ "success": false, "error": e.to_string(), "canRecall": false });
    }
    write_recall_log(&state.db, message_id, user_id, "successful").await;

    let gateway = OutboundGateway::from_state(state);
    match platform.as_deref() {
        Some("line") => {
            // LINE offers no native unsend through its API: send a
            // customer-facing recall notice instead (CRD 1005). Failure is
            // swallowed and never reverts the database recall.
            let _ = gateway
                .send_batch(
                    "line",
                    recipient.as_deref().unwrap_or_default(),
                    &[OutboundItem::text("This message has been recalled")],
                )
                .await;
        }
        Some("facebook") => {
            if let Some(platform_message_id) = platform_message_id.as_deref() {
                if let Err(error) = gateway
                    .delete_message("facebook", platform_message_id)
                    .await
                {
                    tracing::warn!(
                        error = %error,
                        message_id,
                        platform_message_id,
                        "facebook platform delete failed after local recall"
                    );
                }
            }
        }
        _ => {} // Other platforms are skipped (CRD 1005).
    }

    json!({ "success": true, "messageId": message_id, "recalledAt": now, "canRecall": true })
}

/// Dispatch every pending delayed item whose scheduled send time has arrived
/// (CRD 991: "triggered when a scheduled send becomes due"). Returns the number
/// of items processed. The binary drives this from a timer loop; tests invoke
/// it (or [`process_delayed`]) directly.
pub async fn dispatch_due(state: &AppState) -> usize {
    let due: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM scheduled_messages
         WHERE status = 'pending' AND scheduled_at IS NOT NULL AND scheduled_at <= $1
         ORDER BY scheduled_at",
    )
    .bind(crate::db::now_iso())
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let count = due.len();
    for id in due {
        let _ = process_delayed(state, &id).await;
    }
    count
}

/// Retire stale failed delayed items older than one day: failed -> archived
/// (CRD 1014, 1028).
pub async fn archive_stale_failed(db: &PgPool) -> Result<u64, sqlx::Error> {
    let cutoff = iso_in(-24 * 3600);
    let res = sqlx::query(
        "UPDATE scheduled_messages SET status = 'archived', updated_at = $1
         WHERE status = 'failed' AND COALESCE(updated_at, created_at) < $2",
    )
    .bind(crate::db::now_iso())
    .bind(cutoff)
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}

// ----------------------------------------- Offline message buffering (CRD 1018, 1038)

pub const BUFFER_RETENTION_DAYS: i64 = 7;
/// Brief retention once delivered (CRD 1018).
pub const DELIVERED_RETENTION_SECS: i64 = 3600;
pub const MAX_BUFFER_RETRIES: i64 = 3;

/// Buffer a real-time message for an offline recipient.
pub async fn buffer_message(
    db: &PgPool,
    recipient_id: &str,
    conversation_id: &str,
    message_id: &str,
    payload: &Value,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO offline_message_buffer
             (id, message_id, recipient_id, conversation_id, payload, buffered_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(message_id)
    .bind(recipient_id)
    .bind(conversation_id)
    .bind(payload.to_string())
    .bind(crate::db::now_iso())
    .bind(iso_in(BUFFER_RETENTION_DAYS * 24 * 3600))
    .execute(db)
    .await?;
    Ok(id)
}

#[derive(sqlx::FromRow)]
pub struct BufferedEntry {
    pub id: String,
    pub message_id: Option<String>,
    pub recipient_id: String,
    pub conversation_id: Option<String>,
    pub payload: Option<String>,
    pub buffered_at: String,
    pub delivered: i64,
    pub retry_count: i64,
    pub expires_at: String,
}

/// Replay buffered messages on reconnect: undelivered by default, optionally
/// including already-delivered ones (CRD 1038).
pub async fn replay_buffered(
    db: &PgPool,
    recipient_id: &str,
    include_delivered: bool,
) -> Result<Vec<BufferedEntry>, sqlx::Error> {
    let sql = format!(
        "SELECT id, message_id, recipient_id, conversation_id, payload, buffered_at,
                delivered, retry_count, expires_at
         FROM offline_message_buffer
         WHERE recipient_id = $1 AND expires_at > $2 {}
         ORDER BY buffered_at, id",
        if include_delivered {
            ""
        } else {
            "AND delivered = 0"
        }
    );
    sqlx::query_as(&crate::db::pg_params(&sql))
        .bind(recipient_id)
        .bind(crate::db::now_iso())
        .fetch_all(db)
        .await
}

/// Idempotent delivery marking for one or many entries (CRD 1038); a delivered
/// entry is retained only briefly.
pub async fn mark_delivered(db: &PgPool, ids: &[String]) -> Result<u64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "UPDATE offline_message_buffer SET delivered = 1, delivered_at = $1, expires_at = $2
         WHERE id IN ({placeholders}) AND delivered = 0"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query(&sql)
        .bind(crate::db::now_iso())
        .bind(iso_in(DELIVERED_RETENTION_SECS));
    for id in ids {
        q = q.bind(id);
    }
    Ok(q.execute(db).await?.rows_affected())
}

/// Bump the retry counter on undelivered entries; entries that exceed the
/// retry maximum are dropped (CRD 1018). Returns (retried, dropped).
pub async fn retry_undelivered(db: &PgPool, recipient_id: &str) -> Result<(u64, u64), sqlx::Error> {
    let now = crate::db::now_iso();
    let retried = sqlx::query(
        "UPDATE offline_message_buffer SET retry_count = retry_count + 1
         WHERE recipient_id = $1 AND delivered = 0 AND expires_at > $2",
    )
    .bind(recipient_id)
    .bind(&now)
    .execute(db)
    .await?
    .rows_affected();
    let dropped = sqlx::query(
        "DELETE FROM offline_message_buffer
         WHERE recipient_id = $1 AND delivered = 0 AND retry_count > $2",
    )
    .bind(recipient_id)
    .bind(MAX_BUFFER_RETRIES)
    .execute(db)
    .await?
    .rows_affected();
    Ok((retried - dropped.min(retried), dropped))
}

/// Per-recipient buffer statistics: total, delivered, pending, expired
/// (CRD 1038).
pub async fn buffer_stats(db: &PgPool, recipient_id: &str) -> Result<Value, sqlx::Error> {
    let now = crate::db::now_iso();
    let (total, delivered, pending, expired): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN delivered = 1 THEN 1 ELSE 0 END), 0)::bigint,
                COALESCE(SUM(CASE WHEN delivered = 0 AND expires_at > $1 THEN 1 ELSE 0 END), 0)::bigint,
                COALESCE(SUM(CASE WHEN expires_at <= $2 THEN 1 ELSE 0 END), 0)::bigint
         FROM offline_message_buffer WHERE recipient_id = $3",
    )
    .bind(&now)
    .bind(&now)
    .bind(recipient_id)
    .fetch_one(db)
    .await?;
    Ok(json!({ "total": total, "delivered": delivered, "pending": pending, "expired": expired }))
}

/// Purge entries past their expiry (CRD 1018).
pub async fn purge_expired(db: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM offline_message_buffer WHERE expires_at <= $1")
            .bind(crate::db::now_iso())
            .execute(db)
            .await?
            .rows_affected(),
    )
}

// NOTE(scale-out): real-time fan-out batching (CRD 1040, 3465) can group
// outbound events per target in the hub, flush on size threshold / short delay,
// let urgent priority bypass, and collapse duplicate transient events (typing,
// join/leave). The hub currently delivers every event immediately, which the
// spec permits ("non-urgent events MAY be coalesced"); batching is a pure
// delivery-efficiency optimization.

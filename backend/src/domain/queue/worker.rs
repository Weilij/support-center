//! Queue consumer (CRD 5150-5194): batches of up to 10 jobs (5s window),
//! independent per-job processing, at-least-once delivery with progressive
//! backoff, dead-letter routing after 3 attempts.

use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;

use crate::db::now_iso;
use crate::domain::conversations::channels::{
    OutboundError, OutboundGateway, OutboundItem, BATCH_CAP,
};
use crate::state::AppState;

use super::{is_retryable, retry_delay_ms, Job, MAX_BATCH_SIZE, MAX_BATCH_WAIT, MAX_RETRIES};

#[derive(Debug, thiserror::Error)]
enum QueueWorkerError {
    #[error("validation: {0}")]
    Validation(&'static str),
    #[error("network: {0}")]
    Network(&'static str),
    #[error(transparent)]
    Delivery(#[from] OutboundError),
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
}

type WorkerResult<T> = std::result::Result<T, QueueWorkerError>;

/// Start the background consumer; call once from the runtime entry point
/// (and from tests that exercise queued work end-to-end).
pub fn spawn(state: Arc<AppState>) {
    let Some(mut rx) = state.queue.take_receiver() else {
        return; // already running
    };
    tokio::spawn(async move {
        loop {
            // Collect a batch: the first job blocks, then whatever is already
            // queued joins it (up to the cap). An empty channel delivers the
            // partial batch immediately — the 5s window only bounds how long
            // a non-empty backlog may accumulate (CRD 5151).
            let Some(first) = rx.recv().await else { break };
            let mut batch = vec![first];
            let deadline = tokio::time::Instant::now() + MAX_BATCH_WAIT;
            while batch.len() < MAX_BATCH_SIZE && tokio::time::Instant::now() < deadline {
                match rx.try_recv() {
                    Ok(job) => batch.push(job),
                    Err(_) => break,
                }
            }
            // Jobs are processed independently (CRD 5158).
            for job in batch {
                process_job(&state, job).await;
            }
        }
    });
}

async fn process_job(state: &Arc<AppState>, job: Job) {
    let started = Instant::now();
    let kind = job
        .body
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("outbound_message");
    let result = if kind == "media_processing" {
        process_media(state, &job.body).await
    } else {
        // No discriminator is treated as outbound (CRD 5124).
        process_outbound(state, &job.body).await
    };
    let elapsed = started.elapsed().as_millis();

    match result {
        Ok(()) => state.queue.record(true, false, elapsed),
        Err(error) => {
            let error_text = error.to_string();
            let category = super::categorize(&error_text);
            let retry = job.attempt + 1 < MAX_RETRIES && is_retryable(category);
            state.queue.record(false, retry, elapsed);
            if retry {
                let delay = retry_delay_ms(job.attempt);
                let queue_state = state.clone();
                let next = Job {
                    body: job.body,
                    attempt: job.attempt + 1,
                };
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    queue_state.queue.requeue(next);
                });
            } else {
                tracing::warn!(error = %error, category, "job exhausted retries; dead-lettered");
                if let Ok(mut dlq) = state.queue.dead_letter.lock() {
                    dlq.push(job);
                }
            }
        }
    }
}

/// Recognized file-description placeholders are never sent as visible text
/// (CRD 5165).
fn is_placeholder(content: &str) -> bool {
    let c = content.trim();
    c.starts_with("Sent a file:")
        || (c.starts_with("Sent ") && c.ends_with(" files"))
        || (c.starts_with('[') && c.ends_with(']'))
        || (c.starts_with('（') && c.ends_with('）'))
}

async fn process_outbound(state: &Arc<AppState>, body: &Value) -> WorkerResult<()> {
    let message_id = body
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let conversation_id = body
        .get("conversationId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let recipient = body
        .get("recipientId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content = body
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let retry_count = body
        .get("metadata")
        .and_then(|m| m.get("retryCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    // 1. Assemble the platform message set (CRD 5165).
    let mut items: Vec<OutboundItem> = Vec::new();
    if !content.is_empty() && !is_placeholder(content) {
        items.push(OutboundItem::text(content.to_string()));
    }
    for att in body
        .get("attachments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = att.get("type").and_then(Value::as_str).unwrap_or("file");
        let url = att.get("url").and_then(Value::as_str).unwrap_or_default();
        if kind == "image" {
            items.push(OutboundItem::text(format!("[Image] {url}")));
        } else {
            let name = att
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("file");
            let mime = att.get("mimeType").and_then(Value::as_str).unwrap_or("");
            let size = att.get("size").and_then(Value::as_u64).unwrap_or(0);
            items.push(OutboundItem::text(format!(
                "[File] {name} ({mime}, {size} bytes) {url}"
            )));
        }
    }
    // 2. Empty set is a delivery failure (CRD 5166).
    let outcome = if items.is_empty() {
        Err(QueueWorkerError::Validation("no messages to send"))
    } else {
        // 3. Chunks of 5 with a brief pause; any chunk failure fails the send.
        let gateway = OutboundGateway::from_config(&state.config);
        let mut result: WorkerResult<()> = Ok(());
        for (i, chunk) in items.chunks(BATCH_CAP).enumerate() {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if let Err(e) = gateway.send_batch("line", recipient, chunk).await {
                result = Err(e.into());
                break;
            }
        }
        result
    };

    match outcome {
        Ok(()) => {
            // 4. Best-effort delivered-state write + success broadcast (CRD 5168).
            if let Err(error) = sqlx::query(
                "UPDATE messages SET delivery_status = 'delivered', updated_at = $1 WHERE id = $2",
            )
            .bind(now_iso())
            .bind(message_id)
            .execute(&state.db)
            .await
            {
                tracing::warn!(error = %error, message_id, "queue delivered-state update failed");
            }
            let payload = json!({
                    "messageId": message_id,
                    "conversationId": conversation_id,
                    "success": true,
                    "deliveredAt": now_iso(),
            });
            state.realtime.to_conversation(
                conversation_id,
                "message_delivery_status",
                payload.clone(),
            );
            crate::realtime::broadcaster::publish_remote_event(
                state,
                "message_delivery_status",
                payload,
                vec![json!({ "type": "conversation", "ids": [conversation_id] })],
                "high",
            )
            .await;
            Ok(())
        }
        Err(reason) => {
            let reason_text = reason.to_string();
            // 5. Failure broadcast with reason + retry counter (CRD 5169).
            if let Err(error) = sqlx::query(
                "UPDATE messages SET delivery_status = 'failed', updated_at = $1 WHERE id = $2",
            )
            .bind(now_iso())
            .bind(message_id)
            .execute(&state.db)
            .await
            {
                tracing::warn!(error = %error, message_id, "queue failed-state update failed");
            }
            let payload = json!({
                    "messageId": message_id,
                    "conversationId": conversation_id,
                    "success": false,
                    "error": reason_text,
                    "retryCount": retry_count,
            });
            state.realtime.to_conversation(
                conversation_id,
                "message_delivery_status",
                payload.clone(),
            );
            crate::realtime::broadcaster::publish_remote_event(
                state,
                "message_delivery_status",
                payload,
                vec![json!({ "type": "conversation", "ids": [conversation_id] })],
                "high",
            )
            .await;
            Err(reason)
        }
    }
}

async fn process_media(state: &Arc<AppState>, body: &Value) -> WorkerResult<()> {
    let message_id = body
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let conversation_id = body
        .get("conversationId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let platform_message_id = body
        .get("platformMessageId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let media_type = body
        .get("mediaType")
        .and_then(Value::as_str)
        .unwrap_or("file");
    let filename = body.get("fileName").and_then(Value::as_str);
    let team_id = body.get("teamId").and_then(Value::as_i64);

    // Idempotency: an attachment for this message+key means a retry already
    // succeeded (duplicate records are avoided, CRD 3139-style).
    let key = format!("line/media/{platform_message_id}");
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM attachments WHERE message_id = $1 AND storage_key = $2")
            .bind(message_id)
            .bind(&key)
            .fetch_optional(&state.db)
            .await?;
    if existing.is_some() {
        return Ok(());
    }

    // 1. Fetch + store the media (CRD 5176). Prefer an already mirrored object;
    // on miss, fetch from the LINE content API and cache the bytes for future
    // retries/proxy reads.
    let bytes = if let Some(bytes) =
        crate::domain::files::store::get_object(&state.config.upload_dir, &key).await
    {
        bytes
    } else if let Some(token) = state
        .config
        .line_channel_access_token
        .as_deref()
        .filter(|token| !token.is_empty())
    {
        let Some((bytes, _content_type)) =
            crate::domain::conversations::channels::fetch_line_media_from_base(
                &state.config.line_content_api_base_url,
                token,
                platform_message_id,
                false,
            )
            .await
        else {
            return Err(QueueWorkerError::Network(
                "media download yielded no attachment",
            ));
        };
        if let Err(error) =
            crate::domain::files::store::put_object(&state.config.upload_dir, &key, &bytes).await
        {
            tracing::warn!(error = %error, platform_message_id, "queue media cache write failed");
        }
        bytes
    } else {
        return Err(QueueWorkerError::Network(
            "media download yielded no attachment",
        ));
    };

    let attachment_id = uuid::Uuid::new_v4().to_string();
    let content_type = match media_type {
        "image" => "image/jpeg",
        "video" => "video/mp4",
        "audio" => "audio/mpeg",
        _ => "application/octet-stream",
    };
    sqlx::query(
        "INSERT INTO attachments
            (id, message_id, conversation_id, file_name, content_type, file_size, storage_key,
             upload_status, platform, file_type, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'completed', 'line', $8, $9, $10)",
    )
    .bind(&attachment_id)
    .bind(message_id)
    .bind(conversation_id)
    .bind(filename.unwrap_or(platform_message_id))
    .bind(content_type)
    .bind(bytes.len() as i64)
    .bind(&key)
    .bind(media_type)
    .bind(now_iso())
    .bind(now_iso())
    .execute(&state.db)
    .await?;

    // 3. High-priority global "message updated" broadcast (CRD 5178).
    let attachment = json!({
        "id": attachment_id,
        "messageId": message_id,
        "type": media_type,
        "contentType": content_type,
        "size": bytes.len(),
    });
    let payload = json!({
        "conversationId": conversation_id,
        "messageId": message_id,
        "attachment": attachment,
        "priority": "high",
    });
    match team_id {
        Some(t) => {
            state
                .realtime
                .to_team(t, "message_updated", payload.clone());
            crate::realtime::broadcaster::publish_remote_event(
                state,
                "message_updated",
                payload.clone(),
                vec![json!({ "type": "team", "ids": [t] })],
                "high",
            )
            .await;
        }
        None => {
            state.realtime.global("message_updated", payload.clone());
            crate::realtime::broadcaster::publish_remote_event(
                state,
                "message_updated",
                payload.clone(),
                vec![json!({ "type": "global" })],
                "high",
            )
            .await;
        }
    };
    // 4. Best-effort per-conversation notification (CRD 5179).
    state
        .realtime
        .to_conversation(conversation_id, "message_updated", payload.clone());
    crate::realtime::broadcaster::publish_remote_event(
        state,
        "message_updated",
        payload,
        vec![json!({ "type": "conversation", "ids": [conversation_id] })],
        "high",
    )
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::QueueWorkerError;

    #[test]
    fn worker_error_text_preserves_retry_taxonomy_terms() {
        assert_eq!(
            QueueWorkerError::Validation("no messages to send").to_string(),
            "validation: no messages to send"
        );
        assert_eq!(
            QueueWorkerError::Network("media download yielded no attachment").to_string(),
            "network: media download yielded no attachment"
        );
        assert_eq!(
            crate::domain::queue::categorize(
                &QueueWorkerError::Validation("no messages to send").to_string(),
            ),
            "validation"
        );
        assert_eq!(
            crate::domain::queue::categorize(
                &QueueWorkerError::Network("media download yielded no attachment").to_string(),
            ),
            "network"
        );
    }
}

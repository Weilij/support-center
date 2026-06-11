//! Outbound platform delivery (CRD 765-773): the send endpoint persists a pending
//! message and returns immediately; this module performs the background delivery
//! that drives the observable pending -> sent | partial | failed transitions.
//!
//! The real platform calls are stubbed behind [`ChannelGateway`]; the stub
//! reproduces the documented observable outcome: only the platform with full
//! outbound support (LINE) delivers, all others remain effectively undelivered
//! (CRD 773).

use sqlx::SqlitePool;

/// One outbound unit: the text body or one attachment reference.
pub struct OutboundItem {
    pub content: String,
}

/// The downstream platform's per-call message cap (LINE push cap, CRD 769).
pub const BATCH_CAP: usize = 5;

pub trait ChannelGateway: Send + Sync {
    /// Push one batch (at most [`BATCH_CAP`] items) to the platform.
    /// Returns the platform-side message id on success.
    fn send_batch(
        &self,
        platform: &str,
        recipient: &str,
        items: &[OutboundItem],
    ) -> Result<String, String>;
}

/// Stub gateway recording the observable side effect without any network call.
pub struct StubGateway;

impl ChannelGateway for StubGateway {
    fn send_batch(
        &self,
        platform: &str,
        _recipient: &str,
        _items: &[OutboundItem],
    ) -> Result<String, String> {
        // TODO(channels): replace with the real LINE Messaging API push call
        // (and future platform integrations). Per CRD 773 only LINE has full
        // outbound support; other platforms remain effectively undelivered.
        match platform {
            "line" => Ok(format!("stub-line-{}", uuid::Uuid::new_v4())),
            other => Err(format!("Outbound delivery is not supported for platform '{other}'")),
        }
    }
}

/// Background delivery task (fire-and-forget from the send handler, CRD 769-773):
/// batches the items to the platform cap, then persists the final sent flag,
/// delivery status (sent / partial / failed), and platform message id.
pub async fn deliver_pending(
    db: SqlitePool,
    hub: std::sync::Arc<crate::realtime::RealtimeHub>,
    conversation_id: String,
    message_id: String,
    platform: String,
    recipient: String,
    items: Vec<OutboundItem>,
) {
    let gateway = StubGateway;
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut platform_message_id: Option<String> = None;
    let mut last_error: Option<String> = None;

    for batch in items.chunks(BATCH_CAP) {
        match gateway.send_batch(&platform, &recipient, batch) {
            Ok(id) => {
                succeeded += 1;
                platform_message_id.get_or_insert(id);
            }
            Err(e) => {
                failed += 1;
                last_error = Some(e);
            }
        }
    }

    // Partial success: some but not all platform batches succeeded (CRD 773).
    let status = if succeeded > 0 && failed == 0 {
        "sent"
    } else if succeeded > 0 {
        "partial"
    } else {
        "failed"
    };
    let is_sent = succeeded > 0;
    let now = crate::db::now_iso();
    let result = sqlx::query(
        "UPDATE messages
            SET delivery_status = ?, is_sent = ?, platform_message_id = ?,
                sent_at = CASE WHEN ? THEN ? ELSE sent_at END, updated_at = ?
          WHERE id = ?",
    )
    .bind(status)
    .bind(is_sent as i64)
    .bind(&platform_message_id)
    .bind(is_sent as i64)
    .bind(&now)
    .bind(&now)
    .bind(&message_id)
    .execute(&db)
    .await;
    if let Err(e) = result {
        tracing::error!(error = %e, message = %message_id, "failed to persist delivery outcome");
    }

    // Realtime: `message_updated` carrying the final delivery status so
    // clients can transition the message out of the pending state (CRD 827-828,
    // 3450); best-effort only — a broadcast failure never alters the persisted
    // outcome.
    hub.to_conversation(
        &conversation_id,
        "message_updated",
        serde_json::json!({
            "messageId": message_id,
            "conversationId": conversation_id,
            "deliveryStatus": status,
            "isSent": is_sent,
            "platformMessageId": platform_message_id,
            "error": last_error,
            "timestamp": now,
        }),
    );
}

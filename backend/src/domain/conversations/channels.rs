//! Outbound platform delivery (CRD 765-773): the send endpoint persists a pending
//! message and returns immediately; this module performs the background delivery
//! that drives the observable pending -> sent | partial | failed transitions.
//!
//! The real platform calls are stubbed behind [`ChannelGateway`]; the stub
//! reproduces the documented observable outcome: only the platform with full
//! outbound support (LINE) delivers, all others remain effectively undelivered
//! (CRD 773).

use sqlx::PgPool;

/// One outbound unit: the text body or one attachment reference.
pub struct OutboundItem {
    pub content: String,
}

/// The downstream platform's per-call message cap (LINE push cap, CRD 769).
pub const BATCH_CAP: usize = 5;

use serde_json::json;
use std::sync::OnceLock;

/// Shared HTTP client (connection pooling) for all outbound platform calls.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Real LINE Messaging API gateway (global channel access token).
pub struct LineGateway {
    token: String,
}

impl LineGateway {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

/// The outbound message body for a LINE push (pure — unit-tested).
pub fn build_push_body(recipient: &str, items: &[OutboundItem]) -> serde_json::Value {
    json!({
        "to": recipient,
        "messages": items
            .iter()
            .map(|it| json!({ "type": "text", "text": it.content }))
            .collect::<Vec<_>>(),
    })
}

/// Outbound delivery gateway. `Stub` reproduces the documented observable
/// outcome without any network call (dev/tests); `Line` calls the real API.
pub enum OutboundGateway {
    Stub,
    Line(LineGateway),
}

impl OutboundGateway {
    /// Real LINE gateway when the global token is configured; otherwise the stub
    /// (so dev/test runs without a token make no network calls).
    pub fn from_config(config: &crate::config::Config) -> Self {
        match config.line_channel_access_token.as_deref() {
            Some(t) if !t.is_empty() => OutboundGateway::Line(LineGateway::new(t.to_string())),
            _ => OutboundGateway::Stub,
        }
    }

    /// Push one batch (≤ BATCH_CAP items); returns the platform-side message id.
    pub async fn send_batch(
        &self,
        platform: &str,
        recipient: &str,
        items: &[OutboundItem],
    ) -> Result<String, String> {
        match self {
            OutboundGateway::Stub => match platform {
                "line" => Ok(format!("stub-line-{}", uuid::Uuid::new_v4())),
                other => Err(format!("Outbound delivery is not supported for platform '{other}'")),
            },
            OutboundGateway::Line(g) => {
                if platform != "line" {
                    return Err(format!("Outbound delivery is not supported for platform '{platform}'"));
                }
                let body = build_push_body(recipient, items);
                let resp = http_client()
                    .post("https://api.line.me/v2/bot/message/push")
                    .bearer_auth(&g.token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| format!("LINE request failed: {e}"))?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let txt = resp.text().await.unwrap_or_default();
                    return Err(format!("LINE push failed ({status}): {txt}"));
                }
                let v: serde_json::Value = resp.json().await.unwrap_or_else(|_| json!({}));
                let id = v["sentMessages"][0]["id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("line-{}", uuid::Uuid::new_v4()));
                Ok(id)
            }
        }
    }
}

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
    db: PgPool,
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
            SET delivery_status = $1, is_sent = $2, platform_message_id = $3,
                sent_at = CASE WHEN $4::bigint = 1 THEN $5 ELSE sent_at END, updated_at = $6
          WHERE id = $7",
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

#[cfg(test)]
mod gateway_tests {
    use super::*;

    #[test]
    fn push_body_has_to_and_text_messages() {
        let items = vec![
            OutboundItem { content: "hi".into() },
            OutboundItem { content: "bye".into() },
        ];
        let b = build_push_body("U123", &items);
        assert_eq!(b["to"], "U123");
        assert_eq!(b["messages"][0]["type"], "text");
        assert_eq!(b["messages"][0]["text"], "hi");
        assert_eq!(b["messages"][1]["text"], "bye");
        assert_eq!(b["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn from_config_picks_stub_without_token_and_line_with() {
        let mut c = crate::config::test_config();
        c.line_channel_access_token = None;
        assert!(matches!(OutboundGateway::from_config(&c), OutboundGateway::Stub));
        c.line_channel_access_token = Some("tok".into());
        assert!(matches!(OutboundGateway::from_config(&c), OutboundGateway::Line(_)));
    }
}

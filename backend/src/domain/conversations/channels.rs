//! Outbound platform delivery (CRD 765-773): the send endpoint persists a pending
//! message and returns immediately; this module performs the background delivery
//! that drives the observable pending -> sent | partial | failed transitions.
//!
//! Delivery is routed through [`OutboundGateway`], which holds the configured
//! per-platform tokens. With a token, the platform calls its real API (LINE
//! Messaging API push, Facebook Send API); without one, `line` preserves the
//! documented stub success (no network call in dev/tests) and other platforms
//! report "not supported" (CRD 773).

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
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client")
    })
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

/// One Facebook Send-API text message body (pure — unit-tested).
pub fn fb_send_body(recipient: &str, content: &str) -> serde_json::Value {
    json!({
        "recipient": { "id": recipient },
        "messaging_type": "RESPONSE",
        "message": { "text": content },
    })
}

async fn line_push(token: &str, recipient: &str, items: &[OutboundItem]) -> Result<String, String> {
    let body = build_push_body(recipient, items);
    let resp = http_client()
        .post("https://api.line.me/v2/bot/message/push")
        .bearer_auth(token)
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
    Ok(v["sentMessages"][0]["id"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("line-{}", uuid::Uuid::new_v4())))
}

/// FB has no batch endpoint — send one message per item, return the last id.
async fn fb_send(token: &str, recipient: &str, items: &[OutboundItem]) -> Result<String, String> {
    let url = format!("https://graph.facebook.com/v21.0/me/messages?access_token={token}");
    let mut last_id = String::new();
    for it in items {
        let resp = http_client()
            .post(&url)
            .json(&fb_send_body(recipient, &it.content))
            .send()
            .await
            .map_err(|e| format!("Facebook request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return Err(format!("Facebook send failed ({status}): {txt}"));
        }
        let v: serde_json::Value = resp.json().await.unwrap_or_else(|_| json!({}));
        last_id = v["message_id"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("fb-{}", uuid::Uuid::new_v4()));
    }
    Ok(last_id)
}

/// End-user profile from a platform lookup (best-effort; both fields optional).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Profile {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Trimmed non-empty string from a JSON field, else `None`.
fn non_empty(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a LINE `GET /v2/bot/profile/{userId}` body (pure — unit-tested).
pub fn parse_line_profile(v: &serde_json::Value) -> Profile {
    Profile {
        display_name: non_empty(v.get("displayName")),
        avatar_url: non_empty(v.get("pictureUrl")),
    }
}

/// Parse a Meta Graph `?fields=name,username,profile_pic` body (pure — unit-tested).
pub fn parse_meta_profile(v: &serde_json::Value) -> Profile {
    Profile {
        display_name: non_empty(v.get("name")).or_else(|| non_empty(v.get("username"))),
        avatar_url: non_empty(v.get("profile_pic")),
    }
}

async fn line_profile(token: &str, user_id: &str) -> Profile {
    let url = format!("https://api.line.me/v2/bot/profile/{user_id}");
    match http_client()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            parse_line_profile(&resp.json::<serde_json::Value>().await.unwrap_or_else(|_| json!({})))
        }
        _ => Profile::default(),
    }
}

async fn meta_profile(token: &str, user_id: &str) -> Profile {
    let url = format!(
        "https://graph.facebook.com/v21.0/{user_id}?fields=name,username,profile_pic&access_token={token}"
    );
    match http_client()
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            parse_meta_profile(&resp.json::<serde_json::Value>().await.unwrap_or_else(|_| json!({})))
        }
        _ => Profile::default(),
    }
}

/// Outbound delivery gateway holding the configured per-platform tokens. With no
/// token for a platform, `line` preserves the documented stub success and other
/// platforms report "not supported" (so dev/tests make no network calls).
pub struct OutboundGateway {
    line: Option<String>,
    facebook: Option<String>,
    instagram: Option<String>,
}

impl OutboundGateway {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            line: config.line_channel_access_token.clone().filter(|t| !t.is_empty()),
            facebook: config.facebook_page_access_token.clone().filter(|t| !t.is_empty()),
            instagram: config
                .instagram_access_token
                .clone()
                .filter(|t| !t.is_empty())
                .or_else(|| config.facebook_page_access_token.clone().filter(|t| !t.is_empty())),
        }
    }

    /// Push one batch (≤ BATCH_CAP items); returns the platform-side message id.
    pub async fn send_batch(
        &self,
        platform: &str,
        recipient: &str,
        items: &[OutboundItem],
    ) -> Result<String, String> {
        match platform {
            "line" => match &self.line {
                Some(tok) => line_push(tok, recipient, items).await,
                None => Ok(format!("stub-line-{}", uuid::Uuid::new_v4())),
            },
            "facebook" => match &self.facebook {
                Some(tok) => fb_send(tok, recipient, items).await,
                None => Err("Outbound delivery is not supported for platform 'facebook'".into()),
            },
            "instagram" => match &self.instagram {
                Some(tok) => fb_send(tok, recipient, items).await,
                None => Err("Outbound delivery is not supported for platform 'instagram'".into()),
            },
            "shopee" => Err("Outbound delivery is not supported for platform 'shopee'".into()),
            other => Err(format!("Outbound delivery is not supported for platform '{other}'")),
        }
    }

    /// Best-effort end-user profile lookup (name + avatar). Returns an empty
    /// `Profile` for an unknown platform, a missing token, an empty user id, or
    /// any network/parse failure — never errors, never panics.
    pub async fn fetch_profile(&self, platform: &str, user_id: &str) -> Profile {
        if user_id.is_empty() {
            return Profile::default();
        }
        match platform {
            "line" => match &self.line {
                Some(t) => line_profile(t, user_id).await,
                None => Profile::default(),
            },
            "facebook" => match &self.facebook {
                Some(t) => meta_profile(t, user_id).await,
                None => Profile::default(),
            },
            "instagram" => match &self.instagram {
                Some(t) => meta_profile(t, user_id).await,
                None => Profile::default(),
            },
            _ => Profile::default(),
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
    gateway: OutboundGateway,
) {
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut platform_message_id: Option<String> = None;
    let mut last_error: Option<String> = None;

    for batch in items.chunks(BATCH_CAP) {
        match gateway.send_batch(&platform, &recipient, batch).await {
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

/// Fetch LINE message content (image/video/audio/file) with the channel token.
/// `preview` requests the smaller preview rendition (image/video only). Returns
/// `(bytes, content_type)` or `None` on any failure — best-effort, never panics.
pub(crate) async fn fetch_line_media(
    token: &str,
    message_id: &str,
    preview: bool,
) -> Option<(Vec<u8>, String)> {
    let suffix = if preview { "/preview" } else { "" };
    let url = format!("https://api-data.line.me/v2/bot/message/{message_id}/content{suffix}");
    let resp = http_client()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = resp.bytes().await.ok()?;
    Some((bytes.to_vec(), content_type))
}

#[cfg(test)]
mod gateway_tests {
    use super::*;

    #[test]
    fn push_body_has_to_and_text_messages() {
        let items = vec![OutboundItem { content: "hi".into() }, OutboundItem { content: "bye".into() }];
        let b = build_push_body("U123", &items);
        assert_eq!(b["to"], "U123");
        assert_eq!(b["messages"][0]["text"], "hi");
        assert_eq!(b["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn fb_body_has_recipient_and_text() {
        let b = fb_send_body("PSID1", "hello");
        assert_eq!(b["recipient"]["id"], "PSID1");
        assert_eq!(b["messaging_type"], "RESPONSE");
        assert_eq!(b["message"]["text"], "hello");
    }

    #[test]
    fn from_config_reflects_configured_tokens() {
        let mut c = crate::config::test_config();
        c.line_channel_access_token = None;
        c.facebook_page_access_token = None;
        let g = OutboundGateway::from_config(&c);
        assert!(g.line.is_none() && g.facebook.is_none());
        c.line_channel_access_token = Some("L".into());
        c.facebook_page_access_token = Some("F".into());
        let g = OutboundGateway::from_config(&c);
        assert!(g.line.is_some() && g.facebook.is_some());
    }

    #[test]
    fn from_config_instagram_token_with_fallback() {
        let mut c = crate::config::test_config();
        c.instagram_access_token = None;
        c.facebook_page_access_token = None;
        assert!(OutboundGateway::from_config(&c).instagram.is_none());

        // Falls back to the page token when the IG token is unset.
        c.facebook_page_access_token = Some("PAGE".into());
        assert!(OutboundGateway::from_config(&c).instagram.is_some());

        // Dedicated IG token wins.
        c.instagram_access_token = Some("IG".into());
        assert!(OutboundGateway::from_config(&c).instagram.is_some());
    }

    #[test]
    fn parse_line_profile_extracts_name_and_avatar() {
        let v = serde_json::json!({ "displayName": "陳小明", "pictureUrl": "https://p/x.jpg" });
        let p = parse_line_profile(&v);
        assert_eq!(p.display_name.as_deref(), Some("陳小明"));
        assert_eq!(p.avatar_url.as_deref(), Some("https://p/x.jpg"));
    }

    #[test]
    fn parse_line_profile_empty_fields_are_none() {
        let v = serde_json::json!({ "displayName": "", "pictureUrl": "  " });
        let p = parse_line_profile(&v);
        assert_eq!(p, Profile::default());
    }

    #[test]
    fn parse_meta_profile_prefers_name_then_username() {
        let with_name = serde_json::json!({ "name": "Jane", "username": "jane_ig", "profile_pic": "https://p/a.jpg" });
        assert_eq!(parse_meta_profile(&with_name).display_name.as_deref(), Some("Jane"));
        let only_user = serde_json::json!({ "username": "jane_ig" });
        assert_eq!(parse_meta_profile(&only_user).display_name.as_deref(), Some("jane_ig"));
        assert_eq!(parse_meta_profile(&with_name).avatar_url.as_deref(), Some("https://p/a.jpg"));
    }

    #[tokio::test]
    async fn fetch_profile_without_token_is_empty() {
        let mut c = crate::config::test_config();
        c.line_channel_access_token = None;
        c.facebook_page_access_token = None;
        c.instagram_access_token = None;
        let g = OutboundGateway::from_config(&c);
        assert_eq!(g.fetch_profile("line", "U1").await, Profile::default());
        assert_eq!(g.fetch_profile("facebook", "P1").await, Profile::default());
        assert_eq!(g.fetch_profile("instagram", "I1").await, Profile::default());
        assert_eq!(g.fetch_profile("shopee", "S1").await, Profile::default());
    }
}

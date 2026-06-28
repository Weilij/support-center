//! Outbound platform delivery (CRD 765-773): the send endpoint persists a pending
//! message and returns immediately; this module performs the background delivery
//! that drives the observable pending -> sent | partial | failed transitions.
//!
//! Delivery is routed through [`OutboundGateway`], which holds the configured
//! per-platform tokens. With a token, the platform calls its real API (LINE
//! Messaging API push, Facebook Send API); without one, the gateway returns a
//! delivery error so callers do not mark unsent external messages as delivered.

use sqlx::PgPool;

/// One outbound unit: a text body, or a media attachment.
pub struct OutboundItem {
    pub content: String,
    pub media: Option<OutboundMedia>,
}

impl OutboundItem {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            media: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    File,
}

#[derive(Clone)]
pub struct OutboundMedia {
    pub kind: MediaKind,
    pub url: String,
    pub preview_url: Option<String>,
    pub file_name: Option<String>,
    pub duration_ms: Option<i64>,
}

/// Display-only audio length when the real duration is unknown (LINE plays the
/// full clip regardless).
const DEFAULT_AUDIO_DURATION_MS: i64 = 60_000;

/// Classify an attachment mime into a LINE-deliverable kind.
pub fn classify_mime(mime: &str) -> MediaKind {
    if mime.starts_with("image/") {
        MediaKind::Image
    } else if mime.starts_with("video/") {
        MediaKind::Video
    } else if mime.starts_with("audio/") {
        MediaKind::Audio
    } else {
        MediaKind::File
    }
}

/// The downstream platform's per-call message cap (LINE push cap, CRD 769).
pub const BATCH_CAP: usize = 5;

use serde_json::json;
use std::fmt;
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

/// One LINE message object for an outbound item (pure — unit-tested).
fn line_message(it: &OutboundItem) -> serde_json::Value {
    match &it.media {
        None => json!({ "type": "text", "text": it.content }),
        Some(m) => match m.kind {
            MediaKind::Image => json!({
                "type": "image",
                "originalContentUrl": m.url,
                "previewImageUrl": m.preview_url.clone().unwrap_or_else(|| m.url.clone()),
            }),
            MediaKind::Video => json!({
                "type": "video",
                "originalContentUrl": m.url,
                "previewImageUrl": m.preview_url.clone().unwrap_or_else(|| m.url.clone()),
            }),
            MediaKind::Audio => json!({
                "type": "audio",
                "originalContentUrl": m.url,
                "duration": m.duration_ms.unwrap_or(DEFAULT_AUDIO_DURATION_MS),
            }),
            MediaKind::File => json!({
                "type": "text",
                "text": format!("📎 {}\n{}", m.file_name.clone().unwrap_or_default(), m.url),
            }),
        },
    }
}

/// The outbound message body for a LINE push (pure — unit-tested).
pub fn build_push_body(recipient: &str, items: &[OutboundItem]) -> serde_json::Value {
    json!({
        "to": recipient,
        "messages": items.iter().map(line_message).collect::<Vec<_>>(),
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

#[derive(Debug)]
pub enum OutboundError {
    InvalidRecipient(&'static str),
    RequestFailed {
        platform: &'static str,
        source: reqwest::Error,
    },
    PlatformRejected {
        platform: &'static str,
        status: reqwest::StatusCode,
        body: String,
    },
    DeleteRequestFailed {
        platform: &'static str,
        source: reqwest::Error,
    },
    DeleteRejected {
        platform: &'static str,
        status: reqwest::StatusCode,
        body: String,
    },
    TokenStore(crate::domain::shopee::store::StoreError),
    MissingCredentials(&'static str),
    MissingTokenStore,
    UnsupportedPlatform(String),
}

impl fmt::Display for OutboundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRecipient(message) => f.write_str(message),
            Self::RequestFailed { platform, source } => {
                write!(f, "{platform} request failed: {source}")
            }
            Self::PlatformRejected {
                platform,
                status,
                body,
            } => write!(f, "{platform} send failed ({status}): {body}"),
            Self::DeleteRequestFailed { platform, source } => {
                write!(f, "{platform} delete request failed: {source}")
            }
            Self::DeleteRejected {
                platform,
                status,
                body,
            } => write!(f, "{platform} delete failed ({status}): {body}"),
            Self::TokenStore(error) => write!(f, "{error}"),
            Self::MissingCredentials(platform) => {
                write!(f, "{platform} delivery requires configured credentials")
            }
            Self::MissingTokenStore => f.write_str("Shopee delivery requires shop token storage"),
            Self::UnsupportedPlatform(platform) => write!(
                f,
                "Outbound delivery is not supported for platform '{platform}'"
            ),
        }
    }
}

impl std::error::Error for OutboundError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RequestFailed { source, .. } => Some(source),
            Self::DeleteRequestFailed { source, .. } => Some(source),
            Self::TokenStore(error) => Some(error),
            _ => None,
        }
    }
}

pub type OutboundResult<T> = std::result::Result<T, OutboundError>;

pub fn parse_shopee_recipient(recipient: &str) -> OutboundResult<(i64, String)> {
    let Some((shop, buyer)) = recipient.split_once(':') else {
        return Err(OutboundError::InvalidRecipient(
            "Shopee recipient must be encoded as shop_id:buyer_id",
        ));
    };
    let shop_id = shop
        .parse::<i64>()
        .map_err(|_| OutboundError::InvalidRecipient("Shopee recipient has invalid shop_id"))?;
    if shop_id <= 0 || buyer.trim().is_empty() {
        return Err(OutboundError::InvalidRecipient(
            "Shopee recipient must include a positive shop_id and buyer_id",
        ));
    }
    Ok((shop_id, buyer.trim().to_string()))
}

pub fn shopee_send_body(to_id: &str, item: &OutboundItem) -> serde_json::Value {
    match &item.media {
        Some(m) if m.kind == MediaKind::Image => json!({
            "to_id": to_id,
            "message_type": "image",
            "content": {
                "url": m.url,
                "preview_url": m.preview_url.clone().unwrap_or_else(|| m.url.clone()),
            },
        }),
        Some(m) => json!({
            "to_id": to_id,
            "message_type": "text",
            "content": {
                "text": format!("📎 {}\n{}", m.file_name.clone().unwrap_or_default(), m.url),
            },
        }),
        None => json!({
            "to_id": to_id,
            "message_type": "text",
            "content": { "text": item.content },
        }),
    }
}

async fn line_push(
    url: &str,
    token: &str,
    recipient: &str,
    items: &[OutboundItem],
) -> OutboundResult<String> {
    let body = build_push_body(recipient, items);
    let resp = http_client()
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|source| OutboundError::RequestFailed {
            platform: "LINE",
            source,
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(OutboundError::PlatformRejected {
            platform: "LINE",
            status,
            body: txt,
        });
    }
    let v: serde_json::Value = resp.json().await.unwrap_or_else(|_| json!({}));
    Ok(v["sentMessages"][0]["id"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("line-{}", uuid::Uuid::new_v4())))
}

/// FB has no batch endpoint — send one message per item, return the last id.
async fn fb_send(token: &str, recipient: &str, items: &[OutboundItem]) -> OutboundResult<String> {
    let url = format!("https://graph.facebook.com/v21.0/me/messages?access_token={token}");
    let mut last_id = String::new();
    for it in items {
        let content = match &it.media {
            Some(m) => format!("📎 {}\n{}", m.file_name.clone().unwrap_or_default(), m.url),
            None => it.content.clone(),
        };
        let resp = http_client()
            .post(&url)
            .json(&fb_send_body(recipient, &content))
            .send()
            .await
            .map_err(|source| OutboundError::RequestFailed {
                platform: "Facebook",
                source,
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return Err(OutboundError::PlatformRejected {
                platform: "Facebook",
                status,
                body: txt,
            });
        }
        let v: serde_json::Value = resp.json().await.unwrap_or_else(|_| json!({}));
        last_id = v["message_id"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("fb-{}", uuid::Uuid::new_v4()));
    }
    Ok(last_id)
}

async fn meta_delete_message(
    graph_url: &str,
    platform: &'static str,
    token: &str,
    message_id: &str,
) -> OutboundResult<()> {
    let url = format!("{}/{}", graph_url.trim_end_matches('/'), message_id);
    let resp = http_client()
        .delete(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|source| OutboundError::DeleteRequestFailed { platform, source })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(OutboundError::DeleteRejected {
            platform,
            status,
            body: txt,
        });
    }
    Ok(())
}

async fn shopee_send(
    client: &crate::domain::shopee::client::ShopeeClient,
    db: &PgPool,
    enc_key: Option<&str>,
    recipient: &str,
    items: &[OutboundItem],
) -> OutboundResult<String> {
    let (shop_id, buyer_id) = parse_shopee_recipient(recipient)?;
    let token = crate::domain::shopee::store::valid_access_token(db, client, enc_key, shop_id)
        .await
        .map_err(OutboundError::TokenStore)?;
    let path = "/api/v2/sellerchat/send_message";
    let query = client.signed_query(
        path,
        chrono::Utc::now().timestamp(),
        Some(&token),
        Some(shop_id),
    );
    let url = client.url(path, &query);
    let mut last_id = String::new();
    for it in items {
        let resp = http_client()
            .post(&url)
            .json(&shopee_send_body(&buyer_id, it))
            .send()
            .await
            .map_err(|source| OutboundError::RequestFailed {
                platform: "Shopee",
                source,
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return Err(OutboundError::PlatformRejected {
                platform: "Shopee",
                status,
                body: txt,
            });
        }
        let v: serde_json::Value = resp.json().await.unwrap_or_else(|_| json!({}));
        last_id = v["message_id"]
            .as_str()
            .or_else(|| v["response"]["message_id"].as_str())
            .or_else(|| v["request_id"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("shopee-{}", uuid::Uuid::new_v4()));
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
        Ok(resp) if resp.status().is_success() => parse_line_profile(
            &resp
                .json::<serde_json::Value>()
                .await
                .unwrap_or_else(|_| json!({})),
        ),
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
        Ok(resp) if resp.status().is_success() => parse_meta_profile(
            &resp
                .json::<serde_json::Value>()
                .await
                .unwrap_or_else(|_| json!({})),
        ),
        _ => Profile::default(),
    }
}

/// Outbound delivery gateway holding the configured per-platform tokens. Missing
/// credentials fail delivery instead of reporting a synthetic platform id.
pub struct OutboundGateway {
    line: Option<String>,
    line_push_url: String,
    facebook: Option<String>,
    instagram: Option<String>,
    meta_graph_url: String,
    shopee: Option<crate::domain::shopee::client::ShopeeClient>,
    shopee_db: Option<PgPool>,
    encryption_key: Option<String>,
}

impl OutboundGateway {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            line: config
                .line_channel_access_token
                .clone()
                .filter(|t| !t.is_empty()),
            line_push_url: config.line_push_url.clone(),
            facebook: config
                .facebook_page_access_token
                .clone()
                .filter(|t| !t.is_empty()),
            instagram: config
                .instagram_access_token
                .clone()
                .filter(|t| !t.is_empty())
                .or_else(|| {
                    config
                        .facebook_page_access_token
                        .clone()
                        .filter(|t| !t.is_empty())
                }),
            meta_graph_url: config.meta_graph_url.clone(),
            shopee: crate::domain::shopee::client::ShopeeClient::from_config(config),
            shopee_db: None,
            encryption_key: config.encryption_key.clone(),
        }
    }

    pub fn from_state(state: &crate::state::AppState) -> Self {
        let mut gateway = Self::from_config(&state.config);
        gateway.shopee_db = Some(state.db.clone());
        gateway
    }

    /// Build the gateway from live resolved credentials (DB → .env) per platform.
    /// Used by runtime send paths; `from_config` remains for tests / pure config.
    pub async fn resolve(state: &crate::state::AppState) -> Self {
        use crate::domain::channels::resolve::resolve_channel;
        Self {
            line: resolve_channel(state, "line").await.access_token,
            line_push_url: state.config.line_push_url.clone(),
            facebook: resolve_channel(state, "facebook").await.access_token,
            instagram: resolve_channel(state, "instagram").await.access_token,
            meta_graph_url: state.config.meta_graph_url.clone(),
            shopee: crate::domain::shopee::client::ShopeeClient::from_config(&state.config),
            shopee_db: Some(state.db.clone()),
            encryption_key: state.config.encryption_key.clone(),
        }
    }

    /// Push one batch (≤ BATCH_CAP items); returns the platform-side message id.
    pub async fn send_batch(
        &self,
        platform: &str,
        recipient: &str,
        items: &[OutboundItem],
    ) -> OutboundResult<String> {
        match platform {
            "line" => match &self.line {
                Some(tok) => line_push(&self.line_push_url, tok, recipient, items).await,
                None => Err(OutboundError::MissingCredentials("LINE")),
            },
            "facebook" => match &self.facebook {
                Some(tok) => fb_send(tok, recipient, items).await,
                None => Err(OutboundError::UnsupportedPlatform("facebook".into())),
            },
            "instagram" => match &self.instagram {
                Some(tok) => fb_send(tok, recipient, items).await,
                None => Err(OutboundError::UnsupportedPlatform("instagram".into())),
            },
            "shopee" => match (&self.shopee, &self.shopee_db) {
                (Some(client), Some(db)) => {
                    shopee_send(client, db, self.encryption_key.as_deref(), recipient, items).await
                }
                (Some(_), None) => Err(OutboundError::MissingTokenStore),
                _ => Err(OutboundError::UnsupportedPlatform("shopee".into())),
            },
            other => Err(OutboundError::UnsupportedPlatform(other.to_string())),
        }
    }

    /// Best-effort remote deletion for platforms that expose a message delete
    /// endpoint. Local recall state is owned by the caller and should not be
    /// reverted if this fails.
    pub async fn delete_message(&self, platform: &str, message_id: &str) -> OutboundResult<()> {
        if message_id.trim().is_empty() {
            return Err(OutboundError::InvalidRecipient(
                "Platform message id is required for delete",
            ));
        }
        match platform {
            "facebook" => match &self.facebook {
                Some(tok) => {
                    meta_delete_message(&self.meta_graph_url, "Facebook", tok, message_id).await
                }
                None => Err(OutboundError::UnsupportedPlatform("facebook".into())),
            },
            "instagram" => match &self.instagram {
                Some(tok) => {
                    meta_delete_message(&self.meta_graph_url, "Instagram", tok, message_id).await
                }
                None => Err(OutboundError::UnsupportedPlatform("instagram".into())),
            },
            other => Err(OutboundError::UnsupportedPlatform(other.to_string())),
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

pub struct PendingDelivery {
    pub db: PgPool,
    pub hub: std::sync::Arc<crate::realtime::RealtimeHub>,
    pub conversation_id: String,
    pub message_id: String,
    pub platform: String,
    pub recipient: String,
    pub items: Vec<OutboundItem>,
    pub gateway: OutboundGateway,
}

/// Background delivery task (fire-and-forget from the send handler, CRD 769-773):
/// batches the items to the platform cap, then persists the final sent flag,
/// delivery status (sent / partial / failed), and platform message id.
pub async fn deliver_pending(input: PendingDelivery) {
    let PendingDelivery {
        db,
        hub,
        conversation_id,
        message_id,
        platform,
        recipient,
        items,
        gateway,
    } = input;
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
                last_error = Some(e.to_string());
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
    fetch_line_media_from_base("https://api-data.line.me", token, message_id, preview).await
}

pub(crate) async fn fetch_line_media_from_base(
    base_url: &str,
    token: &str,
    message_id: &str,
    preview: bool,
) -> Option<(Vec<u8>, String)> {
    let suffix = if preview { "/preview" } else { "" };
    let url = format!(
        "{}/v2/bot/message/{message_id}/content{suffix}",
        base_url.trim_end_matches('/')
    );
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
        let items = vec![OutboundItem::text("hi"), OutboundItem::text("bye")];
        let b = build_push_body("U123", &items);
        assert_eq!(b["to"], "U123");
        assert_eq!(b["messages"][0]["text"], "hi");
        assert_eq!(b["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn push_body_dispatches_by_media_kind() {
        let items = vec![
            OutboundItem::text("hello"),
            OutboundItem {
                content: "pic.jpg".into(),
                media: Some(OutboundMedia {
                    kind: MediaKind::Image,
                    url: "https://h/o.jpg".into(),
                    preview_url: Some("https://h/p.jpg".into()),
                    file_name: None,
                    duration_ms: None,
                }),
            },
            OutboundItem {
                content: "clip".into(),
                media: Some(OutboundMedia {
                    kind: MediaKind::Video,
                    url: "https://h/v.mp4".into(),
                    preview_url: Some("https://h/ph.png".into()),
                    file_name: None,
                    duration_ms: None,
                }),
            },
            OutboundItem {
                content: "voice".into(),
                media: Some(OutboundMedia {
                    kind: MediaKind::Audio,
                    url: "https://h/a.m4a".into(),
                    preview_url: None,
                    file_name: None,
                    duration_ms: None,
                }),
            },
            OutboundItem {
                content: "doc".into(),
                media: Some(OutboundMedia {
                    kind: MediaKind::File,
                    url: "https://h/d.pdf".into(),
                    preview_url: None,
                    file_name: Some("report.pdf".into()),
                    duration_ms: None,
                }),
            },
        ];
        let b = build_push_body("U1", &items);
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["type"], "text");
        assert_eq!(m[1]["type"], "image");
        assert_eq!(m[1]["originalContentUrl"], "https://h/o.jpg");
        assert_eq!(m[1]["previewImageUrl"], "https://h/p.jpg");
        assert_eq!(m[2]["type"], "video");
        assert_eq!(m[2]["previewImageUrl"], "https://h/ph.png");
        assert_eq!(m[3]["type"], "audio");
        assert_eq!(m[3]["duration"], 60000);
        assert_eq!(m[4]["type"], "text");
        assert!(m[4]["text"].as_str().unwrap().contains("report.pdf"));
        assert!(m[4]["text"].as_str().unwrap().contains("📎"));
    }

    #[test]
    fn classify_mime_maps_kinds() {
        assert_eq!(classify_mime("image/png"), MediaKind::Image);
        assert_eq!(classify_mime("video/mp4"), MediaKind::Video);
        assert_eq!(classify_mime("audio/m4a"), MediaKind::Audio);
        assert_eq!(classify_mime("application/pdf"), MediaKind::File);
        assert_eq!(classify_mime(""), MediaKind::File);
    }

    #[test]
    fn fb_body_has_recipient_and_text() {
        let b = fb_send_body("PSID1", "hello");
        assert_eq!(b["recipient"]["id"], "PSID1");
        assert_eq!(b["messaging_type"], "RESPONSE");
        assert_eq!(b["message"]["text"], "hello");
    }

    #[test]
    fn shopee_recipient_parser_requires_shop_and_buyer() {
        assert_eq!(
            parse_shopee_recipient("42:9001").unwrap(),
            (42, "9001".to_string())
        );
        assert!(parse_shopee_recipient("9001").is_err());
        assert!(parse_shopee_recipient("x:9001").is_err());
        assert!(parse_shopee_recipient("42:").is_err());
    }

    #[test]
    fn outbound_error_messages_match_delivery_contract() {
        assert_eq!(
            parse_shopee_recipient("9001").unwrap_err().to_string(),
            "Shopee recipient must be encoded as shop_id:buyer_id"
        );
        assert_eq!(
            OutboundError::UnsupportedPlatform("facebook".into()).to_string(),
            "Outbound delivery is not supported for platform 'facebook'"
        );
        assert_eq!(
            OutboundError::MissingTokenStore.to_string(),
            "Shopee delivery requires shop token storage"
        );
        assert_eq!(
            OutboundError::MissingCredentials("LINE").to_string(),
            "LINE delivery requires configured credentials"
        );
        assert_eq!(
            OutboundError::PlatformRejected {
                platform: "LINE",
                status: reqwest::StatusCode::UNAUTHORIZED,
                body: "bad token".into(),
            }
            .to_string(),
            "LINE send failed (401 Unauthorized): bad token"
        );
        assert_eq!(
            OutboundError::DeleteRejected {
                platform: "Facebook",
                status: reqwest::StatusCode::BAD_REQUEST,
                body: "too old".into(),
            }
            .to_string(),
            "Facebook delete failed (400 Bad Request): too old"
        );
    }

    #[tokio::test]
    async fn line_send_without_token_fails_instead_of_stubbing_success() {
        let mut c = crate::config::test_config();
        c.line_channel_access_token = None;
        let g = OutboundGateway::from_config(&c);

        assert_eq!(
            g.send_batch("line", "U1", &[OutboundItem::text("hello")])
                .await
                .unwrap_err()
                .to_string(),
            "LINE delivery requires configured credentials"
        );
    }

    #[tokio::test]
    async fn delete_message_rejects_missing_id_and_missing_token_without_network() {
        let mut c = crate::config::test_config();
        c.facebook_page_access_token = None;
        c.instagram_access_token = None;
        let g = OutboundGateway::from_config(&c);

        assert_eq!(
            g.delete_message("facebook", "")
                .await
                .unwrap_err()
                .to_string(),
            "Platform message id is required for delete"
        );
        assert_eq!(
            g.delete_message("facebook", "mid.1")
                .await
                .unwrap_err()
                .to_string(),
            "Outbound delivery is not supported for platform 'facebook'"
        );
        assert_eq!(
            g.delete_message("line", "mid.1")
                .await
                .unwrap_err()
                .to_string(),
            "Outbound delivery is not supported for platform 'line'"
        );
    }

    #[test]
    fn shopee_body_has_buyer_and_text_content() {
        let b = shopee_send_body("9001", &OutboundItem::text("hello"));
        assert_eq!(b["to_id"], "9001");
        assert_eq!(b["message_type"], "text");
        assert_eq!(b["content"]["text"], "hello");
    }

    #[test]
    fn shopee_body_dispatches_native_media_when_supported() {
        let image = shopee_send_body(
            "9001",
            &OutboundItem {
                content: "pic.png".into(),
                media: Some(OutboundMedia {
                    kind: MediaKind::Image,
                    url: "https://cdn.example/pic.png".into(),
                    preview_url: Some("https://cdn.example/preview.png".into()),
                    file_name: Some("pic.png".into()),
                    duration_ms: None,
                }),
            },
        );
        assert_eq!(image["to_id"], "9001");
        assert_eq!(image["message_type"], "image");
        assert_eq!(image["content"]["url"], "https://cdn.example/pic.png");
        assert_eq!(
            image["content"]["preview_url"],
            "https://cdn.example/preview.png"
        );

        let file = shopee_send_body(
            "9001",
            &OutboundItem {
                content: "report.pdf".into(),
                media: Some(OutboundMedia {
                    kind: MediaKind::File,
                    url: "https://cdn.example/report.pdf".into(),
                    preview_url: None,
                    file_name: Some("report.pdf".into()),
                    duration_ms: None,
                }),
            },
        );
        assert_eq!(file["message_type"], "text");
        assert!(file["content"]["text"]
            .as_str()
            .unwrap()
            .contains("report.pdf"));
        assert!(file["content"]["text"]
            .as_str()
            .unwrap()
            .contains("https://cdn.example/report.pdf"));
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
        c.shopee_partner_id = Some(1);
        c.shopee_partner_key = Some("S".into());
        let g = OutboundGateway::from_config(&c);
        assert!(g.line.is_some() && g.facebook.is_some());
        assert!(g.shopee.is_some());
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
        assert_eq!(
            parse_meta_profile(&with_name).display_name.as_deref(),
            Some("Jane")
        );
        let only_user = serde_json::json!({ "username": "jane_ig" });
        assert_eq!(
            parse_meta_profile(&only_user).display_name.as_deref(),
            Some("jane_ig")
        );
        assert_eq!(
            parse_meta_profile(&with_name).avatar_url.as_deref(),
            Some("https://p/a.jpg")
        );
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

    #[tokio::test]
    async fn fetch_line_media_from_base_uses_bearer_token_and_content_type() {
        use axum::body::Bytes;
        use axum::extract::Path;
        use axum::http::{header, HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        use axum::routing::get;
        use axum::Router;
        use std::net::SocketAddr;

        async fn content(Path(id): Path<String>, headers: HeaderMap) -> axum::response::Response {
            let token = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if token == "Bearer media-ok" && id == "mid-1" {
                (
                    [(header::CONTENT_TYPE, "image/png")],
                    Bytes::from_static(b"png-bytes"),
                )
                    .into_response()
            } else {
                StatusCode::UNAUTHORIZED.into_response()
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let app = Router::new().route("/v2/bot/message/{id}/content", get(content));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{addr}");

        let (bytes, content_type) = fetch_line_media_from_base(&base, "media-ok", "mid-1", false)
            .await
            .unwrap();
        assert_eq!(bytes, b"png-bytes");
        assert_eq!(content_type, "image/png");
        assert!(fetch_line_media_from_base(&base, "bad", "mid-1", false)
            .await
            .is_none());
    }
}

//! Public webhook ingress handlers (CRD §4.2, lines 2728-2812): LINE-style
//! message webhook + readiness probe, and the Facebook/Instagram-style
//! subscription handshake + event delivery endpoint.
//!
//! These routes are PUBLIC: no session/JWT — trust is established by
//! platform-signature verification over the exact raw request body.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::now_iso;
use crate::state::AppState;

use super::ingest::{self, InboundMessage};
use super::parse;

type HmacSha256 = Hmac<Sha256>;

/// Payload-size ceiling: about one megabyte (CRD 2723, 2736).
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

#[cfg(test)]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
fn hmac_sha256(secret: &str, body: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key size");
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
}

fn verify_hmac_sha256(secret: &str, body: &[u8], presented: &[u8]) -> bool {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key size");
    mac.update(body);
    mac.verify_slice(presented).is_ok()
}

fn valid_line_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    B64.decode(signature)
        .ok()
        .is_some_and(|presented| verify_hmac_sha256(secret, body, &presented))
}

fn decode_hex_signature(signature: &str) -> Option<Vec<u8>> {
    let bytes = signature.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

fn valid_facebook_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    let presented_hex = signature.strip_prefix("sha256=").unwrap_or(signature);
    decode_hex_signature(presented_hex)
        .is_some_and(|presented| verify_hmac_sha256(secret, body, &presented))
}

fn valid_shopee_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    let presented_hex = signature.strip_prefix("sha256=").unwrap_or(signature);
    decode_hex_signature(presented_hex)
        .is_some_and(|presented| verify_hmac_sha256(secret, body, &presented))
}

fn sha256_hex(input: &[u8]) -> String {
    Sha256::digest(input)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn meta_postback_event_key(platform: &str, sender: &str, item: &Value, postback: &Value) -> String {
    if let Some(mid) = postback
        .get("mid")
        .or_else(|| item.get("mid"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return format!("{platform}:postback:{mid}");
    }
    let recipient = item["recipient"]["id"].as_str().unwrap_or_default();
    let timestamp = item
        .get("timestamp")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let title = postback
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = postback
        .get("payload")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let referral_ref = postback
        .get("referral")
        .and_then(|v| v.get("ref"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let canonical = [
        platform,
        sender,
        recipient,
        &timestamp.to_string(),
        title,
        payload,
        referral_ref,
    ]
    .join("\0");
    let digest = sha256_hex(canonical.as_bytes());
    format!("{platform}:postback:{sender}:{recipient}:{timestamp}:{digest}")
}

fn client_ip(headers: &HeaderMap) -> Option<String> {
    for h in ["cf-connecting-ip", "x-forwarded-for", "x-real-ip"] {
        if let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()) {
            let first = v.split(',').next().unwrap_or(v).trim();
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
    }
    None
}

/// Record one webhook security event (best-effort).
async fn security_event(
    db: &PgPool,
    event_type: &str,
    severity: &str,
    platform: &str,
    source_ip: Option<&str>,
    details: Value,
) {
    if let Err(error) = sqlx::query(
        "INSERT INTO webhook_security_events
            (id, event_type, severity, platform, source_ip, details, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(event_type)
    .bind(severity)
    .bind(platform)
    .bind(source_ip)
    .bind(details.to_string())
    .bind(now_iso())
    .execute(db)
    .await
    {
        tracing::warn!(
            error = %error,
            event_type,
            severity,
            platform,
            "webhook security event write failed"
        );
    }
}

fn processing_failure_alert(
    platform: &str,
    failed: usize,
    total: usize,
    source_ip: Option<&str>,
    details: Value,
) -> (String, String, Value) {
    let title = format!("{platform} webhook processing failure");
    let message = format!("{failed} of {total} {platform} webhook events failed");
    let metadata = json!({
        "platform": platform,
        "failed": failed,
        "total": total,
        "sourceIp": source_ip,
        "details": details,
    });
    (title, message, metadata)
}

async fn dispatch_processing_failure_alert(
    platform: &str,
    failed: usize,
    total: usize,
    source_ip: Option<&str>,
    details: Value,
) {
    let (title, message, metadata) =
        processing_failure_alert(platform, failed, total, source_ip, details);
    let (successes, failures, errors) = crate::domain::notifications::alerts::send_security_alert(
        &title,
        &message,
        "high",
        Some(metadata),
    )
    .await;
    if failures > 0 {
        tracing::warn!(
            platform,
            successes,
            failures,
            errors = ?errors,
            "webhook processing failure alert dispatch incomplete"
        );
    }
}

fn fail(status: StatusCode, error: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "error": error, "timestamp": now_iso() })),
    )
        .into_response()
}

fn batch_ok() -> Response {
    // Standard success envelope with a null data field (CRD 2758).
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "message": "Webhook processed successfully",
            "data": null,
            "timestamp": now_iso(),
        })),
    )
        .into_response()
}

fn value_id(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn shopee_platform_user_id(shop_id: &str, buyer_id: &str) -> String {
    format!("{shop_id}:{buyer_id}")
}

// --------------------------------------------- LINE readiness probe (CRD 2774-2779)

pub async fn line_probe() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "message": "LINE webhook endpoint is ready",
            "timestamp": now_iso(),
            "endpoint": "/api/webhook",
            "method": "POST",
        })),
    )
        .into_response()
}

// --------------------------------------------- LINE message webhook (CRD 2728-2772)

pub async fn line_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ip = client_ip(&headers);

    // 1. Body size is checked first; oversize is rejected before any other
    //    work (CRD 2738).
    if body.len() > MAX_BODY_BYTES {
        security_event(
            &state.db,
            "payload_too_large",
            "medium",
            "line",
            ip.as_deref(),
            json!({ "size": body.len() }),
        )
        .await;
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "Payload too large");
    }

    // 2. Signature verified against the raw bytes (CRD 2739): HMAC-SHA256 of
    //    the body keyed by the channel secret, base64-encoded, per LINE's
    //    published scheme.
    let Some(secret) = state.config.line_channel_secret.as_deref() else {
        return fail(
            StatusCode::UNAUTHORIZED,
            "LINE channel secret is not configured",
        );
    };
    let Some(signature) = headers
        .get("x-line-signature")
        .and_then(|v| v.to_str().ok())
    else {
        security_event(
            &state.db,
            "missing_signature",
            "high",
            "line",
            ip.as_deref(),
            json!({}),
        )
        .await;
        return fail(StatusCode::UNAUTHORIZED, "Missing signature header");
    };
    if !valid_line_signature(secret, &body, signature) {
        security_event(
            &state.db,
            "invalid_signature",
            "high",
            "line",
            ip.as_deref(),
            json!({}),
        )
        .await;
        return fail(StatusCode::UNAUTHORIZED, "Invalid signature");
    }

    // 3-4. Parse and validate the payload shape (CRD 2740-2741).
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return fail(StatusCode::BAD_REQUEST, "Invalid JSON payload");
    };
    if !payload.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Invalid webhook payload",
                "errors": ["Payload must be an object"],
            })),
        )
            .into_response();
    }
    let Some(events) = payload.get("events").and_then(Value::as_array) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Invalid webhook payload",
                "errors": ["Payload must contain an events array"],
            })),
        )
            .into_response();
    };

    // 5-9. Each event is processed independently and sequentially; one failure
    //      does not abort the rest (CRD 2742-2746).
    let total = events.len();
    let mut failed = 0usize;
    let mut last_error: Option<String> = None;
    for event in events {
        let result: Result<(), ingest::IngestError> =
            match event.get("type").and_then(Value::as_str) {
                Some("message") => match event.get("message").filter(|m| m.is_object()) {
                    Some(message) => {
                        let normalized = parse::normalize_line(message);
                        let mid = message.get("id").and_then(Value::as_str);
                        let user_id = event["source"]["userId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        ingest::ingest_message(
                            &state,
                            InboundMessage {
                                platform: "line",
                                platform_user_id: &user_id,
                                default_display_name: ingest::default_display_name("line"),
                                platform_message_id: mid,
                                normalized,
                            },
                        )
                        .await
                        .map(|_| ())
                    }
                    None => Ok(()),
                },
                Some("follow") => ingest::handle_line_follow(&state, event).await,
                Some("unfollow") => ingest::handle_line_unfollow(&state, event).await,
                _ => Ok(()), // other event kinds are ignored (CRD 2746)
            };
        if let Err(e) = result {
            failed += 1;
            last_error = Some(e.to_string());
        }
    }

    // 10. On partial failure: record (and defer-dispatch) an alert, report the
    //     whole batch as failed so the platform retries (CRD 2747, 2761).
    if failed > 0 {
        let details = json!({ "failed": failed, "total": total, "lastError": last_error });
        security_event(
            &state.db,
            "webhook_processing_failure",
            "high",
            "line",
            ip.as_deref(),
            details.clone(),
        )
        .await;
        dispatch_processing_failure_alert("line", failed, total, ip.as_deref(), details).await;
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("{failed} of {total} events failed"),
        );
    }
    batch_ok()
}

enum ItemResult {
    None,
    Ingested(Result<(), ingest::IngestError>),
}

/// Handle one `messaging[]` item for a Meta platform (facebook | instagram).
async fn process_messaging_item(
    state: &std::sync::Arc<AppState>,
    platform: &str,
    default_name: &str,
    item: &Value,
) -> ItemResult {
    let sender = item["sender"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if let Some(message) = item.get("message").filter(|m| m.is_object()) {
        if message
            .get("is_echo")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || message
                .get("is_self")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return ItemResult::None;
        }
        let normalized = if platform == "instagram" {
            parse::normalize_instagram(message)
        } else {
            parse::normalize_facebook(message)
        };
        let mid = message.get("mid").and_then(Value::as_str);
        return ItemResult::Ingested(
            ingest::ingest_message(
                state,
                InboundMessage {
                    platform,
                    platform_user_id: &sender,
                    default_display_name: default_name,
                    platform_message_id: mid,
                    normalized,
                },
            )
            .await
            .map(|_| ()),
        );
    }
    if let Some(postback) = item.get("postback") {
        let normalized = parse::normalize_facebook_postback(postback);
        let event_key = meta_postback_event_key(platform, &sender, item, postback);
        return ItemResult::Ingested(
            ingest::ingest_message(
                state,
                InboundMessage {
                    platform,
                    platform_user_id: &sender,
                    default_display_name: default_name,
                    platform_message_id: Some(&event_key),
                    normalized,
                },
            )
            .await
            .map(|_| ()),
        );
    }
    if let Some(delivery) = item.get("delivery") {
        let mids: Vec<&str> = delivery
            .get("mids")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        ingest::mark_delivered(&state.db, &mids).await;
        return ItemResult::None;
    }
    if let Some(read) = item.get("read") {
        if let Some(wm) = read.get("watermark").and_then(Value::as_i64) {
            ingest::mark_read(&state.db, platform, &sender, wm).await;
        } else if let Some(mid) = read.get("mid").and_then(Value::as_str) {
            ingest::mark_read_by_mid(&state.db, platform, &sender, mid).await;
        }
        return ItemResult::None;
    }
    if let Some(reaction) = item.get("reaction") {
        ingest::apply_reaction(&state.db, reaction).await;
        return ItemResult::None;
    }
    ItemResult::None
}

// ---------------------------- Facebook handshake + event delivery (CRD 2780-2812)

pub async fn facebook_webhook(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ip = client_ip(&headers);

    // 1. Subscription handshake: mode must equal the platform's subscribe
    //    value and the token must exactly equal the configured verification
    //    token; the challenge is echoed verbatim (CRD 2787, 2790, 2802).
    if let Some(mode) = params.get("hub.mode") {
        let token_ok = state
            .config
            .facebook_verify_token
            .as_deref()
            .is_some_and(|t| params.get("hub.verify_token").map(String::as_str) == Some(t));
        if mode == "subscribe" && token_ok {
            let challenge = params.get("hub.challenge").cloned().unwrap_or_default();
            return (StatusCode::OK, challenge).into_response();
        }
        security_event(
            &state.db,
            "handshake_verification_failed",
            "medium",
            "facebook",
            ip.as_deref(),
            json!({ "mode": mode }),
        )
        .await;
        return fail(StatusCode::FORBIDDEN, "Webhook verification failed");
    }

    // 2. Content-length over the ceiling is rejected before reading the body
    //    (CRD 2791); the actual size is enforced as well.
    let declared = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > MAX_BODY_BYTES) || body.len() > MAX_BODY_BYTES {
        security_event(
            &state.db,
            "payload_too_large",
            "medium",
            "facebook",
            ip.as_deref(),
            json!({ "size": declared.unwrap_or(body.len()) }),
        )
        .await;
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "Payload too large");
    }

    // 3. App secret presence + signature over the raw bytes (CRD 2792):
    //    "sha256=" + HMAC-SHA256 hex per Meta's published scheme; the prefix
    //    marker is tolerated.
    let Some(secret) = state.config.facebook_app_secret.as_deref() else {
        return fail(StatusCode::UNAUTHORIZED, "Webhook not configured");
    };
    let Some(signature) = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
    else {
        security_event(
            &state.db,
            "missing_signature",
            "high",
            "facebook",
            ip.as_deref(),
            json!({}),
        )
        .await;
        return fail(StatusCode::UNAUTHORIZED, "Invalid signature");
    };
    if !valid_facebook_signature(secret, &body, signature) {
        security_event(
            &state.db,
            "invalid_signature",
            "high",
            "facebook",
            ip.as_deref(),
            json!({}),
        )
        .await;
        return fail(StatusCode::UNAUTHORIZED, "Invalid signature");
    }

    // 4-5. Parse + shape validation (CRD 2793-2794): object type must be one
    //      of page / instagram / user, with an entries array.
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return fail(StatusCode::BAD_REQUEST, "Invalid JSON payload");
    };
    let object = payload
        .get("object")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let entries = payload.get("entry").and_then(Value::as_array);
    if !["page", "instagram", "user"].contains(&object) || entries.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Invalid webhook payload",
                "errors": ["Payload must carry a valid object type and an entry array"],
            })),
        )
            .into_response();
    }

    // 6. The page (Facebook) and instagram object types are processed through
    //    the shared per-item processor; the user object type is accepted but
    //    not processed (CRD 2795).
    let mut total = 0usize;
    let mut failed = 0usize;
    let mut last_error: Option<String> = None;
    if object == "page" || object == "instagram" {
        let platform = if object == "instagram" {
            "instagram"
        } else {
            "facebook"
        };
        let default_name = ingest::default_display_name(platform);
        for entry in entries.unwrap_or(&Vec::new()) {
            let Some(items) = entry.get("messaging").and_then(Value::as_array) else {
                continue;
            };
            for item in items {
                match process_messaging_item(&state, platform, default_name, item).await {
                    ItemResult::Ingested(r) => {
                        total += 1;
                        if let Err(e) = r {
                            failed += 1;
                            last_error = Some(e.to_string());
                        }
                    }
                    ItemResult::None => {}
                }
            }
        }
    }

    if failed > 0 {
        let details = json!({ "failed": failed, "total": total, "lastError": last_error });
        security_event(
            &state.db,
            "webhook_processing_failure",
            "high",
            "facebook",
            ip.as_deref(),
            details.clone(),
        )
        .await;
        dispatch_processing_failure_alert("facebook", failed, total, ip.as_deref(), details).await;
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("{failed} of {total} Facebook events failed"),
        );
    }
    batch_ok()
}

// ---------------------------- Shopee Webchat push delivery

pub async fn shopee_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ip = client_ip(&headers);

    let declared = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > MAX_BODY_BYTES) || body.len() > MAX_BODY_BYTES {
        security_event(
            &state.db,
            "payload_too_large",
            "medium",
            "shopee",
            ip.as_deref(),
            json!({ "size": declared.unwrap_or(body.len()) }),
        )
        .await;
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "Payload too large");
    }

    let Some(secret) = state.config.shopee_partner_key.as_deref() else {
        return fail(
            StatusCode::UNAUTHORIZED,
            "Shopee partner key is not configured",
        );
    };
    let Some(signature) = headers
        .get("x-shopee-signature")
        .or_else(|| headers.get("authorization"))
        .and_then(|v| v.to_str().ok())
    else {
        security_event(
            &state.db,
            "missing_signature",
            "high",
            "shopee",
            ip.as_deref(),
            json!({}),
        )
        .await;
        return fail(StatusCode::UNAUTHORIZED, "Missing signature header");
    };
    if !valid_shopee_signature(secret, &body, signature) {
        security_event(
            &state.db,
            "invalid_signature",
            "high",
            "shopee",
            ip.as_deref(),
            json!({}),
        )
        .await;
        return fail(StatusCode::UNAUTHORIZED, "Invalid signature");
    }

    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return fail(StatusCode::BAD_REQUEST, "Invalid JSON payload");
    };
    if !payload.is_object() {
        return fail(StatusCode::BAD_REQUEST, "Invalid webhook payload");
    }

    let shop_id = value_id(payload.get("shop_id").or_else(|| payload.get("shopId")));
    let Some(shop_id) = shop_id else {
        return fail(StatusCode::BAD_REQUEST, "shop_id is required");
    };
    let message = payload
        .get("data")
        .filter(|v| v.is_object())
        .or_else(|| payload.get("message").filter(|v| v.is_object()))
        .unwrap_or(&payload);

    let buyer_id = value_id(
        message
            .get("buyer_id")
            .or_else(|| message.get("buyerId"))
            .or_else(|| message.get("from_id"))
            .or_else(|| message.get("fromId"))
            .or_else(|| message.get("sender_id"))
            .or_else(|| message.get("senderId")),
    );
    let Some(buyer_id) = buyer_id else {
        return batch_ok();
    };
    if buyer_id == shop_id {
        return batch_ok();
    }

    let platform_user_id = shopee_platform_user_id(&shop_id, &buyer_id);
    let mut normalized = parse::normalize_shopee(message);
    normalized.metadata.insert(
        "shopId".into(),
        json!(shop_id.parse::<i64>().ok().unwrap_or_default()),
    );
    normalized.metadata.insert(
        "buyerId".into(),
        json!(buyer_id.parse::<i64>().ok().unwrap_or_default()),
    );
    normalized.metadata.insert(
        "shopeePlatformUserId".into(),
        json!(platform_user_id.clone()),
    );

    let mid = value_id(
        message
            .get("message_id")
            .or_else(|| message.get("messageId"))
            .or_else(|| message.get("msg_id"))
            .or_else(|| message.get("msgId")),
    );
    let result = ingest::ingest_message(
        &state,
        InboundMessage {
            platform: "shopee",
            platform_user_id: &platform_user_id,
            default_display_name: ingest::default_display_name("shopee"),
            platform_message_id: mid.as_deref(),
            normalized,
        },
    )
    .await;

    if let Err(e) = result {
        let error = e.to_string();
        let details = json!({ "lastError": error });
        security_event(
            &state.db,
            "webhook_processing_failure",
            "high",
            "shopee",
            ip.as_deref(),
            details.clone(),
        )
        .await;
        dispatch_processing_failure_alert("shopee", 1, 1, ip.as_deref(), details).await;
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "Shopee event failed");
    }
    batch_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_signature_uses_raw_mac_verification() {
        let body = br#"{"events":[]}"#;
        let sig = B64.encode(hmac_sha256("secret", body));

        assert!(valid_line_signature("secret", body, &sig));
        assert!(!valid_line_signature("other", body, &sig));
        assert!(!valid_line_signature("secret", body, "not-base64"));
    }

    #[test]
    fn facebook_signature_uses_raw_mac_verification() {
        let body = br#"{"object":"page","entry":[]}"#;
        let hex_sig = hex(&hmac_sha256("secret", body));
        let sig = format!("sha256={hex_sig}");
        let upper = format!("sha256={}", hex_sig.to_uppercase());

        assert!(valid_facebook_signature("secret", body, &sig));
        assert!(valid_facebook_signature("secret", body, &upper));
        assert!(!valid_facebook_signature("other", body, &sig));
        assert!(!valid_facebook_signature("secret", body, "sha256=not-hex"));
        assert!(!valid_facebook_signature("secret", body, "sha256=abc"));
    }

    #[test]
    fn processing_failure_alert_payload_carries_retry_context() {
        let (title, message, metadata) = processing_failure_alert(
            "line",
            2,
            3,
            Some("203.0.113.7"),
            json!({ "lastError": "insert failed" }),
        );

        assert_eq!(title, "line webhook processing failure");
        assert_eq!(message, "2 of 3 line webhook events failed");
        assert_eq!(metadata["platform"], "line");
        assert_eq!(metadata["failed"], 2);
        assert_eq!(metadata["total"], 3);
        assert_eq!(metadata["sourceIp"], "203.0.113.7");
        assert_eq!(metadata["details"]["lastError"], "insert failed");
    }

    #[test]
    fn meta_postback_event_key_prefers_mid() {
        let item = json!({
            "sender": { "id": "S1" },
            "recipient": { "id": "R1" },
            "timestamp": 123,
            "postback": { "mid": "m_1", "payload": "P" }
        });

        assert_eq!(
            meta_postback_event_key("facebook", "S1", &item, &item["postback"]),
            "facebook:postback:m_1"
        );
    }

    #[test]
    fn meta_postback_event_key_is_stable_without_mid() {
        let item = json!({
            "sender": { "id": "S1" },
            "recipient": { "id": "R1" },
            "timestamp": 123,
            "postback": { "payload": "P", "title": "Start" }
        });
        let same = item.clone();
        let other_sender = json!({
            "sender": { "id": "S2" },
            "recipient": { "id": "R1" },
            "timestamp": 123,
            "postback": { "payload": "P", "title": "Start" }
        });

        let key = meta_postback_event_key("facebook", "S1", &item, &item["postback"]);
        assert_eq!(
            key,
            meta_postback_event_key("facebook", "S1", &same, &same["postback"])
        );
        assert_ne!(
            key,
            meta_postback_event_key("facebook", "S2", &other_sender, &other_sender["postback"])
        );
    }
}

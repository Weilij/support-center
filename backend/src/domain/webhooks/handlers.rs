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
use sha2::Sha256;
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac_sha256(secret: &str, body: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key size");
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
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
    let _ = sqlx::query(
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
    .await;
}

fn fail(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "success": false, "error": error, "timestamp": now_iso() })))
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
        return fail(StatusCode::UNAUTHORIZED, "LINE channel secret is not configured");
    };
    let Some(signature) = headers.get("x-line-signature").and_then(|v| v.to_str().ok()) else {
        security_event(&state.db, "missing_signature", "high", "line", ip.as_deref(), json!({}))
            .await;
        return fail(StatusCode::UNAUTHORIZED, "Missing signature header");
    };
    let expected = B64.encode(hmac_sha256(secret, &body));
    if signature != expected {
        security_event(&state.db, "invalid_signature", "high", "line", ip.as_deref(), json!({}))
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
        let result: Result<(), String> = match event.get("type").and_then(Value::as_str) {
            Some("message") => match event.get("message").filter(|m| m.is_object()) {
                Some(message) => {
                    let normalized = parse::normalize_line(message);
                    let mid = message.get("id").and_then(Value::as_str);
                    let user_id =
                        event["source"]["userId"].as_str().unwrap_or_default().to_string();
                    ingest::ingest_message(
                        &state,
                        InboundMessage {
                            platform: "line",
                            platform_user_id: &user_id,
                            default_display_name: "LINE User",
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
            last_error = Some(e);
        }
    }

    // 10. On partial failure: record (and defer-dispatch) an alert, report the
    //     whole batch as failed so the platform retries (CRD 2747, 2761).
    if failed > 0 {
        security_event(
            &state.db,
            "webhook_processing_failure",
            "high",
            "line",
            ip.as_deref(),
            json!({ "failed": failed, "total": total, "lastError": last_error }),
        )
        .await;
        // TODO(alerts): when an external alert sink URL is configured, post a
        // formatted alert message to it (best-effort, CRD 2859).
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("{failed} of {total} events failed"),
        );
    }
    batch_ok()
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
    let Some(signature) = headers.get("x-hub-signature-256").and_then(|v| v.to_str().ok()) else {
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
    let presented = signature.strip_prefix("sha256=").unwrap_or(signature);
    let expected = hex(&hmac_sha256(secret, &body));
    if !presented.eq_ignore_ascii_case(&expected) {
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
    let object = payload.get("object").and_then(Value::as_str).unwrap_or_default();
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

    // 6. Only the page object type is processed (CRD 2795).
    let mut total = 0usize;
    let mut failed = 0usize;
    let mut last_error: Option<String> = None;
    if object == "page" {
        for entry in entries.unwrap_or(&Vec::new()) {
            let Some(items) = entry.get("messaging").and_then(Value::as_array) else { continue };
            for item in items {
                let sender = item["sender"]["id"].as_str().unwrap_or_default().to_string();
                if let Some(message) = item.get("message").filter(|m| m.is_object()) {
                    // Skip the page's own echoed messages (would duplicate our outbound).
                    if message.get("is_echo").and_then(Value::as_bool).unwrap_or(false) {
                        continue;
                    }
                    total += 1;
                    let normalized = parse::normalize_facebook(message);
                    let mid = message.get("mid").and_then(Value::as_str);
                    if let Err(e) = ingest::ingest_message(
                        &state,
                        InboundMessage {
                            platform: "facebook",
                            platform_user_id: &sender,
                            default_display_name: "Facebook User",
                            platform_message_id: mid,
                            normalized,
                        },
                    )
                    .await
                    {
                        failed += 1;
                        last_error = Some(e);
                    }
                } else if let Some(postback) = item.get("postback") {
                    total += 1;
                    let normalized = parse::normalize_facebook_postback(postback);
                    if let Err(e) = ingest::ingest_message(
                        &state,
                        InboundMessage {
                            platform: "facebook",
                            platform_user_id: &sender,
                            default_display_name: "Facebook User",
                            platform_message_id: None,
                            normalized,
                        },
                    )
                    .await
                    {
                        failed += 1;
                        last_error = Some(e);
                    }
                } else if let Some(delivery) = item.get("delivery") {
                    let mids: Vec<&str> = delivery
                        .get("mids")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    ingest::mark_delivered(&state.db, &mids).await;
                } else if let Some(read) = item.get("read") {
                    if let Some(wm) = read.get("watermark").and_then(Value::as_i64) {
                        ingest::mark_read(&state.db, "facebook", &sender, wm).await;
                    }
                }
            }
        }
    }

    if failed > 0 {
        security_event(
            &state.db,
            "webhook_processing_failure",
            "high",
            "facebook",
            ip.as_deref(),
            json!({ "failed": failed, "total": total, "lastError": last_error }),
        )
        .await;
        // TODO(alerts): optional external alert dispatch (CRD 2859).
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("{failed} of {total} Facebook events failed"),
        );
    }
    batch_ok()
}

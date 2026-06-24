//! Inbound webhook ingestion per CRD §4.2 (lines 2720-2861).

mod common;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use common::{spawn_app, spawn_app_custom, TestApp};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

fn line_sig(body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-line-secret").unwrap();
    mac.update(body.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

fn fb_sig(body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-fb-secret").unwrap();
    mac.update(body.as_bytes());
    let hex: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

/// Send the exact raw body (no JSON re-serialization, so signatures stay valid)
/// with arbitrary headers.
async fn send_raw(
    app: &TestApp,
    method: &str,
    path: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let resp = app
        .router
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post_line(app: &TestApp, body: &str, sig: Option<&str>) -> (StatusCode, Value) {
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(s) = sig {
        headers.push(("x-line-signature", s));
    }
    let (status, text) = send_raw(app, "POST", "/api/webhook", body, &headers).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn line_text_event(user: &str, mid: &str, text: &str) -> String {
    json!({
        "destination": "dst",
        "events": [{
            "type": "message",
            "timestamp": 1700000000000i64,
            "source": { "type": "user", "userId": user },
            "message": { "id": mid, "type": "text", "text": text }
        }]
    })
    .to_string()
}

// ---------------------------------------------------------------- LINE probe & gates

#[tokio::test]
async fn line_probe_reports_readiness() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/webhook", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["endpoint"], "/api/webhook");
    assert_eq!(body["method"], "POST");
    assert!(body["timestamp"].is_string());
}

#[tokio::test]
async fn line_rejects_oversize_body_before_anything_else() {
    let app = spawn_app().await;
    let big = format!("{{\"events\":[],\"pad\":\"{}\"}}", "x".repeat(1024 * 1024));
    // No signature at all: size must be checked first (CRD 2737).
    let (status, body) = post_line(&app, &big, None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "Payload too large");
}

#[tokio::test]
async fn line_rejects_missing_or_bad_signature() {
    let app = spawn_app().await;
    let body = line_text_event("U1", "m-1", "hi");

    let (status, resp) = post_line(&app, &body, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(resp["error"], "Missing signature header");

    let (status, resp) = post_line(&app, &body, Some("bad-signature")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(resp["error"], "Invalid signature");

    // Security events recorded for both rejections.
    let events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_security_events WHERE platform = 'line'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(events, 2);

    // No customer/message side effects.
    let customers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customers")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(customers, 0);
}

#[tokio::test]
async fn line_rejects_when_secret_unconfigured() {
    let app = spawn_app_custom(|c| c.line_channel_secret = None).await;
    let body = line_text_event("U1", "m-1", "hi");
    let sig = line_sig(&body);
    let (status, resp) = post_line(&app, &body, Some(&sig)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(resp["error"].as_str().unwrap().to_lowercase().contains("secret"));
}

#[tokio::test]
async fn line_rejects_malformed_payloads() {
    let app = spawn_app().await;

    let junk = "not json at all";
    let (status, resp) = post_line(&app, junk, Some(&line_sig(junk))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(resp["error"], "Invalid JSON payload");

    let non_object = "[1,2,3]";
    let (status, resp) = post_line(&app, non_object, Some(&line_sig(non_object))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(resp["error"], "Invalid webhook payload");

    let no_events = json!({"destination": "d"}).to_string();
    let (status, resp) = post_line(&app, &no_events, Some(&line_sig(&no_events))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(resp["errors"].as_array().unwrap()[0]
        .as_str()
        .unwrap()
        .contains("events"));
}

// ------------------------------------------------------- LINE ingestion & dedup

#[tokio::test]
async fn line_text_message_creates_customer_conversation_and_message() {
    let app = spawn_app().await;
    let body = line_text_event("U-alpha", "mid-1", "hello support");
    let (status, resp) = post_line(&app, &body, Some(&line_sig(&body))).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["success"], true);
    assert!(resp["data"].is_null());

    let (cust_id, name): (i64, String) = sqlx::query_as(
        "SELECT id, display_name FROM customers WHERE platform = 'line' AND platform_user_id = 'U-alpha'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(name, "LINE User");

    let (conv_id, status_s): (String, String) =
        sqlx::query_as("SELECT id, status FROM conversations WHERE customer_id = $1")
            .bind(cust_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(status_s, "active");

    let (content, sender_type, delivery): (String, String, String) = sqlx::query_as(
        "SELECT content, sender_type, delivery_status FROM messages WHERE conversation_id = $1 AND platform_message_id = 'mid-1'",
    )
    .bind(&conv_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(content, "hello support");
    assert_eq!(sender_type, "customer");
    assert_eq!(delivery, "delivered");
}

#[tokio::test]
async fn line_inbound_without_token_keeps_placeholder_name() {
    // No LINE access token in the test harness, so the inbound profile fetch
    // must no-op (no network call) and the placeholder "LINE User" is kept.
    let app = spawn_app().await;
    let body = line_text_event("U-noprofile", "mid-np", "hi there");
    let (status, resp) = post_line(&app, &body, Some(&line_sig(&body))).await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let name: String = sqlx::query_scalar(
        "SELECT display_name FROM customers WHERE platform = 'line' AND platform_user_id = 'U-noprofile'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(name, "LINE User");
}

#[tokio::test]
async fn line_redelivery_is_idempotent() {
    let app = spawn_app().await;
    let body = line_text_event("U-dup", "mid-dup", "once");
    let sig = line_sig(&body);
    let (s1, _) = post_line(&app, &body, Some(&sig)).await;
    assert_eq!(s1, StatusCode::OK);

    let marker_before: Option<String> = sqlx::query_scalar(
        "SELECT last_message_at FROM conversations c JOIN customers cu ON cu.id = c.customer_id
         WHERE cu.platform_user_id = 'U-dup'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();

    let (s2, _) = post_line(&app, &body, Some(&sig)).await;
    assert_eq!(s2, StatusCode::OK, "redelivery still reports success");

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE platform_message_id = 'mid-dup'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(count, 1, "no duplicate message row (CRD 2768)");

    let marker_after: Option<String> = sqlx::query_scalar(
        "SELECT last_message_at FROM conversations c JOIN customers cu ON cu.id = c.customer_id
         WHERE cu.platform_user_id = 'U-dup'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(marker_before, marker_after, "activity marker only advances on insert (CRD 2771)");
}

#[tokio::test]
async fn line_open_conversation_is_reused_but_closed_is_not() {
    let app = spawn_app().await;
    for (mid, text) in [("m-a", "first"), ("m-b", "second")] {
        let body = line_text_event("U-reuse", mid, text);
        post_line(&app, &body, Some(&line_sig(&body))).await;
    }
    let convs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversations c JOIN customers cu ON cu.id = c.customer_id
         WHERE cu.platform_user_id = 'U-reuse'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(convs, 1, "open conversation reused");

    sqlx::query("UPDATE conversations SET status = 'closed' WHERE 1=1")
        .execute(&app.state.db)
        .await
        .unwrap();
    let body = line_text_event("U-reuse", "m-c", "after close");
    post_line(&app, &body, Some(&line_sig(&body))).await;
    let convs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversations c JOIN customers cu ON cu.id = c.customer_id
         WHERE cu.platform_user_id = 'U-reuse'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(convs, 2, "closed conversations are never reused (CRD 2851)");
}

#[tokio::test]
async fn line_media_normalization_uses_placeholders() {
    let app = spawn_app().await;
    let body = json!({
        "destination": "d",
        "events": [
            { "type": "message", "timestamp": 1, "source": {"userId": "U-m"},
              "message": {"id": "mm-1", "type": "image"} },
            { "type": "message", "timestamp": 2, "source": {"userId": "U-m"},
              "message": {"id": "mm-2", "type": "file", "fileName": "doc.pdf", "fileSize": 9} },
            { "type": "message", "timestamp": 3, "source": {"userId": "U-m"},
              "message": {"id": "mm-3", "type": "frobnicate"} }
        ]
    })
    .to_string();
    let (status, _) = post_line(&app, &body, Some(&line_sig(&body))).await;
    assert_eq!(status, StatusCode::OK);

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT platform_message_id, content_type FROM messages ORDER BY platform_message_id",
    )
    .fetch_all(&app.state.db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].1, "image");
    assert_eq!(rows[1].1, "file");

    let file_content: String =
        sqlx::query_scalar("SELECT content FROM messages WHERE platform_message_id = 'mm-2'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(file_content.contains("doc.pdf"), "file placeholder carries the name: {file_content}");

    let unknown_content: String =
        sqlx::query_scalar("SELECT content FROM messages WHERE platform_message_id = 'mm-3'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(
        unknown_content.contains("frobnicate"),
        "unsupported kind echoed in bracketed label: {unknown_content}"
    );
}

// ------------------------------------------------------- LINE follow / unfollow

#[tokio::test]
async fn follow_with_routing_creates_team_conversation_and_welcome() {
    let app = spawn_app().await;
    let team = app.seed_team("Routing").await;
    sqlx::query(
        "INSERT INTO customer_team_assignments (id, platform_user_id, team_id, source, assigned_at)
         VALUES ('a1', 'U-follow', $1, 'scan', $2)",
    )
    .bind(team)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let body = json!({
        "destination": "d",
        "events": [{ "type": "follow", "timestamp": 1, "source": {"userId": "U-follow"} }]
    })
    .to_string();
    let (status, _) = post_line(&app, &body, Some(&line_sig(&body))).await;
    assert_eq!(status, StatusCode::OK);

    let (cust_id, meta): (i64, Option<String>) = sqlx::query_as(
        "SELECT id, metadata FROM customers WHERE platform_user_id = 'U-follow'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let meta: Value = serde_json::from_str(&meta.unwrap_or_default()).unwrap_or_default();
    assert!(meta.get("lastFollowedAt").is_some(), "follow metadata recorded: {meta}");

    let (conv_team, conv_status): (Option<i64>, String) =
        sqlx::query_as("SELECT team_id, status FROM conversations WHERE customer_id = $1")
            .bind(cust_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(conv_team, Some(team), "auto-routed to the assigned team");
    assert_eq!(conv_status, "active");

    // Default welcome stored as a system-authored message so the conversation
    // is not empty (CRD 2822).
    let (sender_type, count): (String, i64) = sqlx::query_as(
        "SELECT MAX(m.sender_type), COUNT(*) FROM messages m JOIN conversations c ON c.id = m.conversation_id
         WHERE c.customer_id = $1",
    )
    .bind(cust_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(sender_type, "system");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn follow_without_user_id_is_silently_ignored() {
    let app = spawn_app().await;
    let body = json!({
        "destination": "d",
        "events": [{ "type": "follow", "timestamp": 1, "source": {} }]
    })
    .to_string();
    let (status, resp) = post_line(&app, &body, Some(&line_sig(&body))).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let customers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customers")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(customers, 0);
}

#[tokio::test]
async fn unfollow_touches_existing_customer_and_ignores_unknown() {
    let app = spawn_app().await;
    let cust = app.seed_customer("line", "U-out", "Leaver", None).await;
    let body = json!({
        "destination": "d",
        "events": [
            { "type": "unfollow", "timestamp": 1, "source": {"userId": "U-out"} },
            { "type": "unfollow", "timestamp": 2, "source": {"userId": "U-ghost"} }
        ]
    })
    .to_string();
    let (status, _) = post_line(&app, &body, Some(&line_sig(&body))).await;
    assert_eq!(status, StatusCode::OK);

    let updated: Option<String> =
        sqlx::query_scalar("SELECT updated_at FROM customers WHERE id = $1")
            .bind(cust)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(updated.is_some(), "last-updated marker advanced");
    let ghosts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM customers WHERE platform_user_id = 'U-ghost'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(ghosts, 0, "unfollow of unknown user is a no-op");
}

// ---------------------------------------------------------------- Facebook

#[tokio::test]
async fn facebook_handshake_echoes_challenge_only_with_correct_token() {
    let app = spawn_app().await;
    let (status, text) = send_raw(
        &app,
        "GET",
        "/api/webhooks/facebook?hub.mode=subscribe&hub.verify_token=test-verify-token&hub.challenge=c4f3",
        "",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(text, "c4f3", "challenge echoed verbatim");

    let (status, text) = send_raw(
        &app,
        "GET",
        "/api/webhooks/facebook?hub.mode=subscribe&hub.verify_token=wrong&hub.challenge=c4f3",
        "",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["error"], "Webhook verification failed");
}

fn fb_text_body(sender: &str, mid: &str, text: &str) -> String {
    json!({
        "object": "page",
        "entry": [{
            "id": "page-1",
            "time": 1700000000000i64,
            "messaging": [{
                "sender": {"id": sender},
                "recipient": {"id": "page-1"},
                "timestamp": 1700000000000i64,
                "message": {"mid": mid, "text": text}
            }]
        }]
    })
    .to_string()
}

#[tokio::test]
async fn facebook_delivery_verifies_signature_and_ingests() {
    let app = spawn_app().await;
    let body = fb_text_body("F-1", "fb-mid-1", "fb hello");

    // Bad signature rejected.
    let (status, resp) = send_raw(&app, "POST", "/api/webhooks/facebook", &body,
        &[("x-hub-signature-256", "sha256=deadbeef")]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{resp}");

    // Valid signature ingests.
    let sig = fb_sig(&body);
    let (status, resp) = send_raw(&app, "POST", "/api/webhooks/facebook", &body,
        &[("x-hub-signature-256", sig.as_str())]).await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let (content, platform): (String, String) = sqlx::query_as(
        "SELECT m.content, cu.platform FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         JOIN customers cu ON cu.id = c.customer_id
         WHERE m.platform_message_id = 'fb-mid-1'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(content, "fb hello");
    assert_eq!(platform, "facebook");
}

/// Build a signed `page` payload from a single `messaging` item and POST it.
async fn post_fb_item(app: &TestApp, item: Value) -> (StatusCode, Value) {
    let body = json!({
        "object": "page",
        "entry": [{ "id": "page-1", "time": 1700000000000i64, "messaging": [item] }]
    })
    .to_string();
    let sig = fb_sig(&body);
    let (status, text) =
        send_raw(app, "POST", "/api/webhooks/facebook", &body, &[("x-hub-signature-256", sig.as_str())]).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test]
async fn facebook_echo_messages_are_skipped() {
    let app = spawn_app().await;
    let (status, resp) = post_fb_item(
        &app,
        json!({
            "sender": {"id": "F-echo"},
            "recipient": {"id": "page-1"},
            "timestamp": 1700000000000i64,
            "message": {"mid": "echo-mid-1", "is_echo": true, "text": "x"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    // No inbound message and no customer created for the echoed event.
    let msgs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(msgs, 0, "page's own echoed message is not ingested");
    let customers: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM customers WHERE platform_user_id = 'F-echo'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(customers, 0);
}

#[tokio::test]
async fn facebook_postback_is_ingested_as_text() {
    let app = spawn_app().await;
    let (status, resp) = post_fb_item(
        &app,
        json!({
            "sender": {"id": "F-pb"},
            "recipient": {"id": "page-1"},
            "timestamp": 1700000000000i64,
            "postback": {"title": "Get Started", "payload": "START"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let (content, content_type): (String, String) = sqlx::query_as(
        "SELECT m.content, m.content_type FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         JOIN customers cu ON cu.id = c.customer_id
         WHERE cu.platform = 'facebook' AND cu.platform_user_id = 'F-pb'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(content, "Get Started");
    assert_eq!(content_type, "text");
}

#[tokio::test]
async fn facebook_delivery_marks_messages_delivered() {
    let app = spawn_app().await;
    let cust = app.seed_customer("facebook", "F-del", "Facebook User", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    // Seed an outbound agent message with a known platform_message_id, initially
    // not yet delivered, so the receipt has an observable effect.
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, content, content_type,
                               platform_message_id, is_sent, sent_at, delivery_status, created_at)
         VALUES ($1, $2, 'agent', 'hi', 'text', 'del-mid-1', 1, $3, 'sent', $3)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&conv)
    .bind(&now)
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, resp) = post_fb_item(
        &app,
        json!({
            "sender": {"id": "F-del"},
            "recipient": {"id": "page-1"},
            "timestamp": 1700000000000i64,
            "delivery": {"mids": ["del-mid-1"], "watermark": 1700000000000i64}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let delivery: String =
        sqlx::query_scalar("SELECT delivery_status FROM messages WHERE platform_message_id = 'del-mid-1'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(delivery, "delivered");
}

#[tokio::test]
async fn facebook_read_stamps_read_at_via_watermark() {
    let app = spawn_app().await;
    let cust = app.seed_customer("facebook", "F-read", "Facebook User", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    // An agent message sent in the SAME second as the read watermark, stamped in
    // the canonical ISO form real code writes (now_iso / to_rfc3339_opts(Millis,
    // true) → `Z`-suffixed millis). read_at starts NULL. This exercises the
    // same-second case that the old `+00:00` watermark form dropped: as TEXT,
    // "...20.000Z" > "...20+00:00", so `sent_at <= watermark` was false.
    let sent_at = chrono::DateTime::from_timestamp_millis(1700000000000)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true); // 2023-11-14T22:13:20.000Z
    assert_eq!(sent_at, "2023-11-14T22:13:20.000Z");
    let msg_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, content, content_type,
                               is_sent, sent_at, delivery_status, created_at)
         VALUES ($1, $2, 'agent', 'hi', 'text', 1, $3, 'delivered', $3)",
    )
    .bind(&msg_id)
    .bind(&conv)
    .bind(&sent_at)
    .execute(&app.state.db)
    .await
    .unwrap();

    // Watermark equal to the message's sent_at instant (same second), proving
    // same-second receipts now stamp `read_at`.
    let (status, resp) = post_fb_item(
        &app,
        json!({
            "sender": {"id": "F-read"},
            "recipient": {"id": "page-1"},
            "timestamp": 1700000000000i64,
            "read": {"watermark": 1700000000000i64}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let read_at: Option<String> =
        sqlx::query_scalar("SELECT read_at FROM messages WHERE id = $1")
            .bind(&msg_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(read_at.is_some(), "agent message read_at stamped by the watermark");

    // Collation-independent regression guard against the actual production
    // conversion. The `sent_at <= $2` compare runs as TEXT under the deployment's
    // collation; on byte-ordered (`C`) deployments `+` (0x2B) sorts before `.`
    // (0x2E), so the OLD watermark form `2023-11-14T22:13:20+00:00` is < the
    // canonical `2023-11-14T22:13:20.000Z` sent_at, silently dropping the
    // same-second receipt. Feed the EXACT watermark string `mark_read` builds
    // (via the shared helper) into the byte-ordered compare: this FAILS for the
    // old `to_rfc3339()` form and PASSES for the canonical `Millis`/`Z` form,
    // regardless of the test server's locale.
    let watermark_iso =
        mcss_backend::domain::webhooks::ingest::watermark_to_iso(1700000000000).unwrap();
    let byte_ordered_match: bool =
        sqlx::query_scalar("SELECT ($1 COLLATE \"C\") <= ($2 COLLATE \"C\")")
            .bind(&sent_at)
            .bind(&watermark_iso)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(
        byte_ordered_match,
        "same-second receipt must match under byte-ordered collation: \
         sent_at={sent_at} watermark={watermark_iso}"
    );
}

#[tokio::test]
async fn facebook_user_object_is_accepted_but_not_processed() {
    let app = spawn_app().await;
    // The `user` object type is a valid envelope. Even when it carries a REAL
    // message item we would otherwise ingest, the `object` guard skips it:
    // accepted (200) with no side effects (CRD 2794).
    let body = json!({
        "object": "user",
        "entry": [{
            "id": "u-1",
            "time": 1,
            "messaging": [{
                "sender": {"id": "U-obj-user"},
                "recipient": {"id": "u-1"},
                "timestamp": 1700000000000i64,
                "message": {"mid": "user-obj-mid", "text": "should be ignored"}
            }]
        }]
    })
    .to_string();
    let sig = fb_sig(&body);
    let (status, _) = send_raw(&app, "POST", "/api/webhooks/facebook", &body,
        &[("x-hub-signature-256", sig.as_str())]).await;
    assert_eq!(status, StatusCode::OK);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "the user object type is not processed (CRD 2794)");
    let customers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customers")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(customers, 0, "the object guard, not an empty array, prevents processing (CRD 2794)");
}

// ---------------------------------------------------------------- Instagram

/// Build a signed `instagram` payload from a single `messaging` item and POST it.
async fn post_ig_item(app: &TestApp, item: Value) -> (StatusCode, Value) {
    let body = json!({
        "object": "instagram",
        "entry": [{ "id": "ig-page-1", "time": 1700000000000i64, "messaging": [item] }]
    })
    .to_string();
    let sig = fb_sig(&body);
    let (status, text) =
        send_raw(app, "POST", "/api/webhooks/facebook", &body, &[("x-hub-signature-256", sig.as_str())]).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test]
async fn instagram_message_creates_instagram_customer_and_message() {
    let app = spawn_app().await;
    let (status, resp) = post_ig_item(
        &app,
        json!({
            "sender": {"id": "IG-user-1"},
            "recipient": {"id": "ig-page-1"},
            "timestamp": 1700000000000i64,
            "message": {"mid": "ig-mid-1", "text": "ig hello"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let (content, platform, name): (String, String, String) = sqlx::query_as(
        "SELECT m.content, cu.platform, cu.display_name FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         JOIN customers cu ON cu.id = c.customer_id
         WHERE m.platform_message_id = 'ig-mid-1'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(content, "ig hello");
    assert_eq!(platform, "instagram");
    assert_eq!(name, "Instagram User");
}

#[tokio::test]
async fn instagram_echo_messages_are_skipped() {
    let app = spawn_app().await;
    let (status, resp) = post_ig_item(
        &app,
        json!({
            "sender": {"id": "IG-echo"},
            "recipient": {"id": "ig-page-1"},
            "timestamp": 1700000000000i64,
            "message": {"mid": "ig-echo-mid-1", "is_echo": true, "text": "x"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let msgs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(msgs, 0, "instagram echoed message is not ingested");
}

#[tokio::test]
async fn instagram_reaction_records_metadata() {
    let app = spawn_app().await;
    let cust = app.seed_customer("instagram", "IG-react", "Instagram User", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    // Seed a customer message with a known platform_message_id and empty-object
    // metadata so the reaction has a target to update.
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, customer_id, content, content_type,
                               platform_message_id, is_sent, sent_at, delivery_status, metadata, created_at)
         VALUES ($1, $2, 'customer', $3, 'hi', 'text', 'ig-react-mid', 1, $4, 'delivered', '{}', $4)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&conv)
    .bind(cust)
    .bind(&now)
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, resp) = post_ig_item(
        &app,
        json!({
            "sender": {"id": "IG-react"},
            "recipient": {"id": "ig-page-1"},
            "timestamp": 1700000000000i64,
            "reaction": {"mid": "ig-react-mid", "action": "react", "reaction": "love", "emoji": "❤️"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let meta: String =
        sqlx::query_scalar("SELECT metadata FROM messages WHERE platform_message_id = 'ig-react-mid'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    let reactions = meta["reactions"].as_array().expect("reactions array present");
    assert_eq!(reactions.len(), 1, "one reaction recorded: {meta}");
    assert_eq!(reactions[0]["reaction"], "love");
    assert_eq!(reactions[0]["emoji"], "❤️");
}

#[tokio::test]
async fn instagram_unreaction_removes_reaction() {
    let app = spawn_app().await;
    let cust = app.seed_customer("instagram", "IG-unreact", "Instagram User", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    // Seed a customer message with a known platform_message_id and empty-object
    // metadata so the reaction has a target to update.
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, customer_id, content, content_type,
                               platform_message_id, is_sent, sent_at, delivery_status, metadata, created_at)
         VALUES ($1, $2, 'customer', $3, 'hi', 'text', 'ig-unreact-mid', 1, $4, 'delivered', '{}', $4)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&conv)
    .bind(cust)
    .bind(&now)
    .execute(&app.state.db)
    .await
    .unwrap();

    // React: the entry is appended.
    let (status, resp) = post_ig_item(
        &app,
        json!({
            "sender": {"id": "IG-unreact"},
            "recipient": {"id": "ig-page-1"},
            "timestamp": 1700000000000i64,
            "reaction": {"mid": "ig-unreact-mid", "action": "react", "reaction": "love", "emoji": "❤️"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let meta: String =
        sqlx::query_scalar("SELECT metadata FROM messages WHERE platform_message_id = 'ig-unreact-mid'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    assert_eq!(
        meta["reactions"].as_array().expect("reactions array present").len(),
        1,
        "one reaction recorded after react: {meta}"
    );

    // Unreact: the matching entry is removed, leaving an empty array.
    let (status, resp) = post_ig_item(
        &app,
        json!({
            "sender": {"id": "IG-unreact"},
            "recipient": {"id": "ig-page-1"},
            "timestamp": 1700000000001i64,
            "reaction": {"mid": "ig-unreact-mid", "action": "unreact", "reaction": "love"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let meta: String =
        sqlx::query_scalar("SELECT metadata FROM messages WHERE platform_message_id = 'ig-unreact-mid'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    assert_eq!(
        meta["reactions"].as_array().expect("reactions array present").len(),
        0,
        "reaction removed after unreact: {meta}"
    );
}

#[tokio::test]
async fn instagram_seen_by_mid_stamps_read_at() {
    let app = spawn_app().await;
    let cust = app.seed_customer("instagram", "IG-seen", "Instagram User", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    // Seed an agent message with a known platform_message_id and sent_at; read_at
    // starts NULL. The IG "seen" event carries that mid.
    let sent_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let msg_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, content, content_type,
                               platform_message_id, is_sent, sent_at, delivery_status, created_at)
         VALUES ($1, $2, 'agent', 'hi', 'text', 'ig-seen-mid', 1, $3, 'delivered', $3)",
    )
    .bind(&msg_id)
    .bind(&conv)
    .bind(&sent_at)
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, resp) = post_ig_item(
        &app,
        json!({
            "sender": {"id": "IG-seen"},
            "recipient": {"id": "ig-page-1"},
            "timestamp": 1700000000000i64,
            "read": {"mid": "ig-seen-mid"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let read_at: Option<String> =
        sqlx::query_scalar("SELECT read_at FROM messages WHERE id = $1")
            .bind(&msg_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(read_at.is_some(), "agent message read_at stamped by the seen mid");
}

#[tokio::test]
async fn instagram_story_mention_is_labelled() {
    let app = spawn_app().await;
    let (status, resp) = post_ig_item(
        &app,
        json!({
            "sender": {"id": "IG-story"},
            "recipient": {"id": "ig-page-1"},
            "timestamp": 1700000000000i64,
            "message": {
                "mid": "ig-story-mid",
                "attachments": [{ "type": "story_mention", "payload": { "url": "https://x/s.jpg" } }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let content: String = sqlx::query_scalar(
        "SELECT content FROM messages WHERE platform_message_id = 'ig-story-mid'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(content, "[Story mention]");
}

#[tokio::test]
async fn facebook_declared_oversize_content_length_is_rejected() {
    let app = spawn_app().await;
    let body = fb_text_body("F-big", "fb-big", "x");
    let sig = fb_sig(&body);
    // The declared content-length triggers rejection before signature checks.
    let (status, resp) = send_raw(&app, "POST", "/api/webhooks/facebook", &body,
        &[("x-hub-signature-256", sig.as_str()), ("content-length", "2000000")]).await;
    // Either the framework rejects the mismatched length or the handler does;
    // the observable contract is a 4xx, specifically 413 when it reaches the handler.
    assert!(
        status == StatusCode::PAYLOAD_TOO_LARGE || status == StatusCode::BAD_REQUEST,
        "got {status}: {resp}"
    );
}

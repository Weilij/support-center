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
        sqlx::query_as("SELECT id, status FROM conversations WHERE customer_id = ?")
            .bind(cust_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(status_s, "active");

    let (content, sender_type, delivery): (String, String, String) = sqlx::query_as(
        "SELECT content, sender_type, delivery_status FROM messages WHERE conversation_id = ? AND platform_message_id = 'mid-1'",
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
         VALUES ('a1', 'U-follow', ?, 'scan', ?)",
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
        sqlx::query_as("SELECT team_id, status FROM conversations WHERE customer_id = ?")
            .bind(cust_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(conv_team, Some(team), "auto-routed to the assigned team");
    assert_eq!(conv_status, "active");

    // Default welcome stored as a system-authored message so the conversation
    // is not empty (CRD 2822).
    let (sender_type, count): (String, i64) = sqlx::query_as(
        "SELECT sender_type, COUNT(*) FROM messages m JOIN conversations c ON c.id = m.conversation_id
         WHERE c.customer_id = ?",
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
        sqlx::query_scalar("SELECT updated_at FROM customers WHERE id = ?")
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

#[tokio::test]
async fn facebook_non_page_objects_are_accepted_but_not_processed() {
    let app = spawn_app().await;
    let body = json!({
        "object": "instagram",
        "entry": [{
            "id": "ig-1", "time": 1,
            "messaging": [{
                "sender": {"id": "IG-1"}, "recipient": {"id": "ig-1"}, "timestamp": 1,
                "message": {"mid": "ig-mid-1", "text": "ig"}
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
    assert_eq!(count, 0, "only the page object type is processed (CRD 2794)");
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

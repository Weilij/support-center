//! Customer-side real-time channel tests (CRD §5.4 lines 3847-3974): the
//! channel WebSocket (fast path + session fallback), presence lifecycle,
//! notify fan-out triggers, header-driven message list/create, upload, and
//! the §2.3 `/api/customer-ws` registration into the same channel registry.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::ws::{connect_rejected, mint, serve, wait_for_event, ws_connect, Ws};
use common::{spawn_app, TestApp};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

struct Seeded {
    agent_id: String,
    conv: String,
}

async fn seed(app: &TestApp) -> Seeded {
    let agent_id = app.seed_agent("agent@cust.io", "Secret123!", "agent").await;
    let team_id = app.seed_team("Customer Channel Team").await;
    app.add_membership(&agent_id, team_id, "member", true).await;
    let customer = app.seed_customer("line", "U-line-1", "Line Customer", Some(team_id)).await;
    let conv = app.seed_conversation(customer, Some(team_id), "active").await;
    Seeded { agent_id, conv }
}

fn fast_ws(conv: &str, user: &str) -> String {
    format!("/api/customer-channel/ws?conversationId={conv}&preValidated=true&validatedUserId={user}")
}

async fn seed_auth_session(app: &TestApp, id: &str, agent_id: &str, data: Option<&str>, ttl_secs: i64) {
    let expires = (chrono::Utc::now() + chrono::Duration::seconds(ttl_secs))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    sqlx::query(
        "INSERT INTO auth_sessions (id, agent_id, data, expires_at, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(agent_id)
    .bind(data)
    .bind(expires)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();
}

async fn expect_silence(ws: &mut Ws) {
    use futures_util::StreamExt;
    let got = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
    assert!(got.is_err(), "expected no frame, got {got:?}");
}

// ------------------------------------------- channel websocket (CRD 3854-3871)

#[tokio::test]
async fn channel_ws_fast_path_presence_and_multi_tab_lifecycle() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    let mut alice = ws_connect(addr, &fast_ws(&s.conv, "alice")).await.unwrap();
    // Each accepted connection joins the live audience and triggers a
    // presence "connected" event to the *other* connections (CRD 3864, 3966).
    let mut bob1 = ws_connect(addr, &fast_ws(&s.conv, "bob")).await.unwrap();
    let ev = wait_for_event(&mut alice, "USER_CONNECTED").await;
    assert_eq!(ev["userId"], "bob");
    assert!(ev["timestamp"].is_string());

    // The joining socket gets no presence event for itself (audience is the
    // channel's *other* connections, CRD 3966).
    expect_silence(&mut bob1).await;

    // Multiple simultaneous connections per user are supported (CRD 3864).
    let bob2 = ws_connect(addr, &fast_ws(&s.conv, "bob")).await.unwrap();
    let ev = wait_for_event(&mut alice, "USER_CONNECTED").await;
    assert_eq!(ev["userId"], "bob");
    // bob's first tab also sees the second tab's presence event.
    wait_for_event(&mut bob1, "USER_CONNECTED").await;

    // Closing one of several tabs does not mark the user offline (CRD 3959).
    drop(bob2);
    let (status, body, _) = app
        .request_with_headers(
            "POST",
            "/api/customer-channel/notify-message",
            None,
            Some(json!({ "conversationId": s.conv, "message": { "content": "ping" } })),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    // alice's next frame is the message, not a disconnect.
    let ev = wait_for_event(&mut alice, "new_message").await;
    assert_eq!(ev["data"]["content"], "ping");

    // The user's last connection ending emits USER_DISCONNECTED (CRD 3959).
    drop(bob1);
    let ev = wait_for_event(&mut alice, "USER_DISCONNECTED").await;
    assert_eq!(ev["userId"], "bob");

    // Inbound client frames are accepted but have no observable effect
    // (CRD 3871, 3972).
    common::ws::send_json(&mut alice, json!({ "type": "typing" })).await;
    expect_silence(&mut alice).await;
}

#[tokio::test]
async fn channel_ws_error_contract_and_session_fallback() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    // Upgrade header absent -> plain HTTP 400 (CRD 3868, 3944).
    let (status, body, _) = app
        .request("GET", &fast_ws(&s.conv, "alice"), None, None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.is_null(), "plain-text body expected");

    // Fallback path with no session token -> 400 (CRD 3869).
    let (status, _) = connect_rejected(
        addr,
        &format!("/api/customer-channel/ws?conversationId={}", s.conv),
    )
    .await;
    assert_eq!(status, 400);

    // Unknown session -> 401 JSON with success:false (CRD 3870).
    let (status, body) = connect_rejected(
        addr,
        &format!("/api/customer-channel/ws?conversationId={}&sessionId=ghost", s.conv),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(body["success"], json!(false));
    assert!(body["error"].is_string());

    // Expired session -> 401 (CRD 3863).
    seed_auth_session(&app, "expired-sess", &s.agent_id, None, -60).await;
    let (status, _) = connect_rejected(
        addr,
        &format!("/api/customer-channel/ws?conversationId={}&sessionId=expired-sess", s.conv),
    )
    .await;
    assert_eq!(status, 401);

    // Unreadable stored session -> 401 (CRD 3863).
    seed_auth_session(&app, "garbled-sess", &s.agent_id, Some("{not json"), 3600).await;
    let (status, _) = connect_rejected(
        addr,
        &format!("/api/customer-channel/ws?conversationId={}&sessionId=garbled-sess", s.conv),
    )
    .await;
    assert_eq!(status, 401);

    // Valid session resolves identity from the stored profile (CRD 3863).
    seed_auth_session(&app, "live-sess", &s.agent_id, Some(r#"{"userId":"sess-user"}"#), 3600)
        .await;
    let mut watcher = ws_connect(addr, &fast_ws(&s.conv, "watcher")).await.unwrap();
    let _fallback = ws_connect(
        addr,
        &format!("/api/customer-channel/ws?conversationId={}&sessionId=live-sess", s.conv),
    )
    .await
    .unwrap();
    let ev = wait_for_event(&mut watcher, "USER_CONNECTED").await;
    assert_eq!(ev["userId"], "sess-user");

    // preValidated without a validated user id falls back to the session path
    // (CRD 3861) -> 400 when no session id is supplied either.
    let (status, _) = connect_rejected(
        addr,
        &format!("/api/customer-channel/ws?conversationId={}&preValidated=true", s.conv),
    )
    .await;
    assert_eq!(status, 400);
}

// --------------------------------------------- notify endpoints (CRD 3873-3887)

#[tokio::test]
async fn notify_message_broadcasts_and_reports_diagnostics() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    let mut alice = ws_connect(addr, &fast_ws(&s.conv, "alice")).await.unwrap();
    let mut bob = ws_connect(addr, &fast_ws(&s.conv, "bob")).await.unwrap();
    wait_for_event(&mut alice, "USER_CONNECTED").await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/customer-channel/notify-message",
            None,
            Some(json!({
                "conversationId": s.conv,
                "message": {
                    "id": "m-1",
                    "content": "hello",
                    "messageType": "text",
                    "senderType": "agent",
                    "senderId": "alice",
                },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    // Diagnostic block: total connections, distinct users, conversation id
    // (CRD 3877).
    assert_eq!(body["debug"]["totalConnections"], json!(2));
    assert_eq!(body["debug"]["connectedUsers"], json!(["alice", "bob"]));
    assert_eq!(body["debug"]["conversationId"], json!(s.conv));

    // Every open connection receives the event (CRD 3876); shape per CRD 3968:
    // lowercase type, top-level data, original message copy, platform
    // defaulting to LINE.
    for ws in [&mut alice, &mut bob] {
        let ev = wait_for_event(ws, "new_message").await;
        assert_eq!(ev["conversationId"], json!(s.conv));
        assert_eq!(ev["data"]["content"], "hello");
        assert_eq!(ev["data"]["platform"], "line");
        assert_eq!(ev["message"]["id"], "m-1");
        assert!(ev["timestamp"].is_string());
    }

    // Processing error (missing conversation id) -> 500 with success:false
    // (CRD 3878).
    let (status, body, _) = app
        .request("POST", "/api/customer-channel/notify-message", None, Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn notify_message_updated_broadcasts_attachment_data() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    let mut ws = ws_connect(addr, &fast_ws(&s.conv, "alice")).await.unwrap();

    let (status, body, _) = app
        .request(
            "POST",
            "/api/customer-channel/notify-message-updated",
            None,
            Some(json!({
                "conversationId": s.conv,
                "messageId": "m-9",
                "data": { "attachments": [{ "id": "a-1", "url": "/uploads/a1.png" }] },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "success": true }));

    let ev = wait_for_event(&mut ws, "message_updated").await;
    assert_eq!(ev["conversationId"], json!(s.conv));
    assert_eq!(ev["data"]["messageId"], "m-9");
    assert_eq!(ev["data"]["attachments"][0]["id"], "a-1");

    // Processing error -> 500 with success:false (CRD 3886).
    let (status, body, _) = app
        .request("POST", "/api/customer-channel/notify-message-updated", None, Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["success"], json!(false));
}

// ----------------------------------------------- message listing (CRD 3889-3901)

#[tokio::test]
async fn list_messages_pagination_cursor_and_attachments() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Missing conversation header -> 400 (CRD 3899).
    let (status, body, _) = app
        .request("GET", "/api/customer-channel/messages", None, None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));

    let m1 = app.seed_message(&s.conv, "customer", "first", Some("2026-06-01T10:00:00.000Z")).await;
    let m2 = app.seed_message(&s.conv, "agent", "second", Some("2026-06-01T11:00:00.000Z")).await;
    let m3 = app.seed_message(&s.conv, "customer", "third", Some("2026-06-01T12:00:00.000Z")).await;

    // Attachment on m2: with a stored object -> download link; without ->
    // inline only (CRD 3896).
    std::fs::create_dir_all(&app.state.config.upload_dir).unwrap();
    std::fs::write(
        std::path::Path::new(&app.state.config.upload_dir).join("obj-key.png"),
        b"png",
    )
    .unwrap();
    for (id, key) in [("att-with", Some("obj-key.png")), ("att-without", None::<&str>)] {
        sqlx::query(
            "INSERT INTO attachments (id, message_id, file_name, content_type, file_size, file_url, storage_key, created_at)
             VALUES ($1, $2, 'f.png', 'image/png', 3, '/uploads/obj-key.png', $3, $4)",
        )
        .bind(id)
        .bind(&m2)
        .bind(key)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&app.state.db)
        .await
        .unwrap();
    }

    let headers: &[(&str, &str)] = &[("x-conversation-id", s.conv.as_str())];
    // Newest first; full page implies hasMore (CRD 3897, 3901).
    let (status, body, _) = app
        .request_with_headers("GET", "/api/customer-channel/messages?limit=2", None, None, headers)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["hasMore"], json!(true));
    assert_eq!(body["messages"][0]["id"], json!(m3));
    assert_eq!(body["messages"][1]["id"], json!(m2));
    // Unified sender id + attachments with optional download link (CRD 3897).
    assert!(body["messages"][0]["senderId"].is_string()); // customer id as string
    let atts = body["messages"][1]["attachments"].as_array().unwrap();
    assert_eq!(atts.len(), 2);
    let with = atts.iter().find(|a| a["id"] == "att-with").unwrap();
    let without = atts.iter().find(|a| a["id"] == "att-without").unwrap();
    assert!(with["downloadUrl"].is_string());
    assert!(without["downloadUrl"].is_null());

    // "before" cursor restricts to strictly older messages (CRD 3896).
    let (_, body, _) = app
        .request_with_headers(
            "GET",
            &format!("/api/customer-channel/messages?before={m2}"),
            None,
            None,
            headers,
        )
        .await;
    let ids: Vec<&str> =
        body["messages"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![m1.as_str()]);
    assert_eq!(body["hasMore"], json!(false));

    // Unresolvable cursor degrades to the latest page (CRD 3896).
    let (_, body, _) = app
        .request_with_headers(
            "GET",
            "/api/customer-channel/messages?before=no-such-message",
            None,
            None,
            headers,
        )
        .await;
    assert_eq!(body["messages"].as_array().unwrap().len(), 3);
}

// ----------------------------------------------- message creation (CRD 3903-3924)

#[tokio::test]
async fn create_message_validation_contract() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Missing conversation header -> 400 (CRD 3921).
    let (status, body, _) = app
        .request("POST", "/api/customer-channel/messages", None, Some(json!({ "content": "x" })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));

    // Missing credential header -> 401 (CRD 3921).
    let (status, body, _) = app
        .request_with_headers(
            "POST",
            "/api/customer-channel/messages",
            None,
            Some(json!({ "content": "x" })),
            &[("x-conversation-id", s.conv.as_str())],
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], json!(false));

    // Neither content nor attachments -> 400 (CRD 3922).
    let (status, body, _) = app
        .request_with_headers(
            "POST",
            "/api/customer-channel/messages",
            None,
            Some(json!({ "content": "   " })),
            &[("x-conversation-id", s.conv.as_str()), ("x-session-token", "someone")],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn create_message_persists_links_broadcasts_and_round_trips_correlation() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    let mut viewer = ws_connect(addr, &fast_ws(&s.conv, "viewer")).await.unwrap();

    // Unlinked attachment to be claimed by the new message (CRD 3912).
    sqlx::query(
        "INSERT INTO attachments (id, file_name, content_type, file_size, file_url, storage_key, created_at)
         VALUES ('att-new', 'doc.pdf', 'application/pdf', 9, '/uploads/doc.pdf', 'doc.pdf', $1)",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    // Three-part signed credential: identity from the decoded middle segment
    // (CRD 3907).
    let token = mint(&s.agent_id, "agent", 3600);
    let (status, body, _) = app
        .request_with_headers(
            "POST",
            "/api/customer-channel/messages",
            None,
            Some(json!({
                "content": "outbound hello",
                "attachmentIds": ["att-new"],
                "correlationId": "corr-42",
            })),
            &[("x-conversation-id", s.conv.as_str()), ("x-session-token", token.as_str())],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    let msg = &body["message"];
    let message_id = msg["id"].as_str().unwrap().to_string();
    // Attachments force the file kind; correlation id round-trips (CRD 3911,
    // 3924).
    assert_eq!(msg["messageType"], "file");
    assert_eq!(msg["senderId"], json!(s.agent_id));
    assert_eq!(msg["correlationId"], "corr-42");
    assert_eq!(msg["attachments"][0]["id"], "att-new");

    // Persisted already sent/delivered and attributed to the agent (CRD 3911).
    let row: (String, String, i64, Option<String>) = sqlx::query_as(
        "SELECT sender_type, delivery_status, is_sent, agent_id FROM messages WHERE id = $1",
    )
    .bind(&message_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!((row.0.as_str(), row.1.as_str(), row.2), ("agent", "delivered", 1));
    assert_eq!(row.3.as_deref(), Some(s.agent_id.as_str()));

    // Attachment linked (CRD 3912).
    let linked: Option<String> =
        sqlx::query_scalar("SELECT message_id FROM attachments WHERE id = 'att-new'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(linked.as_deref(), Some(message_id.as_str()));

    // Conversation recency advanced (CRD 3913).
    let last: Option<String> =
        sqlx::query_scalar("SELECT last_message_at FROM conversations WHERE id = $1")
            .bind(&s.conv)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(last.is_some());

    // Channel fan-out uses the accepted payload immediately (CRD 3915, 3923).
    let ev = wait_for_event(&mut viewer, "new_message").await;
    assert_eq!(ev["data"]["senderType"], "agent");
    assert_eq!(ev["message"]["correlationId"], "corr-42");
    assert_eq!(ev["message"]["id"], json!(message_id));

    // Latest-message cache eventually reflects the new message (CRD 4170).
    let mut fresh = false;
    for _ in 0..50 {
        if let Some(snapshot) = app.state.realtime.latest.peek(&s.conv) {
            if snapshot["messageId"] == json!(message_id) {
                fresh = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(fresh, "latest-message cache did not refresh");

    // Raw (non-JWT) credential value is treated as the user id (CRD 3907).
    let (status, body, _) = app
        .request_with_headers(
            "POST",
            "/api/customer-channel/messages",
            None,
            Some(json!({ "content": "plain credential" })),
            &[("x-conversation-id", s.conv.as_str()), ("x-session-token", "opaque-user")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"]["senderId"], "opaque-user");
    assert_eq!(body["message"]["messageType"], "text");
}

// ----------------------------------------------------- file upload (CRD 3926-3941)

fn multipart_upload(
    path: &str,
    conv: Option<&str>,
    session: Option<&str>,
    file: Option<&[u8]>,
) -> Request<Body> {
    let boundary = "XCUSTBOUNDARYX";
    let mut body: Vec<u8> = Vec::new();
    if let Some(data) = file {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
                 filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    } else {
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    }
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"));
    if let Some(c) = conv {
        builder = builder.header("x-conversation-id", c);
    }
    if let Some(t) = session {
        builder = builder.header("x-session-token", t);
    }
    builder.body(Body::from(body)).unwrap()
}

async fn send(app: &TestApp, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn upload_stores_asset_without_creating_a_message() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    seed_auth_session(&app, "upload-sess", &s.agent_id, None, 3600).await;
    let path = "/api/customer-channel/upload";

    // Missing conversation header -> 400 (CRD 3937).
    let (status, body) =
        send(&app, multipart_upload(path, None, Some("upload-sess"), Some(b"png"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));

    // Missing session header -> 401 (CRD 3938).
    let (status, body) =
        send(&app, multipart_upload(path, Some(&s.conv), None, Some(b"png"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], json!(false));

    // Invalid session -> 401 with the validation error (CRD 3938).
    let (status, body) =
        send(&app, multipart_upload(path, Some(&s.conv), Some("ghost"), Some(b"png"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], json!(false));

    // No file part -> 400 (CRD 3939).
    let (status, body) =
        send(&app, multipart_upload(path, Some(&s.conv), Some("upload-sess"), None)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));

    // Success: stored under a conversation-namespaced unique key preserving
    // the extension; URL + name + size + type returned; no message created
    // (CRD 3933-3936, 3941).
    let (status, body) = send(
        &app,
        multipart_upload(path, Some(&s.conv), Some("upload-sess"), Some(b"pngdata")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["fileName"], "photo.png");
    assert_eq!(body["size"], json!(7));
    assert_eq!(body["contentType"], "image/png");
    let url = body["url"].as_str().unwrap();
    assert!(url.starts_with("/uploads/"), "unexpected url {url}");
    assert!(url.ends_with(".png"));
    let key = url.strip_prefix("/uploads/").unwrap();
    assert!(std::path::Path::new(&app.state.config.upload_dir).join(key).exists());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE conversation_id = $1")
        .bind(&s.conv)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// ----------------------------------------------- non-matching paths (CRD 3944)

#[tokio::test]
async fn unknown_channel_paths_answer_plain_404() {
    let app = spawn_app().await;
    let (status, body, _) =
        app.request("GET", "/api/customer-channel/no-such-path", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.is_null(), "plain-text body expected");
}

// ------------------------------- §2.3 customer-ws registration (CRD 1124-1146)

#[tokio::test]
async fn customer_ws_registers_into_the_channel_and_receives_replies() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    // Missing parameters -> 400 (CRD 1142).
    let (status, _) = connect_rejected(addr, "/api/customer-ws?conversationId=only").await;
    assert_eq!(status, 400);

    // Invalid session -> 401 (CRD 1143).
    let (status, _) = connect_rejected(
        addr,
        &format!("/api/customer-ws?conversationId={}&sessionId=bogus", s.conv),
    )
    .await;
    assert_eq!(status, 401);

    // Valid subscriber registers into the conversation's isolated channel;
    // a USER_CONNECTED presence event reaches the other subscribers
    // (CRD 1137-1139).
    let session = mint(&s.agent_id, "agent", 3600);
    let mut first = ws_connect(
        addr,
        &format!("/api/customer-ws?conversationId={}&sessionId={}", s.conv, session),
    )
    .await
    .unwrap();
    let _second = ws_connect(
        addr,
        &format!("/api/customer-ws?conversationId={}&sessionId={}", s.conv, session),
    )
    .await
    .unwrap();
    let ev = wait_for_event(&mut first, "USER_CONNECTED").await;
    assert_eq!(ev["userId"], json!(s.agent_id));

    // A reply created through the §2.3 message endpoint is delivered to every
    // live connection as a raw new_message frame (CRD 1162-1164).
    let (status, body, _) = app
        .request_with_headers(
            "POST",
            &format!("/api/customer-conversations/{}/messages", s.conv),
            None,
            Some(json!({ "content": "reply over ws", "correlationId": "cw-1" })),
            &[("x-session-id", session.as_str())],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "send_reply failed: {body}");
    let ev = wait_for_event(&mut first, "new_message").await;
    assert_eq!(ev["data"]["content"], "reply over ws");
    assert_eq!(ev["message"]["correlationId"], "cw-1");

    // notify-message also reaches §2.3 subscribers (same registry).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/customer-channel/notify-message",
            None,
            Some(json!({ "conversationId": s.conv, "message": { "content": "via notify" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let ev = wait_for_event(&mut first, "new_message").await;
    assert_eq!(ev["data"]["content"], "via notify");
}

//! Messaging behavior tests (CRD §2.2, lines 830-1042).

mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use common::{spawn_app, spawn_app_custom, TestApp};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tower::ServiceExt;

use mcss_backend::domain::messaging::service;

async fn mock_line_push_url() -> String {
    async fn capture(headers: HeaderMap, _body: axum::body::Bytes) -> (StatusCode, Json<Value>) {
        let token = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if token != "Bearer good-line" {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"message": "bad line token"})),
            );
        }
        (
            StatusCode::OK,
            Json(json!({"sentMessages": [{"id": "line-delayed"}]})),
        )
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/line/push", post(capture)))
            .await
            .unwrap();
    });
    format!("http://{addr}/line/push")
}

async fn admin(app: &TestApp) -> (String, String) {
    let id = app
        .seed_agent("admin@test.dev", "Secret123!", "admin")
        .await;
    let token = app.login("admin@test.dev", "Secret123!").await.0;
    (token, id)
}

async fn agent(app: &TestApp, email: &str, team_id: Option<i64>) -> (String, String) {
    let id = app.seed_agent(email, "Secret123!", "agent").await;
    if let Some(t) = team_id {
        app.add_membership(&id, t, "member", true).await;
    }
    let token = app.login(email, "Secret123!").await.0;
    (token, id)
}

/// Seed an agent-authored message with explicit recall fields.
async fn seed_agent_message(
    app: &TestApp,
    conversation_id: &str,
    agent_id: &str,
    content: &str,
    is_recalled: bool,
    recall_deadline: Option<&str>,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content, content_type,
                               is_sent, sent_at, delivery_status, is_recalled, recall_deadline,
                               sender_name, created_at)
         VALUES ($1, $2, 'agent', $3, $4, 'text', 1, $5, 'sent', $6, $7, 'seed agent', $8)",
    )
    .bind(&id)
    .bind(conversation_id)
    .bind(agent_id)
    .bind(content)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(is_recalled as i64)
    .bind(recall_deadline)
    .bind(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    .execute(&app.state.db)
    .await
    .unwrap();
    id
}

async fn set_message_metadata(app: &TestApp, message_id: &str, metadata: Value) {
    sqlx::query("UPDATE messages SET metadata = $1 WHERE id = $2")
        .bind(metadata.to_string())
        .bind(message_id)
        .execute(&app.state.db)
        .await
        .unwrap();
}

/// Standard fixture: admin token + customer + unassigned conversation.
async fn fixture(app: &TestApp) -> (String, String, i64, String) {
    let (token, admin_id) = admin(app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    (token, admin_id, cust, conv)
}

/// GET returning the raw body string (for CSV/TXT exports).
async fn raw_get(app: &TestApp, path: &str, token: &str) -> (StatusCode, HeaderMap, String) {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

fn multipart_request(
    path: &str,
    token: &str,
    filename: &str,
    content_type: &str,
    data: &[u8],
) -> Request<Body> {
    let boundary = "XTESTBOUNDARYX";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method("POST")
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

// ---------------------------------------------------------------- health & info

#[tokio::test]
async fn health_and_info_require_no_auth() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/messages/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], json!("healthy"));
    assert!(body["timestamp"].is_string());

    let (status, body, _) = app.request("GET", "/api/messages/info", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert!(body["data"]["features"].is_array());
    assert!(body["data"]["endpoints"].is_array());
}

#[tokio::test]
async fn protected_endpoints_require_auth() {
    let app = spawn_app().await;
    for (method, path) in [
        ("POST", "/api/messages"),
        ("GET", "/api/messages/some-id"),
        ("GET", "/api/messages/search"),
        ("GET", "/api/messages/stats"),
        ("GET", "/api/messages/export"),
    ] {
        let (status, _, _) = app.request(method, path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

// ---------------------------------------------------------------- create message

#[tokio::test]
async fn create_message_persists_and_bumps_conversation() {
    let app = spawn_app().await;
    let (token, admin_id, _cust, conv) = fixture(&app).await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": conv, "content": "hello world" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let data = &body["data"];
    assert!(data["id"].as_str().unwrap().starts_with("msg_"));
    assert_eq!(data["conversationId"], json!(conv));
    assert_eq!(data["content"], json!("hello world"));
    assert_eq!(data["messageType"], json!("text"));
    assert_eq!(data["senderType"], json!("agent"));
    assert_eq!(data["agentId"], json!(admin_id));

    // Sent state, sender snapshot, conversation recency bump.
    let (status, name, last_msg): (String, String, Option<String>) = sqlx::query_as(
        "SELECT m.delivery_status, m.sender_name, c.last_message_at
         FROM messages m JOIN conversations c ON c.id = m.conversation_id WHERE m.id = $1",
    )
    .bind(data["id"].as_str().unwrap())
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(status, "sent");
    assert_eq!(name, "admin user");
    assert!(last_msg.is_some());

    // Audit activity entry (CRD 855).
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action = 'message send' AND resource_id = $1",
    )
    .bind(data["id"].as_str().unwrap())
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(logged, 1);
}

#[tokio::test]
async fn create_message_validates_required_fields() {
    let app = spawn_app().await;
    let (token, _, _, conv) = fixture(&app).await;
    // Missing content.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": conv })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Blank content after trimming.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": conv, "content": "   " })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Missing conversationId.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "content": "hi" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Malformed JSON.
    let (status, _) = app
        .request_raw("POST", "/api/messages", Some(&token), "{not json")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_message_rejects_missing_or_deleted_conversation() {
    let app = spawn_app().await;
    let (token, _, cust, conv) = fixture(&app).await;
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": "nope", "content": "hi" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Soft-deleted conversation is treated as missing.
    let deleted = app.seed_conversation(cust, None, "active").await;
    sqlx::query("UPDATE conversations SET deleted_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(&deleted)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": deleted, "content": "hi" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let _ = conv;
}

#[tokio::test]
async fn create_message_enforces_team_scope() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "Secret123!", "admin")
        .await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (token, _) = agent(&app, "agent@test.dev", Some(team_a)).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let other_team_conv = app.seed_conversation(cust, Some(team_b), "assigned").await;
    let own_team_conv = app.seed_conversation(cust, Some(team_a), "assigned").await;
    let pool_conv = app.seed_conversation(cust, None, "active").await;

    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": other_team_conv, "content": "hi" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    for conv in [&own_team_conv, &pool_conv] {
        let (status, _, _) = app
            .request(
                "POST",
                "/api/messages",
                Some(&token),
                Some(json!({ "conversationId": conv, "content": "hi" })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }
}

#[tokio::test]
async fn create_message_validates_reply_target() {
    let app = spawn_app().await;
    let (token, admin_id, cust, conv) = fixture(&app).await;
    let other_conv = app.seed_conversation(cust, None, "active").await;
    let other_msg =
        seed_agent_message(&app, &other_conv, &admin_id, "elsewhere", false, None).await;

    // Reply target in a different conversation is invalid.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": conv, "content": "re", "replyToMessageId": other_msg })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown reply target is invalid.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": conv, "content": "re", "replyToMessageId": "missing" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Valid same-conversation reply target works.
    let target = seed_agent_message(&app, &conv, &admin_id, "original", false, None).await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({ "conversationId": conv, "content": "re", "replyToMessageId": target })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn create_message_links_attachments_and_detects_mentions() {
    let app = spawn_app().await;
    let (token, _, _, conv) = fixture(&app).await;
    // A single-word display name so the @-token matches.
    let bob_id = app.seed_agent("bob@test.dev", "Secret123!", "agent").await;
    sqlx::query("UPDATE agents SET display_name = 'bob' WHERE id = $1")
        .bind(&bob_id)
        .execute(&app.state.db)
        .await
        .unwrap();

    // Pre-uploaded, not-yet-linked attachment.
    let attachment_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO attachments (id, message_id, conversation_id, file_name, content_type,
                                  file_size, file_url, storage_key, created_at)
         VALUES ($1, NULL, $2, 'a.png', 'image/png', 10, '/uploads/a.png', 'a.png', $3)",
    )
    .bind(&attachment_id)
    .bind(&conv)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, body, _) = app
        .request(
            "POST",
            "/api/messages",
            Some(&token),
            Some(json!({
                "conversationId": conv,
                "content": "hey @bob please look",
                "attachmentIds": [attachment_id],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let data = &body["data"];
    assert_eq!(data["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(data["mentions"], json!([bob_id]));

    // Mention notification dispatched to bob.
    let notified: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE agent_id = $1 AND type = 'mention'",
    )
    .bind(&bob_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(notified, 1);
}

// ---------------------------------------------------------------- get message

#[tokio::test]
async fn get_message_returns_detail_view() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "details", false, None).await;
    let (status, body, _) = app
        .request("GET", &format!("/api/messages/{id}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["id"], json!(id));
    assert_eq!(data["conversationId"], json!(conv));
    assert_eq!(data["senderType"], json!("agent"));
    assert_eq!(data["content"], json!("details"));
    assert_eq!(data["isRecalled"], json!(false));
    assert_eq!(data["deliveryStatus"], json!("sent"));
    assert_eq!(data["senderInfo"]["id"], json!(admin_id));
    assert_eq!(data["conversationInfo"]["status"], json!("active"));
    assert_eq!(data["conversationInfo"]["priority"], json!("normal"));
}

#[tokio::test]
async fn get_message_hides_scope_violations_as_not_found() {
    let app = spawn_app().await;
    let (admin_token, admin_id) = admin(&app).await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (agent_token, _) = agent(&app, "agent@test.dev", Some(team_a)).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team_b), "assigned").await;
    let id = seed_agent_message(&app, &conv, &admin_id, "secret", false, None).await;

    // Out-of-scope read is reported as not found (CRD 861).
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/messages/{id}"),
            Some(&agent_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Admin sees it.
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/messages/{id}"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // Unknown id is 404; soft-deleted is 404.
    let (status, _, _) = app
        .request("GET", "/api/messages/missing-id", Some(&admin_token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    sqlx::query("UPDATE messages SET deleted_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(&id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/messages/{id}"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn message_collection_endpoints_enforce_team_scope() {
    let app = spawn_app().await;
    let (admin_token, admin_id) = admin(&app).await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (agent_token, _) = agent(&app, "agent@test.dev", Some(team_a)).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let visible_conv = app.seed_conversation(cust, Some(team_a), "assigned").await;
    let hidden_conv = app.seed_conversation(cust, Some(team_b), "assigned").await;
    let pool_conv = app.seed_conversation(cust, None, "active").await;
    let visible_msg =
        seed_agent_message(&app, &visible_conv, &admin_id, "visible team", false, None).await;
    let hidden_msg =
        seed_agent_message(&app, &hidden_conv, &admin_id, "hidden team", false, None).await;
    let pool_msg = seed_agent_message(&app, &pool_conv, &admin_id, "pool", false, None).await;

    let (status, body, _) = app
        .request(
            "GET",
            "/api/messages/search?q=team&limit=50",
            Some(&agent_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<&str> = body["data"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert!(ids.contains(&visible_msg.as_str()), "{body}");
    assert!(!ids.contains(&hidden_msg.as_str()), "{body}");

    let (status, body, _) = app
        .request(
            "GET",
            "/api/messages/export?format=json&limit=50",
            Some(&agent_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let exported: Vec<&str> = body["data"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert!(exported.contains(&visible_msg.as_str()), "{body}");
    assert!(exported.contains(&pool_msg.as_str()), "{body}");
    assert!(!exported.contains(&hidden_msg.as_str()), "{body}");

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/messages/conversation/{hidden_conv}"),
            Some(&agent_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body, _) = app
        .request(
            "GET",
            "/api/messages/export/count",
            Some(&agent_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["count"], json!(2), "{body}");

    let (status, body, _) = app
        .request(
            "GET",
            "/api/messages/export?format=json&limit=50",
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["exportInfo"]["totalRecords"],
        json!(3),
        "{body}"
    );
}

// ---------------------------------------------------------------- update message

#[tokio::test]
async fn update_message_applies_fields_for_author() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "before", false, None).await;
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{id}"),
            Some(&token),
            Some(json!({ "content": "after", "metadata": { "k": "v" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["content"], json!("after"));
    assert_eq!(body["data"]["metadata"]["k"], json!("v"));
    assert!(body["message"].as_str().unwrap().contains("updated"));
}

#[tokio::test]
async fn update_message_error_conditions() {
    let app = spawn_app().await;
    let (admin_token, admin_id) = admin(&app).await;
    let (other_token, _) = agent(&app, "other@test.dev", None).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let id = seed_agent_message(&app, &conv, &admin_id, "text", false, None).await;

    // Empty content when provided.
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{id}"),
            Some(&admin_token),
            Some(json!({ "content": " " })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Not found.
    let (status, _, _) = app
        .request(
            "PUT",
            "/api/messages/missing",
            Some(&admin_token),
            Some(json!({ "content": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Non-author, non-admin.
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{id}"),
            Some(&other_token),
            Some(json!({ "content": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Customer-origin messages are never editable.
    let cust_msg = app
        .seed_message(&conv, "customer", "from customer", None)
        .await;
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{cust_msg}"),
            Some(&admin_token),
            Some(json!({ "content": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Already recalled.
    let recalled = seed_agent_message(&app, &conv, &admin_id, "gone", true, None).await;
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{recalled}"),
            Some(&admin_token),
            Some(json!({ "content": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------- recall message

#[tokio::test]
async fn recall_message_overwrites_content_with_placeholder() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "sensitive", false, None).await;
    let (status, body, _) = app
        .request("DELETE", &format!("/api/messages/{id}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["isRecalled"], json!(true));
    assert_eq!(body["data"]["recalledBy"]["id"], json!(admin_id));
    assert!(body["data"]["recalledAt"].is_string());

    let (content, is_recalled, status_col): (String, i64, String) =
        sqlx::query_as("SELECT content, is_recalled, delivery_status FROM messages WHERE id = $1")
            .bind(&id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(content, "[Message recalled]");
    assert_eq!(is_recalled, 1);
    assert_eq!(status_col, "recalled");

    // Audit entry written.
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action = 'message recall' AND resource_id = $1",
    )
    .bind(&id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(logged, 1);
}

#[tokio::test]
async fn recall_message_error_conditions() {
    let app = spawn_app().await;
    let (admin_token, admin_id) = admin(&app).await;
    let (other_token, _) = agent(&app, "other@test.dev", None).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    // Not found.
    let (status, _, _) = app
        .request("DELETE", "/api/messages/missing", Some(&admin_token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Not author/admin.
    let id = seed_agent_message(&app, &conv, &admin_id, "x", false, None).await;
    let (status, _, _) = app
        .request(
            "DELETE",
            &format!("/api/messages/{id}"),
            Some(&other_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Already recalled.
    let recalled = seed_agent_message(&app, &conv, &admin_id, "x", true, None).await;
    let (status, _, _) = app
        .request(
            "DELETE",
            &format!("/api/messages/{recalled}"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Recall deadline passed.
    let expired = seed_agent_message(
        &app,
        &conv,
        &admin_id,
        "x",
        false,
        Some("2000-01-01T00:00:00.000Z"),
    )
    .await;
    let (status, _, _) = app
        .request(
            "DELETE",
            &format!("/api/messages/{expired}"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ------------------------------------------------------- list conversation messages

#[tokio::test]
async fn conversation_listing_paginates_and_filters() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    app.seed_message(&conv, "customer", "c1", Some("2026-01-01T00:00:00.000Z"))
        .await;
    seed_agent_message(&app, &conv, &admin_id, "a1", false, None).await;
    let recalled = seed_agent_message(&app, &conv, &admin_id, "gone", true, None).await;

    // Recalled excluded by default; newest first.
    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/messages/conversation/{conv}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let messages = body["data"]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["content"], json!("a1"));
    assert_eq!(messages[1]["content"], json!("c1"));
    assert_eq!(body["data"]["pagination"]["total"], json!(2));
    assert_eq!(body["data"]["pagination"]["pageSize"], json!(20));
    assert_eq!(body["data"]["filters"]["includeRecalled"], json!(false));

    // includeRecalled=true includes the recalled message.
    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/messages/conversation/{conv}?includeRecalled=true"),
            Some(&token),
            None,
        )
        .await;
    let ids: Vec<&str> = body["data"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&recalled.as_str()));

    // senderType filter + clamped page size.
    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/messages/conversation/{conv}?senderType=customer&pageSize=500&includeRecalled=true"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body["data"]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["pagination"]["pageSize"], json!(100));

    // Unknown conversation -> 404.
    let (status, _, _) = app
        .request(
            "GET",
            "/api/messages/conversation/missing",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- search & stats

#[tokio::test]
async fn search_matches_substring_and_filters() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    seed_agent_message(&app, &conv, &admin_id, "the quick brown fox", false, None).await;
    seed_agent_message(&app, &conv, &admin_id, "lazy dog", false, None).await;
    seed_agent_message(&app, &conv, &admin_id, "quick recalled", true, None).await;

    let (status, body, _) = app
        .request("GET", "/api/messages/search?q=quick", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["total"], json!(2));
    assert_eq!(body["data"]["pagination"]["limit"], json!(50));
    assert_eq!(body["data"]["pagination"]["hasMore"], json!(false));
    assert_eq!(body["data"]["query"]["q"], json!("quick"));
    let first = &body["data"]["messages"][0];
    assert!(first["senderName"].is_string());
    assert!(first["attachments"].is_array());
    assert!(first["reactions"].is_array());
    assert!(first["readBy"].is_array());

    // isRecalled filter narrows to the recalled one.
    let (_, body, _) = app
        .request(
            "GET",
            "/api/messages/search?q=quick&isRecalled=true",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body["data"]["total"], json!(1));

    // A LIKE wildcard in the term is treated literally.
    let (_, body, _) = app
        .request("GET", "/api/messages/search?q=%25", Some(&token), None)
        .await;
    assert_eq!(body["data"]["total"], json!(0));
}

#[tokio::test]
async fn stats_reports_totals_and_zero_breakdowns() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    seed_agent_message(&app, &conv, &admin_id, "one", false, None).await;
    seed_agent_message(&app, &conv, &admin_id, "two", false, None).await;
    let (status, body, _) = app
        .request("GET", "/api/messages/stats", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["overview"]["totalMessages"], json!(2));
    assert_eq!(body["data"]["overview"]["todayMessages"], json!(0));
    assert_eq!(body["data"]["overview"]["recalledMessages"], json!(0));
    assert!(body["data"]["overview"]["averagePerDay"].as_f64().unwrap() > 0.0);
    assert!(body["data"]["breakdown"].is_object());
}

#[tokio::test]
async fn tag_listing_aggregates_metadata_tags() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let m1 = seed_agent_message(&app, &conv, &admin_id, "a", false, None).await;
    let m2 = seed_agent_message(&app, &conv, &admin_id, "b", false, None).await;
    let m3 = seed_agent_message(&app, &conv, &admin_id, "c", true, None).await; // recalled: excluded
    set_message_metadata(&app, &m1, json!({ "tags": ["vip", "billing"] })).await;
    set_message_metadata(&app, &m2, json!({ "tags": ["vip"] })).await;
    set_message_metadata(&app, &m3, json!({ "tags": ["hidden"] })).await;

    let (status, body, _) = app
        .request("GET", "/api/messages/tags", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let tags = body["data"]["tags"].as_array().unwrap();
    assert_eq!(body["data"]["total"], json!(2));
    assert_eq!(tags[0]["name"], json!("vip"));
    assert_eq!(tags[0]["count"], json!(2));
    assert_eq!(tags[1]["name"], json!("billing"));
}

// ---------------------------------------------------------------- export family

#[tokio::test]
async fn export_filter_options_list_customers_and_agents() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    app.seed_customer("line", "U1", "Alice", None).await;
    let (status, body, _) = app
        .request("GET", "/api/messages/export/customers", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["displayName"], json!("Alice"));
    assert_eq!(body["data"][0]["platform"], json!("line"));

    let (status, body, _) = app
        .request("GET", "/api/messages/export/agents", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["displayName"], json!("admin user"));
    assert_eq!(body["data"][0]["role"], json!("admin"));
}

#[tokio::test]
async fn export_count_excludes_recalled() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    seed_agent_message(&app, &conv, &admin_id, "kept", false, None).await;
    seed_agent_message(&app, &conv, &admin_id, "recalled", true, None).await;
    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/messages/export/count?conversationId={conv}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["count"], json!(1));
    assert_eq!(body["data"]["willBeTruncated"], json!(false));
    assert!(body["data"]["limit"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn export_json_csv_txt_and_invalid_format() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    seed_agent_message(&app, &conv, &admin_id, "export me", false, None).await;
    seed_agent_message(&app, &conv, &admin_id, "skip me (recalled)", true, None).await;

    // JSON envelope with exportInfo.
    let (status, body, _) = app
        .request(
            "GET",
            "/api/messages/export?format=json",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["exportInfo"]["format"], json!("json"));
    assert_eq!(body["data"]["exportInfo"]["totalRecords"], json!(1));
    assert_eq!(body["data"]["exportInfo"]["exportedBy"], json!(admin_id));

    // CSV download.
    let (status, headers, text) = raw_get(&app, "/api/messages/export?format=csv", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .contains("text/csv"));
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .contains("attachment"));
    assert!(text.starts_with("id,conversationId,senderType"));
    assert!(text.contains("export me"));
    assert!(!text.contains("skip me"));

    // TXT transcript grouped by conversation.
    let (status, headers, text) = raw_get(&app, "/api/messages/export?format=txt", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .contains("text/plain"));
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .contains("attachment"));
    assert!(text.contains(&format!("Conversation: {conv}")));
    assert!(text.contains("export me"));

    // Invalid format.
    let (status, _, _) = app
        .request("GET", "/api/messages/export?format=xml", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------- bulk create

#[tokio::test]
async fn bulk_create_inserts_valid_and_reports_item_errors() {
    let app = spawn_app().await;
    let (token, _, _, conv) = fixture(&app).await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/messages/bulk-create",
            Some(&token),
            Some(json!({ "messages": [
                { "conversationId": conv, "content": "one" },
                { "conversationId": "missing", "content": "two" },
                { "conversationId": conv, "content": "  " },
            ]})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["totalRequested"], json!(3));
    assert_eq!(body["data"]["successCount"], json!(1));
    assert_eq!(body["data"]["failureCount"], json!(2));
    assert_eq!(body["data"]["results"][0]["index"], json!(0));
    assert_eq!(body["data"]["results"][0]["status"], json!("created"));
    assert_eq!(body["data"]["errors"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn bulk_create_validates_batch_shape() {
    let app = spawn_app().await;
    let (token, _, _, conv) = fixture(&app).await;
    // Empty array.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages/bulk-create",
            Some(&token),
            Some(json!({ "messages": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Over 100 entries.
    let entries: Vec<Value> = (0..101)
        .map(|i| json!({ "conversationId": conv, "content": format!("m{i}") }))
        .collect();
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages/bulk-create",
            Some(&token),
            Some(json!({ "messages": entries })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Malformed JSON.
    let (status, _) = app
        .request_raw("POST", "/api/messages/bulk-create", Some(&token), "nope")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------- bulk recall

#[tokio::test]
async fn bulk_delete_recalls_eligible_and_reports_item_errors() {
    let app = spawn_app().await;
    let (admin_token, admin_id) = admin(&app).await;
    let other_id = app
        .seed_agent("other@test.dev", "Secret123!", "agent")
        .await;
    let other_token = app.login("other@test.dev", "Secret123!").await.0;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    let mine = seed_agent_message(&app, &conv, &other_id, "mine", false, None).await;
    let admins = seed_agent_message(&app, &conv, &admin_id, "admin's", false, None).await;
    let recalled = seed_agent_message(&app, &conv, &other_id, "done", true, None).await;
    let expired = seed_agent_message(
        &app,
        &conv,
        &other_id,
        "late",
        false,
        Some("2000-01-01T00:00:00.000Z"),
    )
    .await;

    // Non-admin agent: own message recalls; the admin's is denied; recalled and
    // expired are per-item errors; unknown id is a per-item error.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/messages/bulk-delete",
            Some(&other_token),
            Some(json!({ "messageIds": [mine, admins, recalled, expired, "missing"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["totalRequested"], json!(5));
    assert_eq!(body["data"]["successCount"], json!(1));
    assert_eq!(body["data"]["failureCount"], json!(4));
    assert_eq!(body["data"]["results"][0]["status"], json!("recalled"));
    let errors = body["data"]["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 4);

    // The recalled one has the placeholder content.
    let content: String = sqlx::query_scalar("SELECT content FROM messages WHERE id = $1")
        .bind(body["data"]["results"][0]["messageId"].as_str().unwrap())
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(content, "[Message recalled]");
    let _ = admin_token;
}

#[tokio::test]
async fn bulk_delete_validates_batch_shape() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages/bulk-delete",
            Some(&token),
            Some(json!({ "messageIds": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let ids: Vec<String> = (0..101).map(|i| format!("m{i}")).collect();
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages/bulk-delete",
            Some(&token),
            Some(json!({ "messageIds": ids })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------- attachments

#[tokio::test]
async fn attachment_listing_returns_records_and_count() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "with file", false, None).await;
    sqlx::query(
        "INSERT INTO attachments (id, message_id, conversation_id, file_name, content_type,
                                  file_size, file_url, storage_key, created_at)
         VALUES ('att-1', $1, $2, 'doc.pdf', 'application/pdf', 42, '/uploads/doc.pdf', 'doc.pdf', $3)",
    )
    .bind(&id)
    .bind(&conv)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/messages/{id}/attachments"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["count"], json!(1));
    let a = &body["data"]["attachments"][0];
    assert_eq!(a["filename"], json!("doc.pdf"));
    assert_eq!(a["mimeType"], json!("application/pdf"));
    assert_eq!(a["fileSize"], json!(42));
    assert_eq!(a["storageKey"], json!("doc.pdf"));

    // Missing message -> 404.
    let (status, _, _) = app
        .request(
            "GET",
            "/api/messages/missing/attachments",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn attachment_upload_stores_file_and_validates() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "target", false, None).await;

    // Success.
    let req = multipart_request(
        &format!("/api/messages/{id}/attachments"),
        &token,
        "pic.png",
        "image/png",
        b"PNGDATA",
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["filename"], json!("pic.png"));
    assert_eq!(body["data"]["mimeType"], json!("image/png"));
    assert_eq!(body["data"]["fileSize"], json!(7));
    // The stored object exists on disk.
    let key: String =
        sqlx::query_scalar("SELECT storage_key FROM attachments WHERE message_id = $1")
            .bind(&id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(std::path::Path::new(&app.state.config.upload_dir)
        .join(&key)
        .exists());

    // Disallowed type.
    let req = multipart_request(
        &format!("/api/messages/{id}/attachments"),
        &token,
        "app.exe",
        "application/x-msdownload",
        b"MZ",
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Oversize (>10MB).
    let req = multipart_request(
        &format!("/api/messages/{id}/attachments"),
        &token,
        "big.png",
        "image/png",
        &vec![0u8; 10 * 1024 * 1024 + 1],
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Missing file field.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/messages/{id}/attachments"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "multipart/form-data; boundary=EMPTYBOUND")
        .body(Body::from("--EMPTYBOUND--\r\n"))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Unknown message -> 404.
    let req = multipart_request(
        "/api/messages/missing/attachments",
        &token,
        "pic.png",
        "image/png",
        b"PNGDATA",
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Non-author, non-admin on an agent-origin message -> 403.
    let (other_token, _) = agent(&app, "other@test.dev", None).await;
    let req = multipart_request(
        &format!("/api/messages/{id}/attachments"),
        &other_token,
        "pic.png",
        "image/png",
        b"PNGDATA",
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------- forward

#[tokio::test]
async fn forward_creates_copies_with_provenance() {
    let app = spawn_app().await;
    let (token, admin_id, cust, conv) = fixture(&app).await;
    let target_a = app.seed_conversation(cust, None, "active").await;
    let target_b = app.seed_conversation(cust, None, "active").await;
    let id = seed_agent_message(&app, &conv, &admin_id, "pass it on", false, None).await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/messages/{id}/forward"),
            Some(&token),
            Some(json!({
                "targetConversationIds": [target_a, target_b, "missing"],
                "comment": "FYI",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["originalMessageId"], json!(id));
    assert_eq!(body["data"]["totalTargets"], json!(3));
    assert_eq!(body["data"]["successCount"], json!(2));
    assert_eq!(body["data"]["failureCount"], json!(1));
    assert_eq!(
        body["data"]["errors"][0]["conversationId"],
        json!("missing")
    );

    // Forwarded copy carries the marker, the comment, and provenance metadata;
    // the target conversation's timestamps were bumped.
    let new_id = body["data"]["results"][0]["messageId"].as_str().unwrap();
    let (content, metadata, status_col): (String, String, String) =
        sqlx::query_as("SELECT content, metadata, delivery_status FROM messages WHERE id = $1")
            .bind(new_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(content.starts_with("[Forwarded] pass it on"));
    assert!(content.contains("FYI"));
    assert_eq!(status_col, "sent");
    let meta: Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(meta["forwardedFrom"]["messageId"], json!(id));
    assert_eq!(meta["forwardedBy"], json!(admin_id));
    let bumped: Option<String> =
        sqlx::query_scalar("SELECT last_message_at FROM conversations WHERE id = $1")
            .bind(&target_a)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(bumped.is_some());

    // Audit entry.
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action = 'message forward' AND resource_id = $1",
    )
    .bind(&id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(logged, 1);
}

#[tokio::test]
async fn forward_error_conditions() {
    let app = spawn_app().await;
    let (token, admin_id, cust, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "x", false, None).await;
    // Empty target list.
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/messages/{id}/forward"),
            Some(&token),
            Some(json!({ "targetConversationIds": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Over 20 targets.
    let many: Vec<String> = (0..21).map(|i| format!("c{i}")).collect();
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/messages/{id}/forward"),
            Some(&token),
            Some(json!({ "targetConversationIds": many })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Original message missing.
    let target = app.seed_conversation(cust, None, "active").await;
    let (status, _, _) = app
        .request(
            "POST",
            "/api/messages/missing/forward",
            Some(&token),
            Some(json!({ "targetConversationIds": [target] })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- per-message tags

#[tokio::test]
async fn set_tags_replaces_and_records_audit_stamps() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "x", false, None).await;
    set_message_metadata(&app, &id, json!({ "tags": ["old"], "keep": true })).await;

    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{id}/tags"),
            Some(&token),
            Some(json!({ "tags": [" vip ", "billing"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["tags"], json!(["vip", "billing"]));
    assert_eq!(body["data"]["previousTags"], json!(["old"]));
    assert_eq!(body["data"]["updatedBy"], json!(admin_id));

    // Metadata merged, not replaced wholesale.
    let metadata: String = sqlx::query_scalar("SELECT metadata FROM messages WHERE id = $1")
        .bind(&id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let meta: Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(meta["tags"], json!(["vip", "billing"]));
    assert_eq!(meta["keep"], json!(true));
    assert_eq!(meta["tagsUpdatedBy"], json!(admin_id));
}

#[tokio::test]
async fn set_tags_validation_and_not_found() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "x", false, None).await;
    // Not an array.
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{id}/tags"),
            Some(&token),
            Some(json!({ "tags": "vip" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Over 10 tags.
    let many: Vec<String> = (0..11).map(|i| format!("t{i}")).collect();
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{id}/tags"),
            Some(&token),
            Some(json!({ "tags": many })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Blank entry.
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{id}/tags"),
            Some(&token),
            Some(json!({ "tags": ["ok", " "] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Non-string entry.
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/messages/{id}/tags"),
            Some(&token),
            Some(json!({ "tags": [1] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Unknown message.
    let (status, _, _) = app
        .request(
            "PUT",
            "/api/messages/missing/tags",
            Some(&token),
            Some(json!({ "tags": ["x"] })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn remove_tags_clears_collection() {
    let app = spawn_app().await;
    let (token, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "x", false, None).await;
    set_message_metadata(&app, &id, json!({ "tags": ["vip"] })).await;

    let (status, body, _) = app
        .request(
            "DELETE",
            &format!("/api/messages/{id}/tags"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["removedTags"], json!(["vip"]));
    assert!(body["data"]["removedAt"].is_string());

    let metadata: String = sqlx::query_scalar("SELECT metadata FROM messages WHERE id = $1")
        .bind(&id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let meta: Value = serde_json::from_str(&metadata).unwrap();
    assert!(meta.get("tags").is_none());
    assert!(meta["tagsRemovedAt"].is_string());

    // Unknown message -> 404.
    let (status, _, _) = app
        .request("DELETE", "/api/messages/missing/tags", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- delayed-send service

#[tokio::test]
async fn schedule_delayed_validates_and_marks_recallable() {
    let app = spawn_app().await;
    let (_, admin_id, _, conv) = fixture(&app).await;

    // Delay out of range.
    for delay in [0, 121] {
        let result = service::schedule_delayed(
            &app.state,
            &admin_id,
            service::ScheduleParams {
                conversation_id: conv.clone(),
                content: "later".into(),
                delay_seconds: delay,
                message_type: None,
                recipient_id: None,
                platform: None,
                media_url: None,
                metadata: None,
            },
        )
        .await;
        assert_eq!(result["success"], json!(false), "{result}");
        assert!(result["error"].as_str().unwrap().contains("between"));
    }

    // Missing conversation.
    let result = service::schedule_delayed(
        &app.state,
        &admin_id,
        service::ScheduleParams {
            conversation_id: "missing".into(),
            content: "later".into(),
            delay_seconds: 5,
            message_type: None,
            recipient_id: None,
            platform: None,
            media_url: None,
            metadata: None,
        },
    )
    .await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(result["error"], json!("Conversation not found"));

    // Success: pending row + recallable marker; scheduled time doubles as the
    // recall deadline.
    let result = service::schedule_delayed(
        &app.state,
        &admin_id,
        service::ScheduleParams {
            conversation_id: conv.clone(),
            content: "later".into(),
            delay_seconds: 60,
            message_type: None,
            recipient_id: Some("U1".into()),
            platform: Some("webchat".into()),
            media_url: None,
            metadata: None,
        },
    )
    .await;
    assert_eq!(result["success"], json!(true), "{result}");
    let id = result["delayedMessageId"].as_str().unwrap();
    assert_eq!(result["scheduledSendTime"], result["recallDeadline"]);
    assert!(app.state.recallable_messages.is_recallable(id));
    let status: String = sqlx::query_scalar("SELECT status FROM scheduled_messages WHERE id = $1")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(status, "pending");
}

async fn schedule_for(app: &TestApp, agent_id: &str, conv: &str, platform: &str) -> String {
    let result = service::schedule_delayed(
        &app.state,
        agent_id,
        service::ScheduleParams {
            conversation_id: conv.to_string(),
            content: "delayed hello".into(),
            delay_seconds: 60,
            message_type: None,
            recipient_id: Some("U1".into()),
            platform: Some(platform.into()),
            media_url: None,
            metadata: None,
        },
    )
    .await;
    assert_eq!(result["success"], json!(true), "{result}");
    result["delayedMessageId"].as_str().unwrap().to_string()
}

async fn force_due(app: &TestApp, id: &str) {
    sqlx::query("UPDATE scheduled_messages SET scheduled_at = $1 WHERE id = $2")
        .bind("2000-01-01T00:00:00.000Z")
        .bind(id)
        .execute(&app.state.db)
        .await
        .unwrap();
}

#[tokio::test]
async fn process_delayed_dispatches_per_platform() {
    let line_push_url = mock_line_push_url().await;
    let app = spawn_app_custom(move |config| {
        config.line_channel_access_token = Some("good-line".into());
        config.line_push_url = line_push_url;
    })
    .await;
    let (_, admin_id, _, conv) = fixture(&app).await;

    // Too early -> reschedule signal, still pending.
    let early = schedule_for(&app, &admin_id, &conv, "webchat").await;
    let result = service::process_delayed(&app.state, &early).await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(result["reschedule"], json!(true));

    // Webchat: creates the persisted message (sent, 30-min recall window) and
    // marks the item sent.
    force_due(&app, &early).await;
    let result = service::process_delayed(&app.state, &early).await;
    assert_eq!(result["success"], json!(true), "{result}");
    let message_id = result["messageId"].as_str().unwrap();
    let (status_col, is_sent, deadline): (String, i64, Option<String>) = sqlx::query_as(
        "SELECT delivery_status, is_sent, recall_deadline FROM messages WHERE id = $1",
    )
    .bind(message_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(status_col, "sent");
    assert_eq!(is_sent, 1);
    assert!(deadline.is_some());
    let item_status: String =
        sqlx::query_scalar("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(&early)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(item_status, "sent");
    assert!(!app.state.recallable_messages.is_recallable(&early));

    // Already-processed item is not reprocessed.
    let result = service::process_delayed(&app.state, &early).await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(result["status"], json!("sent"));

    // LINE dispatches through the configured gateway endpoint and is marked sent.
    let line = schedule_for(&app, &admin_id, &conv, "line").await;
    force_due(&app, &line).await;
    let result = service::process_delayed(&app.state, &line).await;
    assert_eq!(result["success"], json!(true), "{result}");

    // Unsupported platform -> failed with a reason.
    let weird = schedule_for(&app, &admin_id, &conv, "telegram").await;
    force_due(&app, &weird).await;
    let result = service::process_delayed(&app.state, &weird).await;
    assert_eq!(result["success"], json!(false));
    let (item_status, metadata): (String, String) =
        sqlx::query_as("SELECT status, metadata FROM scheduled_messages WHERE id = $1")
            .bind(&weird)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(item_status, "failed");
    assert!(metadata.contains("failureReason"));

    // Unknown item.
    let result = service::process_delayed(&app.state, "missing").await;
    assert_eq!(result["success"], json!(false));
}

#[tokio::test]
async fn dispatch_due_processes_only_due_items() {
    let app = spawn_app().await;
    let (_, admin_id, _, conv) = fixture(&app).await;
    let due = schedule_for(&app, &admin_id, &conv, "webchat").await;
    let future = schedule_for(&app, &admin_id, &conv, "webchat").await;
    force_due(&app, &due).await;

    let processed = service::dispatch_due(&app.state).await;
    assert_eq!(processed, 1);
    let due_status: String =
        sqlx::query_scalar("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(&due)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(due_status, "sent");
    let future_status: String =
        sqlx::query_scalar("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(&future)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(future_status, "pending");
}

#[tokio::test]
async fn cancel_delayed_transitions_and_guards() {
    let app = spawn_app().await;
    let (_, admin_id, _, conv) = fixture(&app).await;

    // Successful recall before the scheduled time, with a recall-log entry.
    let id = schedule_for(&app, &admin_id, &conv, "webchat").await;
    let result =
        service::cancel_delayed(&app.state, &id, &admin_id, Some("changed my mind"), true).await;
    assert_eq!(result["success"], json!(true), "{result}");
    let (status_col, metadata): (String, String) =
        sqlx::query_as("SELECT status, metadata FROM scheduled_messages WHERE id = $1")
            .bind(&id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(status_col, "cancelled");
    assert!(metadata.contains("changed my mind"));
    assert!(!app.state.recallable_messages.is_recallable(&id));
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_recall_logs WHERE message_id = $1 AND action = 'successful'",
    )
    .bind(&id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(logged, 1);

    // Already cancelled -> not pending.
    let result = service::cancel_delayed(&app.state, &id, &admin_id, None, false).await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(result["status"], json!("cancelled"));

    // Past the scheduled time -> cannot recall.
    let late = schedule_for(&app, &admin_id, &conv, "webchat").await;
    force_due(&app, &late).await;
    let result = service::cancel_delayed(&app.state, &late, &admin_id, None, true).await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(
        result["error"],
        json!("Cannot recall after scheduled send time")
    );

    // Unknown item.
    let result = service::cancel_delayed(&app.state, "missing", &admin_id, None, false).await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(result["error"], json!("Delayed message not found"));
}

#[tokio::test]
async fn recall_sent_message_service_capability() {
    let app = spawn_app().await;
    let (_, admin_id, _, conv) = fixture(&app).await;
    let id = seed_agent_message(&app, &conv, &admin_id, "sent already", false, None).await;

    let result = service::recall_sent_message(&app.state, &id, &admin_id).await;
    assert_eq!(result["success"], json!(true), "{result}");
    assert_eq!(result["canRecall"], json!(true));
    assert_eq!(result["messageId"], json!(id));
    let (content, is_recalled, status_col): (String, i64, String) =
        sqlx::query_as("SELECT content, is_recalled, delivery_status FROM messages WHERE id = $1")
            .bind(&id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(content, "[Message recalled]");
    assert_eq!(is_recalled, 1);
    assert_eq!(status_col, "recalled");
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_recall_logs WHERE message_id = $1 AND action = 'successful'",
    )
    .bind(&id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(logged, 1);

    // Distinct error reasons: already recalled vs deadline exceeded.
    let result = service::recall_sent_message(&app.state, &id, &admin_id).await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(result["canRecall"], json!(false));
    assert_eq!(result["error"], json!("Message already recalled"));

    let expired = seed_agent_message(
        &app,
        &conv,
        &admin_id,
        "too late",
        false,
        Some("2000-01-01T00:00:00.000Z"),
    )
    .await;
    let result = service::recall_sent_message(&app.state, &expired, &admin_id).await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(result["error"], json!("Recall deadline exceeded"));

    let result = service::recall_sent_message(&app.state, "missing", &admin_id).await;
    assert_eq!(result["success"], json!(false));
    assert_eq!(result["error"], json!("Message not found"));
}

#[tokio::test]
async fn archive_stale_failed_retires_old_items() {
    let app = spawn_app().await;
    let (_, admin_id, _, conv) = fixture(&app).await;
    let fresh = schedule_for(&app, &admin_id, &conv, "telegram").await;
    let stale = schedule_for(&app, &admin_id, &conv, "telegram").await;
    for id in [&fresh, &stale] {
        force_due(&app, id).await;
        let _ = service::process_delayed(&app.state, id).await; // -> failed
    }
    // Make one failure look a day old.
    sqlx::query("UPDATE scheduled_messages SET updated_at = $1 WHERE id = $2")
        .bind("2000-01-01T00:00:00.000Z")
        .bind(&stale)
        .execute(&app.state.db)
        .await
        .unwrap();

    let archived = service::archive_stale_failed(&app.state.db).await.unwrap();
    assert_eq!(archived, 1);
    let stale_status: String =
        sqlx::query_scalar("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(&stale)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(stale_status, "archived");
    let fresh_status: String =
        sqlx::query_scalar("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(&fresh)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(fresh_status, "failed");
}

// ---------------------------------------------------------------- offline buffer

#[tokio::test]
async fn offline_buffer_replay_delivery_and_stats() {
    let app = spawn_app().await;
    let db = &app.state.db;
    let payload = json!({ "content": "while you were away" });
    let b1 = service::buffer_message(db, "user-1", "conv-1", "msg-1", &payload)
        .await
        .unwrap();
    let b2 = service::buffer_message(db, "user-1", "conv-1", "msg-2", &payload)
        .await
        .unwrap();
    service::buffer_message(db, "user-2", "conv-1", "msg-3", &payload)
        .await
        .unwrap();

    // Replay undelivered for one recipient only.
    let entries = service::replay_buffered(db, "user-1", false).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].recipient_id, "user-1");

    // Idempotent delivery marking.
    let marked = service::mark_delivered(db, std::slice::from_ref(&b1))
        .await
        .unwrap();
    assert_eq!(marked, 1);
    let marked_again = service::mark_delivered(db, std::slice::from_ref(&b1))
        .await
        .unwrap();
    assert_eq!(marked_again, 0);

    // Undelivered-only replay now skips it; include_delivered brings it back.
    let undelivered = service::replay_buffered(db, "user-1", false).await.unwrap();
    assert_eq!(undelivered.len(), 1);
    let all = service::replay_buffered(db, "user-1", true).await.unwrap();
    assert_eq!(all.len(), 2);

    // Stats.
    let stats = service::buffer_stats(db, "user-1").await.unwrap();
    assert_eq!(stats["total"], json!(2));
    assert_eq!(stats["delivered"], json!(1));
    assert_eq!(stats["pending"], json!(1));
    assert_eq!(stats["expired"], json!(0));

    // Retries beyond the maximum drop the entry.
    for _ in 0..service::MAX_BUFFER_RETRIES {
        let (retried, dropped) = service::retry_undelivered(db, "user-1").await.unwrap();
        assert_eq!((retried, dropped), (1, 0));
    }
    let (_, dropped) = service::retry_undelivered(db, "user-1").await.unwrap();
    assert_eq!(dropped, 1);
    let gone = service::replay_buffered(db, "user-1", false).await.unwrap();
    assert!(gone.is_empty());

    // Expired entries are purged (b1 is still retained as delivered; expire it).
    sqlx::query(
        "UPDATE offline_message_buffer SET expires_at = '2000-01-01T00:00:00.000Z' WHERE id = $1",
    )
    .bind(&b1)
    .execute(db)
    .await
    .unwrap();
    let purged = service::purge_expired(db).await.unwrap();
    assert_eq!(purged, 1);
    let _ = b2;
}

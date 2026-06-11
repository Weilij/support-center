//! Customer-Facing Conversations behavior tests (CRD §2.3, lines 1042-1170).

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{spawn_app, TestApp};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use mcss_backend::domain::auth::tokens::{sign, Claims};

async fn admin(app: &TestApp) -> (String, String) {
    let id = app.seed_agent("admin@test.dev", "Secret123!", "admin").await;
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

/// A signed customer-owner session credential (role "customer", subject = the
/// customer's identifier).
fn customer_token(customer_id: i64) -> String {
    let mut claims = Claims::new(customer_id.to_string(), "customer", "access", 3600);
    claims.name = Some("Alice".into());
    sign(&claims, "test-secret").unwrap()
}

async fn seed_attachment(
    app: &TestApp,
    conversation_id: &str,
    message_id: Option<&str>,
    storage_key: &str,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO attachments (id, message_id, conversation_id, file_name, content_type,
                                  file_size, file_url, storage_key, created_at)
         VALUES (?, ?, ?, 'f.png', 'image/png', 5, ?, ?, ?)",
    )
    .bind(&id)
    .bind(message_id)
    .bind(conversation_id)
    .bind(format!("/uploads/{storage_key}"))
    .bind(storage_key)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();
    id
}

fn multipart_request(path: &str, session: &str, data: Option<&[u8]>) -> Request<Body> {
    let boundary = "XTESTBOUNDARYX";
    let mut body = Vec::new();
    if let Some(data) = data {
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
    Request::builder()
        .method("POST")
        .uri(path)
        .header("X-Session-Id", session)
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap()
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

// ------------------------------------------------------------------ message history

#[tokio::test]
async fn history_returns_enriched_messages_newest_first() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let older = app.seed_message(&conv, "customer", "hi", Some("2026-01-01T00:00:00.000Z")).await;
    let newer = app.seed_message(&conv, "agent", "hello", Some("2026-02-01T00:00:00.000Z")).await;

    // One attachment with a stored binary (gets a force-download link) and one
    // without (inline link only).
    let upload_dir = std::path::Path::new(&app.state.config.upload_dir);
    std::fs::create_dir_all(upload_dir).unwrap();
    std::fs::write(upload_dir.join("stored.png"), b"PNG").unwrap();
    seed_attachment(&app, &conv, Some(&newer), "stored.png").await;
    seed_attachment(&app, &conv, Some(&older), "missing.png").await;

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/customer-conversations/{conv}/messages"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true));
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], json!(newer));
    assert_eq!(messages[1]["id"], json!(older));
    assert_eq!(body["hasMore"], json!(false));

    // Unified sender identifier: customer messages resolve the customer-side
    // reference.
    assert_eq!(messages[1]["senderId"], json!(cust.to_string()));
    // Attachment links: stored binary gets the extra download link, the other
    // falls back to inline-only.
    let stored = &messages[0]["attachments"][0];
    assert_eq!(stored["url"], json!("/uploads/stored.png"));
    assert_eq!(stored["downloadUrl"], json!("/uploads/stored.png?download=1"));
    let unstored = &messages[1]["attachments"][0];
    assert!(unstored["downloadUrl"].is_null());
}

#[tokio::test]
async fn history_paginates_with_before_cursor() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let first = app.seed_message(&conv, "customer", "1", Some("2026-01-01T00:00:00.000Z")).await;
    let second = app.seed_message(&conv, "customer", "2", Some("2026-01-02T00:00:00.000Z")).await;
    let third = app.seed_message(&conv, "customer", "3", Some("2026-01-03T00:00:00.000Z")).await;

    // limit=1 returns the newest; hasMore signals the full-page condition.
    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/customer-conversations/{conv}/messages?limit=1"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body["messages"][0]["id"], json!(third));
    assert_eq!(body["hasMore"], json!(true));

    // before=<third> restricts to strictly older entries.
    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/customer-conversations/{conv}/messages?before={third}"),
            Some(&token),
            None,
        )
        .await;
    let ids: Vec<&str> =
        body["messages"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![second.as_str(), first.as_str()]);

    // An unresolvable cursor falls back to the most recent page.
    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/customer-conversations/{conv}/messages?before=unknown"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body["messages"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn history_accepts_all_credential_supply_methods() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let path = format!("/api/customer-conversations/{conv}/messages");

    // Authorization: Bearer.
    let (status, _, _) = app.request("GET", &path, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    // X-Session-Id header.
    let (status, _, _) = app
        .request_with_headers("GET", &path, None, None, &[("X-Session-Id", &token)])
        .await;
    assert_eq!(status, StatusCode::OK);
    // sessionId query parameter.
    let (status, _, _) =
        app.request("GET", &format!("{path}?sessionId={token}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn history_error_conditions() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let team = app.seed_team("Support").await;
    let (outsider_token, _) = agent(&app, "outsider@test.dev", None).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let assigned = app.seed_conversation(cust, Some(team), "assigned").await;
    let path = format!("/api/customer-conversations/{assigned}/messages");

    // Missing credential -> 401.
    let (status, body, _) = app.request("GET", &path, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], json!(false));
    assert!(body["error"].as_str().unwrap().contains("Authentication required"));
    // Invalid session token -> 401.
    let (status, body, _) = app.request("GET", &path, Some("garbage"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].as_str().unwrap().contains("Invalid or expired"));
    // Valid session but not admitted by the four-way rule -> 403.
    let (status, body, _) = app.request("GET", &path, Some(&outsider_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["success"], json!(false));
    // Conversation missing -> 404.
    let (status, _, _) = app
        .request("GET", "/api/customer-conversations/missing/messages", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Soft-deleted conversation is not a valid target -> 404.
    sqlx::query("UPDATE conversations SET deleted_at = ? WHERE id = ?")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(&assigned)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) = app.request("GET", &path, Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn four_way_access_rule_admits_each_leg() {
    let app = spawn_app().await;
    let (admin_token, _) = admin(&app).await;
    let team = app.seed_team("Support").await;
    let (member_token, _) = agent(&app, "member@test.dev", Some(team)).await;
    let (outsider_token, _) = agent(&app, "outsider@test.dev", None).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let other_cust = app.seed_customer("line", "U2", "Eve", None).await;
    let assigned = app.seed_conversation(cust, Some(team), "assigned").await;
    let pool = app.seed_conversation(cust, None, "active").await;
    let assigned_path = format!("/api/customer-conversations/{assigned}/messages");
    let pool_path = format!("/api/customer-conversations/{pool}/messages");

    // 1. Administrators are always admitted.
    let (status, _, _) = app.request("GET", &assigned_path, Some(&admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
    // 2. The conversation's owner (its customer) is admitted even when assigned.
    let (status, _, _) =
        app.request("GET", &assigned_path, Some(&customer_token(cust)), None).await;
    assert_eq!(status, StatusCode::OK);
    // ...but a different customer is not.
    let (status, _, _) =
        app.request("GET", &assigned_path, Some(&customer_token(other_cust)), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // 3. An unassigned conversation admits any valid session.
    let (status, _, _) = app.request("GET", &pool_path, Some(&outsider_token), None).await;
    assert_eq!(status, StatusCode::OK);
    // 4. Assigned conversations admit team members; non-members are rejected.
    let (status, _, _) = app.request("GET", &assigned_path, Some(&member_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app.request("GET", &assigned_path, Some(&outsider_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------------ send a reply

#[tokio::test]
async fn reply_persists_delivered_message_and_echoes_correlation_id() {
    let app = spawn_app().await;
    let (token, admin_id) = admin(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/customer-conversations/{conv}/messages"),
            Some(&token),
            Some(json!({
                "content": "thanks for reaching out",
                "platform": "line",
                "correlationId": "client-123",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true));
    let message = &body["message"];
    assert_eq!(message["content"], json!("thanks for reaching out"));
    assert_eq!(message["messageType"], json!("text"));
    assert_eq!(message["senderType"], json!("agent"));
    assert_eq!(message["senderId"], json!(admin_id));
    assert_eq!(message["correlationId"], json!("client-123"));

    // Recorded as sent + delivered, with the display-name snapshot, and the
    // conversation recency markers advanced (CRD 1084-1086, 1158).
    let row: (String, i64, String, String) = sqlx::query_as(
        "SELECT delivery_status, is_sent, sender_name, metadata FROM messages WHERE id = ?",
    )
    .bind(message["id"].as_str().unwrap())
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(row.0, "delivered");
    assert_eq!(row.1, 1);
    assert_eq!(row.2, "admin user");
    let metadata: Value = serde_json::from_str(&row.3).unwrap();
    assert_eq!(metadata["correlationId"], json!("client-123"));
    assert_eq!(metadata["platform"], json!("line"));
    let (last_msg, updated): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT last_message_at, updated_at FROM conversations WHERE id = ?")
            .bind(&conv)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(last_msg.is_some());
    assert!(updated.is_some());
}

#[tokio::test]
async fn reply_links_attachments_and_forces_file_kind() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let attachment = seed_attachment(&app, &conv, None, "upload.png").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/customer-conversations/{conv}/messages"),
            Some(&token),
            Some(json!({ "attachmentIds": [attachment], "messageType": "text" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Attachments force the file kind regardless of the supplied value.
    assert_eq!(body["message"]["messageType"], json!("file"));
    assert_eq!(body["message"]["attachments"].as_array().unwrap().len(), 1);

    let linked: Option<String> =
        sqlx::query_scalar("SELECT message_id FROM attachments WHERE id = ?")
            .bind(&attachment)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(linked.as_deref(), body["message"]["id"].as_str());
}

#[tokio::test]
async fn reply_requires_content_or_attachments() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    for body in [json!({}), json!({ "content": "  ", "attachmentIds": [] })] {
        let (status, resp, _) = app
            .request(
                "POST",
                &format!("/api/customer-conversations/{conv}/messages"),
                Some(&token),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp["success"], json!(false));
    }
}

#[tokio::test]
async fn reply_shares_the_gate_error_conditions() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let team = app.seed_team("Support").await;
    let (outsider_token, _) = agent(&app, "outsider@test.dev", None).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let assigned = app.seed_conversation(cust, Some(team), "assigned").await;
    let path = format!("/api/customer-conversations/{assigned}/messages");
    let payload = json!({ "content": "hi" });

    let (status, _, _) = app.request("POST", &path, None, Some(payload.clone())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) =
        app.request("POST", &path, Some(&outsider_token), Some(payload.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request(
            "POST",
            "/api/customer-conversations/missing/messages",
            Some(&token),
            Some(payload),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------ file upload

#[tokio::test]
async fn upload_stores_namespaced_file_and_creates_no_message() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await; // login created a live session record
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    let req = multipart_request(
        &format!("/api/customer-conversations/{conv}/upload"),
        &token,
        Some(b"PNGDATA"),
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["filename"], json!("photo.png"));
    assert_eq!(body["size"], json!(7));
    assert_eq!(body["contentType"], json!("image/png"));
    let url = body["url"].as_str().unwrap();
    // Conversation-namespaced unique key preserving the extension.
    assert!(url.starts_with(&format!("/uploads/conv_{conv}_")));
    assert!(url.ends_with(".png"));
    let key = url.strip_prefix("/uploads/").unwrap();
    assert!(std::path::Path::new(&app.state.config.upload_dir).join(key).exists());

    // No message was created by the upload (CRD 1110, 1112).
    let messages: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE conversation_id = ?")
            .bind(&conv)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(messages, 0);
}

#[tokio::test]
async fn upload_revalidates_against_the_live_session_store() {
    let app = spawn_app().await;
    let admin_id = app.seed_agent("admin@test.dev", "Secret123!", "admin").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    // A validly signed token whose holder has no live session record fails the
    // storage-layer re-check with 401 (CRD 1109, 1119).
    let mut claims = Claims::new(admin_id, "admin", "access", 3600);
    claims.name = Some("admin user".into());
    let token = sign(&claims, "test-secret").unwrap();
    let req = multipart_request(
        &format!("/api/customer-conversations/{conv}/upload"),
        &token,
        Some(b"PNGDATA"),
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(resp).await;
    assert!(body["error"].as_str().unwrap().contains("Session"));
}

#[tokio::test]
async fn upload_error_conditions() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    // No file part -> 400.
    let req = multipart_request(
        &format!("/api/customer-conversations/{conv}/upload"),
        &token,
        None,
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert!(body["error"].as_str().unwrap().contains("file"));

    // Missing credential -> 401.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/customer-conversations/{conv}/upload"))
        .header("Content-Type", "multipart/form-data; boundary=X")
        .body(Body::from("--X--\r\n"))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Conversation missing -> 404.
    let req = multipart_request(
        "/api/customer-conversations/missing/upload",
        &token,
        Some(b"PNGDATA"),
    );
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------ WebSocket channel

#[tokio::test]
async fn ws_requires_both_query_parameters() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    for path in [
        "/api/customer-ws".to_string(),
        "/api/customer-ws?conversationId=c1".to_string(),
        format!("/api/customer-ws?sessionId={token}"),
    ] {
        let (status, body, _) = app.request("GET", &path, None, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
        assert!(body["error"].as_str().unwrap().contains("Missing required parameters"));
    }
}

#[tokio::test]
async fn ws_applies_session_and_four_way_access_checks() {
    let app = spawn_app().await;
    let (admin_token, _) = admin(&app).await;
    let team = app.seed_team("Support").await;
    let (outsider_token, _) = agent(&app, "outsider@test.dev", None).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let assigned = app.seed_conversation(cust, Some(team), "assigned").await;

    // Invalid session -> 401.
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/customer-ws?conversationId={assigned}&sessionId=garbage"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Valid session, not admitted -> 403.
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/customer-ws?conversationId={assigned}&sessionId={outsider_token}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Conversation missing -> 404.
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/customer-ws?conversationId=missing&sessionId={admin_token}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The owner passes the access check; without upgrade headers the request
    // then fails with the documented expected-WebSocket error.
    let (status, body, _) = app
        .request(
            "GET",
            &format!(
                "/api/customer-ws?conversationId={assigned}&sessionId={}",
                customer_token(cust)
            ),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("WebSocket"));
}

#[tokio::test]
async fn ws_non_upgrade_request_with_valid_access_gets_documented_error() {
    let app = spawn_app().await;
    let (token, _) = admin(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/customer-ws?conversationId={conv}&sessionId={token}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
    assert!(body["error"].as_str().unwrap().contains("WebSocket"));
}

// NOTE: a positive HTTP 101 upgrade cannot be exercised through `oneshot`
// (hyper only attaches the connection-upgrade extension on a live connection),
// so the successful-upgrade path and in-channel behavior are covered by the
// Phase 4 realtime work; the access gates above are what this phase owns.

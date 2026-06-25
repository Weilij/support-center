//! Notifications, reminders & alerting per CRD §6.4 (lines 4881-5104).

mod common;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use common::{spawn_app, TestApp};
use mcss_backend::domain::notifications::{alerts, reminders};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn users(app: &TestApp) -> (String, String, String) {
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    let agent_id = app.seed_agent("n@test.dev", "pw123456", "agent").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (agent, _, _) = app.login("n@test.dev", "pw123456").await;
    (admin, agent, agent_id)
}

async fn webhook_sink() -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    async fn capture(State(seen): State<Arc<Mutex<Vec<Value>>>>, body: Bytes) -> Json<Value> {
        let parsed = serde_json::from_slice::<Value>(&body).unwrap_or_else(|_| json!({}));
        seen.lock().await.push(parsed);
        Json(json!({"ok": true}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/alert", post(capture))
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/alert"), seen)
}

// ---------------------------------------------------------------- inbox

#[tokio::test]
async fn health_and_info_are_public() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/notifications/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "healthy");
    let (status, body, _) = app.request("GET", "/api/notifications/info", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["types"].as_array().unwrap().len() >= 10);
}

#[tokio::test]
async fn create_validates_sanitizes_and_enforces_ownership() {
    let app = spawn_app().await;
    let (admin, agent, agent_id) = users(&app).await;

    // Validation failures.
    let (status, _, _) = app
        .request("POST", "/api/notifications", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _, _) = app
        .request("POST", "/api/notifications", Some(&agent),
            Some(json!({"type": "telepathy", "title": "t", "content": "c"})))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _, _) = app
        .request("POST", "/api/notifications", Some(&agent),
            Some(json!({"type": "system", "title": "t", "content": "c",
                        "expiresAt": "2000-01-01T00:00:00Z"})))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "expiry must be future");

    // Non-admins always target themselves: the supplied userId is ignored.
    let (status, body, _) = app
        .request("POST", "/api/notifications", Some(&agent),
            Some(json!({"type": "system", "title": "Mine", "content": "c",
                        "userId": "someone-else",
                        "data": {"note": "<script>alert(1)</script>safe"}})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["data"]["id"].as_str().unwrap().to_string();
    let (owner, data): (String, Option<String>) =
        sqlx::query_as("SELECT agent_id, data FROM notifications WHERE id = $1")
            .bind(&id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(owner, agent_id, "ownership enforced server-side");
    assert!(!data.unwrap().contains("<script>"), "markup stripped from data strings");

    // Admin may target another recipient.
    let (status, body, _) = app
        .request("POST", "/api/notifications", Some(&admin),
            Some(json!({"type": "system", "title": "For agent", "content": "c", "userId": agent_id})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let owner: String = sqlx::query_scalar("SELECT agent_id FROM notifications WHERE id = $1")
        .bind(body["data"]["id"].as_str().unwrap())
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(owner, agent_id);
}

#[tokio::test]
async fn list_filters_and_excludes_expired() {
    let app = spawn_app().await;
    let (_, agent, agent_id) = users(&app).await;
    for (title, kind, read) in [("a", "system", 0), ("b", "mention", 0), ("c", "system", 1)] {
        sqlx::query(
            "INSERT INTO notifications (id, agent_id, type, title, content, is_read, priority, created_at)
             VALUES ($1, $2, $3, $4, 'x', $5, 'normal', $6)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&agent_id)
        .bind(kind)
        .bind(title)
        .bind(read)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&app.state.db)
        .await
        .unwrap();
    }
    // An already-expired record never appears.
    sqlx::query(
        "INSERT INTO notifications (id, agent_id, type, title, content, expires_at, priority, created_at)
         VALUES ('expired-1', $1, 'system', 'old', 'x', '2000-01-01T00:00:00Z', 'normal', $2)",
    )
    .bind(&agent_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, body, _) = app.request("GET", "/api/notifications", Some(&agent), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 3, "expired excluded");

    let (_, body, _) = app
        .request("GET", "/api/notifications?type=mention", Some(&agent), None)
        .await;
    assert_eq!(body["data"]["total"], 1);
    let (_, body, _) = app
        .request("GET", "/api/notifications?isRead=false", Some(&agent), None)
        .await;
    assert_eq!(body["data"]["total"], 2);

    // Invalid filters produce structured validation errors.
    let (status, body, _) = app
        .request("GET", "/api/notifications?type=bogus&priority=mega", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["data"]["errors"].as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn read_delete_counts_and_recent() {
    let app = spawn_app().await;
    let (_, agent, _) = users(&app).await;
    let mut ids = Vec::new();
    for i in 0..3 {
        let (_, body, _) = app
            .request("POST", "/api/notifications", Some(&agent),
                Some(json!({"type": "system", "title": format!("n{i}"), "content": "c"})))
            .await;
        ids.push(body["data"]["id"].as_str().unwrap().to_string());
    }

    let (_, body, _) = app
        .request("GET", "/api/notifications/unread-count", Some(&agent), None)
        .await;
    assert_eq!(body["data"]["count"], 3);
    assert_eq!(body["data"]["type"], "all");

    let (status, _, _) = app
        .request("PUT", &format!("/api/notifications/{}/read", ids[0]), Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body, _) = app
        .request("GET", "/api/notifications/recent?limit=5", Some(&agent), None)
        .await;
    assert_eq!(body["data"]["count"], 2, "recent lists only unread");

    let (_, body, _) = app
        .request("PUT", "/api/notifications/mark-all-read", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(body["data"]["updated"], 2);

    // Ownership: another user cannot read or delete my record.
    app.seed_agent("intruder@test.dev", "pw123456", "agent").await;
    let (intruder, _, _) = app.login("intruder@test.dev", "pw123456").await;
    let (status, _, _) = app
        .request("GET", &format!("/api/notifications/{}", ids[1]), Some(&intruder), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = app
        .request("DELETE", &format!("/api/notifications/{}", ids[1]), Some(&intruder), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = app
        .request("DELETE", &format!("/api/notifications/{}", ids[1]), Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE id = $1")
        .bind(&ids[1])
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "hard delete");

    // Stats shape.
    let (_, body, _) = app.request("GET", "/api/notifications/stats", Some(&agent), None).await;
    assert_eq!(body["data"]["total"], 2);
    assert!(body["data"]["byType"]["system"]["total"].as_i64().unwrap() >= 2);
    assert!(body["data"]["timeRanges"]["today"].as_i64().unwrap() >= 2);
    assert!(body["data"]["channelStats"]["realtime"].is_object());
}

#[tokio::test]
async fn bulk_admin_broadcast_and_cleanup() {
    let app = spawn_app().await;
    let (admin, agent, agent_id) = users(&app).await;

    // Bulk is admin-only and validates per item.
    let (status, _, _) = app
        .request("POST", "/api/notifications/bulk", Some(&agent),
            Some(json!({"notifications": [{"type": "system", "title": "t", "content": "c"}]})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body, _) = app
        .request("POST", "/api/notifications/bulk", Some(&admin),
            Some(json!({"notifications": [
                {"type": "system", "title": "ok", "content": "c", "userId": agent_id},
                {"type": "system", "title": "ok2", "content": "c"}
            ]})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["successCount"], 2);

    // Targeted system + broadcast.
    let (status, _, _) = app
        .request("POST", "/api/notifications/system", Some(&agent),
            Some(json!({"title": "t", "content": "c"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body, _) = app
        .request("POST", "/api/notifications/broadcast", Some(&admin),
            Some(json!({"title": "公告", "content": "all hands"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["recipientCount"].as_i64().unwrap() >= 2, "all active staff");

    // Cleanup removes expired records system-wide (admin only).
    sqlx::query(
        "INSERT INTO notifications (id, agent_id, type, title, content, expires_at, priority, created_at)
         VALUES ('exp-1', $1, 'system', 'old', 'x', '2000-01-01T00:00:00Z', 'normal', $2)",
    )
    .bind(&agent_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();
    let (status, _, _) = app.request("DELETE", "/api/notifications/cleanup", Some(&agent), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, body, _) = app.request("DELETE", "/api/notifications/cleanup", Some(&admin), None).await;
    assert_eq!(body["data"]["deleted"], 1);

    // Channel registry + channel test.
    let (_, body, _) = app
        .request("GET", "/api/notifications/channels/stats", Some(&admin), None)
        .await;
    assert_eq!(body["data"]["channels"]["realtime"]["enabled"], true);
    let (status, body, _) = app
        .request("POST", "/api/notifications/channels/realtime/test", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["success"], true);
    let (_, body, _) = app
        .request("POST", "/api/notifications/channels/email/test", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(body["data"]["success"], false, "unconfigured channel fails gracefully");
    let (status, _, _) = app
        .request("POST", "/api/notifications/channels/telepathy/test", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn trigger_endpoints_create_typed_records() {
    let app = spawn_app().await;
    let (_, agent, agent_id) = users(&app).await;
    let (status, _, _) = app
        .request("POST", "/api/notifications/new-message", Some(&agent),
            Some(json!({"userId": agent_id, "conversationId": "c-1",
                        "senderName": "客戶A", "content": "x".repeat(300)})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (kind, content, priority): (String, String, String) = sqlx::query_as(
        "SELECT type, content, priority FROM notifications WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&agent_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(kind, "new_message");
    assert!(content.chars().count() <= 120, "preview truncated (~100 chars)");
    assert_eq!(priority, "normal");

    let (status, _, _) = app
        .request("POST", "/api/notifications/conversation-assigned", Some(&agent),
            Some(json!({"userId": agent_id, "conversationId": "c-1",
                        "customerName": "客戶B", "assignedBy": "主管"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (kind, priority): (String, String) = sqlx::query_as(
        "SELECT type, priority FROM notifications WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&agent_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(kind, "conversation_assigned");
    assert_eq!(priority, "high");
}

// ---------------------------------------------------------------- reminders

#[tokio::test]
async fn reminder_crud_and_validation() {
    let app = spawn_app().await;
    let (_, agent, _) = users(&app).await;

    let cases = [
        (json!({}), "missing both"),
        (json!({"title": "t", "remindAt": "soon"}), "bad date"),
        (json!({"title": "t", "remindAt": "2000-01-01T00:00:00Z"}), "past"),
    ];
    for (body, label) in cases {
        let (status, _, _) = app.request("POST", "/api/reminders", Some(&agent), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label}");
    }

    let future = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let (status, body, _) = app
        .request("POST", "/api/reminders", Some(&agent),
            Some(json!({"title": "跟進客戶", "remindAt": future, "content": "記得回電"})))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["data"]["id"].as_str().unwrap().to_string();

    let (_, body, _) = app.request("GET", "/api/reminders", Some(&agent), None).await;
    assert_eq!(body["data"]["count"], 1);

    // Update re-arms the sent flag when the time changes.
    sqlx::query("UPDATE task_reminders SET is_sent = 1 WHERE id = $1")
        .bind(&id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let new_time = (chrono::Utc::now() + chrono::Duration::hours(4)).to_rfc3339();
    let (status, _, _) = app
        .request("PUT", &format!("/api/reminders/{id}"), Some(&agent),
            Some(json!({"remindAt": new_time})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let sent: i64 = sqlx::query_scalar("SELECT is_sent FROM task_reminders WHERE id = $1")
        .bind(&id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(sent, 0, "editing the time re-arms the reminder");

    // Complete, then it disappears from the default listing.
    let (status, _, _) = app
        .request("PUT", &format!("/api/reminders/{id}/complete"), Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body, _) = app.request("GET", "/api/reminders", Some(&agent), None).await;
    assert_eq!(body["data"]["count"], 0);
    let (_, body, _) = app
        .request("GET", "/api/reminders?includeCompleted=true", Some(&agent), None)
        .await;
    assert_eq!(body["data"]["count"], 1);

    let (status, _, _) = app
        .request("DELETE", &format!("/api/reminders/{id}"), Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app
        .request("GET", &format!("/api/reminders/{id}"), Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn due_reminders_fire_notifications_and_repeat() {
    let app = spawn_app().await;
    let (admin, agent, agent_id) = users(&app).await;

    // A due repeating reminder (inserted directly so it is already past-due).
    sqlx::query(
        "INSERT INTO task_reminders (id, agent_id, title, content, remind_at, repeat_type, repeat_interval, created_at)
         VALUES ('rem-1', $1, '每日站會', '別忘了', $2, 'daily', 1, $3)",
    )
    .bind(&agent_id)
    .bind((chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339())
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    // Stats see it as overdue.
    let (_, body, _) = app.request("GET", "/api/reminders/stats", Some(&agent), None).await;
    assert_eq!(body["data"]["overdue"], 1);

    // The manual pass is admin-only.
    let (status, _, _) = app.request("POST", "/api/reminders/process", Some(&agent), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body, _) = app.request("POST", "/api/reminders/process", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["processed"], 1);

    // Fired: high-priority task_reminder notification + sent flag + next occurrence.
    let (kind, priority): (String, String) = sqlx::query_as(
        "SELECT type, priority FROM notifications WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&agent_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(kind, "task_reminder");
    assert_eq!(priority, "high");
    let sent: i64 = sqlx::query_scalar("SELECT is_sent FROM task_reminders WHERE id = 'rem-1'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(sent, 1);
    let spawned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_reminders WHERE agent_id = $1 AND is_sent = 0",
    )
    .bind(&agent_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(spawned, 1, "repeating reminder spawned the next occurrence");

    // Idempotent: a second pass processes nothing new from the fired one.
    let processed = reminders::process_due(&app.state).await;
    assert_eq!(processed, 0);
}

// ---------------------------------------------------------------- alerting

#[tokio::test]
async fn monitoring_alerts_rate_limit_ack_and_resolve() {
    let app = spawn_app().await;

    let alert = alerts::send_monitoring_alert(&app.state, "critical", "CPU 過載", "90%", None).await;
    assert_eq!(alert["level"], "critical");
    assert_eq!(alert["acknowledged"], false);
    let attempts = alert["channelAttempts"].as_array().unwrap();
    assert!(attempts.iter().any(|a| a["channel"] == "console" && a["success"] == true));
    assert!(attempts.iter().any(|a| a["channel"] == "webhook" && a["success"] == false),
        "unconfigured webhook fails gracefully");

    let id = alert["id"].as_str().unwrap();
    assert!(alerts::acknowledge(&app.state.db, id, "admin-1").await);
    assert!(alerts::resolve(&app.state.db, id).await);
    assert!(!alerts::acknowledge(&app.state.db, "ghost", "x").await);

    // Rate limit: cap 20/hour, emergency bypasses.
    for i in 0..25 {
        alerts::send_monitoring_alert(&app.state, "info", &format!("i{i}"), "d", None).await;
    }
    let limited = alerts::send_monitoring_alert(&app.state, "warning", "limited", "d", None).await;
    assert_eq!(limited["rateLimited"], true);
    assert_eq!(limited["channelAttempts"].as_array().unwrap().len(), 0);
    let emergency = alerts::send_monitoring_alert(&app.state, "emergency", "fire", "d", None).await;
    assert_eq!(emergency["rateLimited"], false, "emergency always bypasses the limit");

    // Config read/merge round trip.
    let config = alerts::get_config(&app.state.db).await;
    assert_eq!(config["rateLimiting"]["maxPerHour"], 20);
    let updated = alerts::update_config(&app.state.db, &json!({"enabled": false})).await;
    assert_eq!(updated["enabled"], false);
    let disabled = alerts::send_monitoring_alert(&app.state, "info", "quiet", "d", None).await;
    assert_eq!(disabled["channelAttempts"].as_array().unwrap().len(), 0,
        "globally disabled: recorded but not delivered");
}

#[tokio::test]
async fn monitoring_alert_posts_configured_webhook_channel() {
    let app = spawn_app().await;
    let (url, seen) = webhook_sink().await;
    sqlx::query(
        "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind("alert.webhook")
    .bind(json!({"url": url}).to_string())
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let alert = alerts::send_monitoring_alert(
        &app.state,
        "critical",
        "DB latency",
        "p95 crossed threshold",
        Some(json!({"metric": "db.p95"})),
    )
    .await;
    let attempts = alert["channelAttempts"].as_array().unwrap();
    assert!(attempts.iter().any(|a| a["channel"] == "webhook" && a["success"] == true));

    let received = seen.lock().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["type"], "monitoring_alert");
    assert_eq!(received[0]["level"], "critical");
    assert_eq!(received[0]["title"], "DB latency");
    assert_eq!(received[0]["metadata"]["metric"], "db.p95");
}

#[tokio::test]
async fn monitoring_alert_posts_slack_specific_payload_for_chat_channel() {
    let app = spawn_app().await;
    let (url, seen) = webhook_sink().await;
    sqlx::query(
        "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind("alert.slack")
    .bind(json!({"webhookUrl": url}).to_string())
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let alert = alerts::send_monitoring_alert(
        &app.state,
        "emergency",
        "Queue down",
        "worker heartbeat missing",
        None,
    )
    .await;
    let attempts = alert["channelAttempts"].as_array().unwrap();
    assert!(attempts.iter().any(|a| a["channel"] == "chat" && a["success"] == true));

    let received = seen.lock().await;
    assert_eq!(received.len(), 1);
    assert!(received[0]["text"].as_str().unwrap().contains("Queue down"));
    assert!(
        received[0].get("type").is_none(),
        "Slack payload should use Incoming Webhook text format, not generic alert JSON"
    );
}

#[tokio::test]
async fn security_alerts_gate_by_severity_and_configuration() {
    // No destinations configured: selected destinations fail gracefully.
    let (ok, failed, errors) =
        alerts::send_security_alert("攻擊偵測", "signature mismatch burst", "critical", None).await;
    assert_eq!(ok, 0);
    assert!(failed >= 1);
    assert!(errors.iter().all(|e| e.contains("not configured")));

    // A low-severity alert selects fewer destinations than critical.
    let (_, failed_low, _) = alerts::send_security_alert("t", "m", "low", None).await;
    assert!(failed_low <= failed);
}

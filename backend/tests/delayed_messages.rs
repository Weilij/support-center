//! Delayed / Scheduled Messages per CRD §2.4 (lines 1171-1332).

mod common;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use serde_json::json;

/// Agent with a team, plus a conversation in that team. Returns
/// (token, agent_id, conversation_id).
async fn setup(app: &TestApp) -> (String, String, String) {
    let team = app.seed_team("DM").await;
    let agent = app.seed_agent("dm@test.dev", "pw123456", "agent").await;
    app.add_membership(&agent, team, "member", true).await;
    let customer = app.seed_customer("line", "U-dm", "DM Customer", Some(team)).await;
    let conversation = app.seed_conversation(customer, Some(team), "active").await;
    let (token, _, _) = app.login("dm@test.dev", "pw123456").await;
    (token, agent, conversation)
}

fn v2_send_body(conversation: &str, delay: i64) -> serde_json::Value {
    json!({
        "conversationId": conversation,
        "content": "scheduled hello",
        "platform": "line",
        "userId": "U-dm",
        "delaySeconds": delay,
    })
}

// ================================================================ v2 family

#[tokio::test]
async fn v2_health_is_public_with_feature_flags() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/delayed-messages-v2/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "healthy");
    assert_eq!(body["data"]["features"]["instantCancel"], true);
    assert_eq!(body["data"]["features"]["preciseScheduling"], true);
    assert_eq!(body["data"]["features"]["durablePersistence"], true);
}

#[tokio::test]
async fn v2_send_validates_and_schedules() {
    let app = spawn_app().await;
    let (token, _, conversation) = setup(&app).await;

    // Missing required fields.
    let (status, _, _) = app
        .request("POST", "/api/delayed-messages-v2/send", Some(&token),
            Some(json!({"conversationId": conversation})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Delay out of range.
    let (status, _, _) = app
        .request("POST", "/api/delayed-messages-v2/send", Some(&token),
            Some(v2_send_body(&conversation, 121)))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/delayed-messages-v2/send", Some(&token),
            Some(v2_send_body(&conversation, 0)))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Success: identifiers, fire time, canCancelUntil == fire time.
    let (status, body, _) = app
        .request("POST", "/api/delayed-messages-v2/send", Some(&token),
            Some(v2_send_body(&conversation, 60)))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert!(data["messageId"].is_string());
    assert!(data["scheduledSendTime"].as_i64().unwrap() > chrono::Utc::now().timestamp_millis());
    assert_eq!(data["scheduledSendTime"], data["canCancelUntil"]);
    assert_eq!(data["delaySeconds"], 60);
    assert_eq!(data["conversationId"], conversation.as_str());

    // Activity log written best-effort.
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action = 'delayed_message_scheduled'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn v2_send_is_permission_gated() {
    let app = spawn_app().await;
    let (_, _, conversation) = setup(&app).await;
    // An agent from another team (no access to this conversation's team).
    let outsider = app.seed_agent("out@test.dev", "pw123456", "agent").await;
    let other_team = app.seed_team("Other").await;
    app.add_membership(&outsider, other_team, "member", true).await;
    let (token, _, _) = app.login("out@test.dev", "pw123456").await;
    let (status, _, _) = app
        .request("POST", "/api/delayed-messages-v2/send", Some(&token),
            Some(v2_send_body(&conversation, 10)))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn v2_cancel_status_pending_round_trip() {
    let app = spawn_app().await;
    let (token, _, conversation) = setup(&app).await;
    let (_, body, _) = app
        .request("POST", "/api/delayed-messages-v2/send", Some(&token),
            Some(v2_send_body(&conversation, 90)))
        .await;
    let message_id = body["data"]["messageId"].as_str().unwrap().to_string();

    // Status while pending: countdown + cancellable.
    let (status, sbody, _) = app
        .request("GET",
            &format!("/api/delayed-messages-v2/status/{message_id}?conversationId={conversation}"),
            Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sbody["data"]["exists"], true);
    assert_eq!(sbody["data"]["status"], "pending");
    assert!(sbody["data"]["remainingSeconds"].as_i64().unwrap() > 0);
    assert_eq!(sbody["data"]["canCancel"], true);

    // Pending listing includes a preview + remaining ms.
    let (_, pbody, _) = app
        .request("GET", &format!("/api/delayed-messages-v2/pending?conversationId={conversation}"),
            Some(&token), None)
        .await;
    assert_eq!(pbody["data"]["count"], 1);
    assert_eq!(pbody["data"]["messages"][0]["messageId"], message_id.as_str());
    assert!(pbody["data"]["messages"][0]["remainingMs"].as_i64().unwrap() > 0);

    // Cancel requires the conversation id.
    let (status, _, _) = app
        .request("DELETE", &format!("/api/delayed-messages-v2/cancel/{message_id}"),
            Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, cbody, _) = app
        .request("DELETE", &format!("/api/delayed-messages-v2/cancel/{message_id}"),
            Some(&token), Some(json!({"conversationId": conversation, "reason": "typo"})))
        .await;
    assert_eq!(status, StatusCode::OK, "{cbody}");
    assert_eq!(cbody["data"]["messageId"], message_id.as_str());
    assert!(cbody["data"]["cancelledAt"].as_i64().unwrap() > 0);

    // Second cancel: already cancelled.
    let (status, cbody, _) = app
        .request("DELETE", &format!("/api/delayed-messages-v2/cancel/{message_id}"),
            Some(&token), Some(json!({"conversationId": conversation})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(cbody["error"].as_str().unwrap().contains("cancelled"), "{cbody}");

    // Status reflects the terminal state; unknown ids report not_found.
    let (_, sbody, _) = app
        .request("GET",
            &format!("/api/delayed-messages-v2/status/{message_id}?conversationId={conversation}"),
            Some(&token), None)
        .await;
    assert_eq!(sbody["data"]["status"], "cancelled");
    assert_eq!(sbody["data"]["canCancel"], false);
    let (_, sbody, _) = app
        .request("GET",
            &format!("/api/delayed-messages-v2/status/ghost?conversationId={conversation}"),
            Some(&token), None)
        .await;
    assert_eq!(sbody["data"]["exists"], false);
    assert_eq!(sbody["data"]["status"], "not_found");
}

#[tokio::test]
async fn v2_metrics_and_failed_inspection() {
    let app = spawn_app().await;
    let (token, _, conversation) = setup(&app).await;
    app.request("POST", "/api/delayed-messages-v2/send", Some(&token),
        Some(v2_send_body(&conversation, 100))).await;

    let (status, body, _) = app
        .request("GET", &format!("/api/delayed-messages-v2/metrics?conversationId={conversation}"),
            Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["totals"]["pending"], 1);
    assert!(body["data"]["nextScheduledTime"].is_string());

    let (status, body, _) = app
        .request("GET", &format!("/api/delayed-messages-v2/failed?conversationId={conversation}"),
            Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["count"], 0);
}

// ================================================================ legacy family

fn legacy_send_body(conversation: &str, sender: &str, delay: i64) -> serde_json::Value {
    json!({
        "conversationId": conversation,
        "content": "legacy scheduled",
        "delaySeconds": delay,
        "senderId": sender,
        "recipientId": "U-dm",
        "platform": "line",
    })
}

#[tokio::test]
async fn legacy_send_validates_all_field_rules() {
    let app = spawn_app().await;
    let (token, agent, conversation) = setup(&app).await;

    let cases = [
        (json!({}), "everything missing"),
        (legacy_send_body(&conversation, &agent, 0), "delay too small"),
        (legacy_send_body(&conversation, &agent, 121), "delay too large"),
        ({
            let mut b = legacy_send_body(&conversation, &agent, 10);
            b["content"] = json!("x".repeat(5001));
            b
        }, "content too long"),
        ({
            let mut b = legacy_send_body(&conversation, &agent, 10);
            b["platform"] = json!("telegram");
            b
        }, "bad platform"),
        ({
            let mut b = legacy_send_body(&conversation, &agent, 10);
            b["mediaUrl"] = json!("http://insecure.example.com/a.png");
            b
        }, "non-https media"),
    ];
    for (body, label) in cases {
        let (status, _, _) = app
            .request("POST", "/api/delayed-messages/send", Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label}");
    }

    let (status, body, _) = app
        .request("POST", "/api/delayed-messages/send", Some(&token),
            Some(legacy_send_body(&conversation, &agent, 30)))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Recall deadline equals the fire time (CRD 1252).
    assert_eq!(body["data"]["scheduledSendTime"], body["data"]["recallDeadline"]);
}

#[tokio::test]
async fn legacy_recall_enforces_marker_ownership_and_deadline() {
    let app = spawn_app().await;
    let (token, agent, conversation) = setup(&app).await;

    // Unknown id: marker missing.
    let (status, body, _) = app
        .request("POST", "/api/delayed-messages/recall/ghost", Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("not found"));

    let (_, body, _) = app
        .request("POST", "/api/delayed-messages/send", Some(&token),
            Some(legacy_send_body(&conversation, &agent, 60)))
        .await;
    let message_id = body["data"]["messageId"].as_str().unwrap().to_string();

    // A different signed-in user is not the sender.
    let other = app.seed_agent("other@test.dev", "pw123456", "agent").await;
    let _ = other;
    let (other_token, _, _) = app.login("other@test.dev", "pw123456").await;
    let (status, body, _) = app
        .request("POST", &format!("/api/delayed-messages/recall/{message_id}"),
            Some(&other_token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("only the sender"), "{body}");

    // The sender succeeds; the record turns cancelled and a recall log exists.
    let (status, body, _) = app
        .request("POST", &format!("/api/delayed-messages/recall/{message_id}"),
            Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let db_status: String =
        sqlx::query_scalar("SELECT status FROM scheduled_messages WHERE id = ?")
            .bind(&message_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(db_status, "cancelled");
    let recalls: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_recall_logs WHERE message_id = ?",
    )
    .bind(&message_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(recalls, 1);

    // Deadline passed: rewind the fire time of a fresh message.
    let (_, body, _) = app
        .request("POST", "/api/delayed-messages/send", Some(&token),
            Some(legacy_send_body(&conversation, &agent, 60)))
        .await;
    let expired_id = body["data"]["messageId"].as_str().unwrap().to_string();
    sqlx::query("UPDATE scheduled_messages SET scheduled_at = '2000-01-01T00:00:00Z' WHERE id = ?")
        .bind(&expired_id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, body, _) = app
        .request("POST", &format!("/api/delayed-messages/recall/{expired_id}"),
            Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("deadline"), "{body}");
}

#[tokio::test]
async fn legacy_pending_lists_only_the_callers_messages() {
    let app = spawn_app().await;
    let (token, agent, conversation) = setup(&app).await;
    app.request("POST", "/api/delayed-messages/send", Some(&token),
        Some(legacy_send_body(&conversation, &agent, 60))).await;
    app.request("POST", "/api/delayed-messages/send", Some(&token),
        Some(legacy_send_body(&conversation, &agent, 90))).await;

    // Pagination validation.
    let (status, _, _) = app
        .request("GET", "/api/delayed-messages/pending?page=0", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("GET", "/api/delayed-messages/pending?pageSize=101", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = app
        .request("GET", "/api/delayed-messages/pending", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 2);
    let first = &body["data"]["messages"][0];
    assert_eq!(first["customerName"], "DM Customer");
    assert_eq!(first["canRecall"], true);

    // Another caller sees none.
    app.seed_agent("empty@test.dev", "pw123456", "agent").await;
    let (other_token, _, _) = app.login("empty@test.dev", "pw123456").await;
    let (_, body, _) = app
        .request("GET", "/api/delayed-messages/pending", Some(&other_token), None)
        .await;
    assert_eq!(body["data"]["total"], 0);
}

#[tokio::test]
async fn legacy_reschedule_moves_fire_time_for_the_sender_only() {
    let app = spawn_app().await;
    let (token, agent, conversation) = setup(&app).await;
    let (_, body, _) = app
        .request("POST", "/api/delayed-messages/send", Some(&token),
            Some(legacy_send_body(&conversation, &agent, 10)))
        .await;
    let message_id = body["data"]["messageId"].as_str().unwrap().to_string();
    let original_fire = body["data"]["scheduledSendTime"].as_str().unwrap().to_string();

    // Validation and guards.
    let (status, _, _) = app
        .request("POST", &format!("/api/delayed-messages/reschedule/{message_id}"),
            Some(&token), Some(json!({"delaySeconds": 0})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body2, _) = app
        .request("POST", "/api/delayed-messages/reschedule/ghost",
            Some(&token), Some(json!({"delaySeconds": 50})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body2["error"].as_str().unwrap().contains("not found"));

    app.seed_agent("other2@test.dev", "pw123456", "agent").await;
    let (other_token, _, _) = app.login("other2@test.dev", "pw123456").await;
    let (status, body2, _) = app
        .request("POST", &format!("/api/delayed-messages/reschedule/{message_id}"),
            Some(&other_token), Some(json!({"delaySeconds": 50})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body2["error"].as_str().unwrap().contains("Permission denied"));

    // Sender reschedules: state stays pending, fire time moves.
    let (status, body2, _) = app
        .request("POST", &format!("/api/delayed-messages/reschedule/{message_id}"),
            Some(&token), Some(json!({"delaySeconds": 110})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body2}");
    let new_fire = body2["data"]["newSendTime"].as_str().unwrap();
    assert!(new_fire > original_fire.as_str());
    let db_status: String =
        sqlx::query_scalar("SELECT status FROM scheduled_messages WHERE id = ?")
            .bind(&message_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(db_status, "pending");

    // A cancelled message cannot be rescheduled.
    app.request("POST", &format!("/api/delayed-messages/recall/{message_id}"),
        Some(&token), Some(json!({}))).await;
    let (status, body2, _) = app
        .request("POST", &format!("/api/delayed-messages/reschedule/{message_id}"),
            Some(&token), Some(json!({"delaySeconds": 50})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body2["error"].as_str().unwrap().contains("cannot be rescheduled"), "{body2}");
}

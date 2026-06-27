//! Conversation-Session Management behavior tests (CRD §1.2B, lines 329-483).

mod common;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use serde_json::json;

async fn admin_token(app: &TestApp) -> String {
    app.seed_agent("admin@test.dev", "Secret123!", "admin")
        .await;
    app.login("admin@test.dev", "Secret123!").await.0
}

async fn agent_token(app: &TestApp, email: &str, team_id: Option<i64>) -> String {
    let id = app.seed_agent(email, "Secret123!", "agent").await;
    sqlx::query("UPDATE agents SET position = 'supervisor' WHERE id = $1")
        .bind(&id)
        .execute(&app.state.db)
        .await
        .unwrap();
    if let Some(t) = team_id {
        app.add_membership(&id, t, "member", true).await;
    }
    app.login(email, "Secret123!").await.0
}

async fn plain_agent_token(app: &TestApp, email: &str) -> String {
    app.seed_agent(email, "Secret123!", "agent").await;
    app.login(email, "Secret123!").await.0
}

/// Seed a conversation and return its id (uuid).
async fn seed_conv(app: &TestApp, team_id: Option<i64>) -> String {
    let cust = app
        .seed_customer("line", &uuid::Uuid::new_v4().to_string(), "Alice", None)
        .await;
    app.seed_conversation(cust, team_id, "active").await
}

fn iso_minutes_ago(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// --------------------------------------------------------------- health & info

#[tokio::test]
async fn health_and_info_are_open() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/sessions/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], json!("healthy"));

    let (status, body, _) = app.request("GET", "/api/sessions/info", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["endpoints"].is_array());
    assert!(body["data"]["features"].is_array());
    assert!(body["data"]["permissions"].is_object());
}

#[tokio::test]
async fn protected_routes_require_bearer_token() {
    let app = spawn_app().await;
    let (status, _, _) = app.request("GET", "/api/sessions", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn protected_routes_require_ops_position() {
    let app = spawn_app().await;
    let token = plain_agent_token(&app, "plain@test.dev").await;
    let (status, _, _) = app
        .request("GET", "/api/sessions", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ----------------------------------------------------------------------- create

#[tokio::test]
async fn create_session_returns_201_with_active_session() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&token),
            Some(json!({
                "conversationId": conv,
                "senderType": "customer",
                "topic": "Greeting",
                "priority": "high",
                "tags": ["a", "", "b"],
                "metadata": {"k": "v"},
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let s = &body["data"];
    assert_eq!(s["conversationId"], json!(conv));
    assert_eq!(s["sessionType"], json!("continuous"));
    assert_eq!(s["topic"], json!("Greeting"));
    assert_eq!(s["isActive"], json!(true));
    assert_eq!(s["messageCount"], json!(0));
    assert_eq!(s["priority"], json!("high"));
    assert_eq!(s["tags"], json!(["a", "b"])); // empties dropped (CRD 345)
    assert!(s["startTime"].is_string());
    assert!(s["endTime"].is_null());
}

#[tokio::test]
async fn create_session_derives_topic_from_message_content() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&token),
            Some(json!({
                "conversationId": conv,
                "senderType": "customer",
                "messageContent": "I want a refund for my last invoice",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["topic"], json!("Billing & Payments"));
}

#[tokio::test]
async fn create_session_validation_errors() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;

    // Missing / non-UUID conversationId.
    for body in [
        json!({"senderType": "customer"}),
        json!({"conversationId": "x", "senderType": "customer"}),
    ] {
        let (status, _, _) = app
            .request("POST", "/api/sessions", Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    // Invalid senderType.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&token),
            Some(json!({"conversationId": conv, "senderType": "robot"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Invalid sessionType / priority.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&token),
            Some(json!({"conversationId": conv, "senderType": "customer", "sessionType": "weird"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&token),
            Some(json!({"conversationId": conv, "senderType": "customer", "priority": "extreme"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Over-length topic.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&token),
            Some(
                json!({"conversationId": conv, "senderType": "customer", "topic": "x".repeat(201)}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Tags not an array / too many.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&token),
            Some(json!({"conversationId": conv, "senderType": "customer", "tags": "nope"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&token),
            Some(json!({
                "conversationId": conv,
                "senderType": "customer",
                "tags": (0..11).map(|i| i.to_string()).collect::<Vec<_>>(),
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Unparseable JSON.
    let (status, _) = app
        .request_raw("POST", "/api/sessions", Some(&token), "{nope")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_session_denies_agent_without_conversation_team_access() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let agent = agent_token(&app, "agent@test.dev", Some(team_a)).await;
    let conv = seed_conv(&app, Some(team_b)).await;

    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions",
            Some(&agent),
            Some(json!({"conversationId": conv, "senderType": "customer"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------------------- list

#[tokio::test]
async fn list_sessions_filters_paginates_and_summarizes() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    app.seed_session(&conv, true, Some("Billing"), None, None, 3)
        .await;
    app.seed_session(&conv, false, Some("Other"), None, None, 5)
        .await;
    let other_conv = seed_conv(&app, None).await;
    app.seed_session(&other_conv, true, Some("Misc"), None, None, 0)
        .await;

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/sessions?conversationId={conv}&pageSize=1"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(data["pagination"]["total"], json!(2));
    assert_eq!(data["pagination"]["totalPages"], json!(2));
    assert_eq!(data["pagination"]["hasNext"], json!(true));
    assert_eq!(data["summary"]["total"], json!(2));
    assert_eq!(data["summary"]["active"], json!(1));
    assert_eq!(data["summary"]["inactive"], json!(1));
    assert!(data["summary"]["byType"].is_object());
    assert!(data["summary"]["byPriority"].is_object());

    // isActive filter.
    let (_, body, _) = app
        .request("GET", "/api/sessions?isActive=false", Some(&token), None)
        .await;
    assert_eq!(body["data"]["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["sessions"][0]["topic"], json!("Other"));

    // topic substring filter.
    let (_, body, _) = app
        .request("GET", "/api/sessions?topic=Bill", Some(&token), None)
        .await;
    assert_eq!(body["data"]["sessions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn list_sessions_rejects_invalid_filters() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    for q in [
        "conversationId=nope",
        "isActive=banana",
        "sessionType=weird",
        "priority=extreme",
        "sentiment=meh",
        "startDate=tomorrow",
        "page=0",
        "page=1001",
        "pageSize=101",
        "pageSize=abc",
    ] {
        let (status, body, _) = app
            .request("GET", &format!("/api/sessions?{q}"), Some(&token), None)
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "filter {q} accepted: {body}"
        );
    }
}

// ----------------------------------------------------------------------- search

#[tokio::test]
async fn search_sessions_matches_topic_with_count() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    app.seed_session(&conv, true, Some("Billing question"), None, None, 0)
        .await;
    app.seed_session(&conv, true, Some("Shipping"), None, None, 0)
        .await;

    let (status, body, _) = app
        .request(
            "GET",
            "/api/sessions/search?query=billing",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], json!(1));
    assert_eq!(body["data"][0]["topic"], json!("Billing question"));
}

#[tokio::test]
async fn search_sessions_requires_min_two_chars() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    for q in ["", "query=a"] {
        let (status, _, _) = app
            .request(
                "GET",
                &format!("/api/sessions/search?{q}"),
                Some(&token),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _, _) = app
        .request(
            "GET",
            "/api/sessions/search?query=ab&limit=0",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// -------------------------------------------------------------------- get detail

#[tokio::test]
async fn get_session_scopes_agents_by_conversation_team() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let admin = admin_token(&app).await;
    let agent = agent_token(&app, "agent@test.dev", Some(team_a)).await;
    let mine = seed_conv(&app, Some(team_a)).await;
    let other = seed_conv(&app, Some(team_b)).await;
    let unassigned = seed_conv(&app, None).await;
    let s_mine = app.seed_session(&mine, true, None, None, None, 0).await;
    let s_other = app.seed_session(&other, true, None, None, None, 0).await;
    let s_unassigned = app
        .seed_session(&unassigned, true, None, None, None, 0)
        .await;

    // Admin sees any.
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/sessions/{s_other}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // Agent sees own-team sessions.
    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/sessions/{s_mine}"),
            Some(&agent),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["id"], json!(s_mine));
    // Access denied is indistinguishable from not-found: 404 (CRD 369).
    for sid in [&s_other, &s_unassigned] {
        let (status, _, _) = app
            .request("GET", &format!("/api/sessions/{sid}"), Some(&agent), None)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    // Truly missing session, and malformed id.
    let ghost = uuid::Uuid::new_v4();
    let (status, _, _) = app
        .request("GET", &format!("/api/sessions/{ghost}"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = app
        .request("GET", "/api/sessions/not-a-uuid", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ----------------------------------------------------------------------- update

#[tokio::test]
async fn update_session_applies_fields() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let sid = app
        .seed_session(&conv, true, Some("Old"), None, None, 0)
        .await;
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/sessions/{sid}"),
            Some(&token),
            Some(json!({
                "topic": "New topic",
                "priority": "urgent",
                "sentiment": "positive",
                "tags": ["x"],
                "isActive": false,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"], json!("Session updated successfully"));
    assert_eq!(body["data"]["topic"], json!("New topic"));
    assert_eq!(body["data"]["priority"], json!("urgent"));
    assert_eq!(body["data"]["sentiment"], json!("positive"));
    assert_eq!(body["data"]["isActive"], json!(false));
    assert_eq!(body["data"]["tags"], json!(["x"]));
}

#[tokio::test]
async fn update_session_error_conditions() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let admin = admin_token(&app).await;
    let agent = agent_token(&app, "agent@test.dev", Some(team_a)).await;
    let conv = seed_conv(&app, Some(team_b)).await;
    let sid = app.seed_session(&conv, true, None, None, None, 0).await;

    // Empty body.
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/sessions/{sid}"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Invalid enum / boolean / date.
    for body in [
        json!({"sessionType": "weird"}),
        json!({"isActive": "yes"}),
        json!({"endTime": "tomorrow"}),
        json!({"tags": (0..11).collect::<Vec<_>>().iter().map(|i| i.to_string()).collect::<Vec<_>>()}),
    ] {
        let (status, _, _) = app
            .request(
                "PUT",
                &format!("/api/sessions/{sid}"),
                Some(&admin),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    // Agent without team access -> 403 (CRD 372).
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/sessions/{sid}"),
            Some(&agent),
            Some(json!({"topic": "t"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Missing session -> not-found semantics.
    let ghost = uuid::Uuid::new_v4();
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/sessions/{ghost}"),
            Some(&admin),
            Some(json!({"topic": "t"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ----------------------------------------------------------------------- delete

#[tokio::test]
async fn delete_session_is_admin_only_hard_delete() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let agent = agent_token(&app, "agent@test.dev", None).await;
    let conv = seed_conv(&app, None).await;
    let sid = app.seed_session(&conv, true, None, None, None, 0).await;

    let (status, body, _) = app
        .request(
            "DELETE",
            &format!("/api/sessions/{sid}"),
            Some(&agent),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("administrator"));

    let (status, body, _) = app
        .request(
            "DELETE",
            &format!("/api/sessions/{sid}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["deleted"], json!(true));
    assert_eq!(body["data"]["sessionId"], json!(sid));
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(remaining, 0);

    // Already gone -> 404; malformed id -> 400.
    let (status, _, _) = app
        .request(
            "DELETE",
            &format!("/api/sessions/{sid}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = app
        .request("DELETE", "/api/sessions/zzz", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ----------------------------------------------------------------- close & reopen

#[tokio::test]
async fn close_then_reopen_session() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let sid = app.seed_session(&conv, true, None, None, None, 0).await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/sessions/{sid}/close"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["closed"], json!(true));
    let (active, ended): (i64, Option<String>) =
        sqlx::query_as("SELECT is_active, ended_at FROM conversation_sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(active, 0);
    assert!(ended.is_some());

    // Closing again is 404 ("not closable").
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/sessions/{sid}/close"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/sessions/{sid}/reopen"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["reopened"], json!(true));
    let (active, ended): (i64, Option<String>) =
        sqlx::query_as("SELECT is_active, ended_at FROM conversation_sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(active, 1);
    assert!(ended.is_none());

    // Reopening an active session is 404 ("not reopenable").
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/sessions/{sid}/reopen"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn close_denied_for_agent_without_team_access() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let agent = agent_token(&app, "agent@test.dev", Some(team_a)).await;
    let conv = seed_conv(&app, Some(team_b)).await;
    let sid = app.seed_session(&conv, true, None, None, None, 0).await;
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/sessions/{sid}/close"),
            Some(&agent),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// --------------------------------------------------------------------- messages

#[tokio::test]
async fn session_messages_paginated() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let sid = app.seed_session(&conv, true, None, None, None, 2).await;
    app.seed_message_full(&conv, "customer", "one", None, Some(&sid), Some(1))
        .await;
    app.seed_message_full(&conv, "agent", "two", None, Some(&sid), Some(2))
        .await;

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/sessions/{sid}/messages?pageSize=1"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["sessionId"], json!(sid));
    assert_eq!(data["messageCount"], json!(2));
    assert_eq!(data["messages"].as_array().unwrap().len(), 1);
    assert_eq!(data["messages"][0]["content"], json!("one"));
    assert_eq!(data["messages"][0]["sessionSeq"], json!(1));
    assert_eq!(data["pagination"]["totalPages"], json!(2));

    let (status, _, _) = app
        .request("GET", "/api/sessions/bogus/messages", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn session_messages_denies_agent_without_team_access() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let agent = agent_token(&app, "agent@test.dev", Some(team_a)).await;
    let conv = seed_conv(&app, Some(team_b)).await;
    let sid = app.seed_session(&conv, true, None, None, None, 1).await;
    app.seed_message_full(&conv, "customer", "secret", None, Some(&sid), Some(1))
        .await;

    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/sessions/{sid}/messages"),
            Some(&agent),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ----------------------------------------------------------------- session health

#[tokio::test]
async fn session_health_reports_issues_and_suggestions() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    // Healthy: fresh session.
    let healthy = app.seed_session(&conv, true, None, None, None, 1).await;
    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/sessions/{healthy}/health"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["healthy"], json!(true));
    assert_eq!(body["data"]["issues"].as_array().unwrap().len(), 0);

    // Unhealthy: long-running, inactive, and message-heavy (CRD 409).
    let old_start = iso_minutes_ago(49 * 60);
    let stale = iso_minutes_ago(120);
    let sick = app
        .seed_session(&conv, true, None, Some(&old_start), Some(&stale), 150)
        .await;
    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/sessions/{sick}/health"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["healthy"], json!(false));
    assert_eq!(body["data"]["issues"].as_array().unwrap().len(), 3);
    assert_eq!(body["data"]["suggestions"].as_array().unwrap().len(), 3);

    // Missing session -> not-found; malformed id -> 400.
    let ghost = uuid::Uuid::new_v4();
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/sessions/{ghost}/health"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = app
        .request("GET", "/api/sessions/zzz/health", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn session_health_hides_sessions_from_other_teams() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let agent = agent_token(&app, "ops@test.dev", Some(team_a)).await;
    let conv = seed_conv(&app, Some(team_b)).await;
    let sid = app.seed_session(&conv, true, None, None, None, 1).await;

    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/sessions/{sid}/health"),
            Some(&agent),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------ topic update

#[tokio::test]
async fn update_topic_sets_topic() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let sid = app
        .seed_session(&conv, true, Some("Old"), None, None, 0)
        .await;
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/sessions/{sid}/topic"),
            Some(&token),
            Some(json!({"topic": "Fresh"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let topic: Option<String> =
        sqlx::query_scalar("SELECT topic FROM conversation_sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(topic.as_deref(), Some("Fresh"));

    // Missing session -> 404 (CRD 418).
    let ghost = uuid::Uuid::new_v4();
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/sessions/{ghost}/topic"),
            Some(&token),
            Some(json!({"topic": "x"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------- statistics

#[tokio::test]
async fn stats_are_admin_only_with_breakdowns() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let agent = agent_token(&app, "agent@test.dev", None).await;
    let conv = seed_conv(&app, None).await;
    app.seed_session(&conv, true, Some("Billing"), None, None, 4)
        .await;
    app.seed_session(&conv, false, Some("Billing"), None, None, 2)
        .await;

    let (status, body, _) = app
        .request("GET", "/api/sessions/stats", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let d = &body["data"];
    assert_eq!(d["total"], json!(2));
    assert_eq!(d["active"], json!(1));
    assert_eq!(d["inactive"], json!(1));
    assert_eq!(d["avgMessagesPerSession"], json!(3.0));
    assert!(d["byType"].is_object());
    assert!(d["bySentiment"].is_object());
    assert_eq!(d["topicDistribution"][0]["topic"], json!("Billing"));
    assert_eq!(d["topicDistribution"][0]["percentage"], json!(100.0));
    assert!(d["perDay"].is_array());

    let (status, _, _) = app
        .request("GET", "/api/sessions/stats", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn per_conversation_stats_include_conversation_id() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let other = seed_conv(&app, None).await;
    app.seed_session(&conv, true, None, None, None, 0).await;
    app.seed_session(&other, true, None, None, None, 0).await;

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/sessions/stats/{conv}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["conversationId"], json!(conv));
    assert_eq!(body["data"]["total"], json!(1));

    let (status, _, _) = app
        .request("GET", "/api/sessions/stats/not-a-uuid", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ----------------------------------------------------------------- activity stats

#[tokio::test]
async fn activity_stats_bucketed_with_summary() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let agent = agent_token(&app, "agent@test.dev", None).await;
    let conv = seed_conv(&app, None).await;
    let sid = app.seed_session(&conv, true, None, None, None, 0).await;
    app.seed_message_full(&conv, "customer", "hi", None, Some(&sid), Some(1))
        .await;

    let (status, body, _) = app
        .request(
            "GET",
            "/api/sessions/activity?timeRange=day",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let d = &body["data"];
    assert_eq!(d["timeRange"], json!("day"));
    assert!(!d["buckets"].as_array().unwrap().is_empty());
    assert_eq!(d["summary"]["totalSessionsCreated"], json!(1));
    assert_eq!(d["summary"]["totalMessages"], json!(1));
    assert!(d["summary"]["peakActivityHour"].is_string());

    // Default range is week; bad range is 400; agents are forbidden.
    let (status, body, _) = app
        .request("GET", "/api/sessions/activity", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["timeRange"], json!("week"));
    let (status, _, _) = app
        .request(
            "GET",
            "/api/sessions/activity?timeRange=decade",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("GET", "/api/sessions/activity", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ----------------------------------------------------------------------- batch

#[tokio::test]
async fn batch_close_collects_per_item_results() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let open = app.seed_session(&conv, true, None, None, None, 0).await;
    let ghost = uuid::Uuid::new_v4().to_string();

    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions/batch",
            Some(&admin),
            Some(json!({"sessionIds": [open, ghost], "action": "close"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let d = &body["data"];
    assert_eq!(d["total"], json!(2));
    assert_eq!(d["succeeded"], json!(1));
    assert_eq!(d["failed"], json!(1));
    assert_eq!(d["results"].as_array().unwrap().len(), 2);
    assert_eq!(d["results"][0]["success"], json!(true));
    assert_eq!(d["results"][1]["success"], json!(false));
}

#[tokio::test]
async fn batch_update_priority_and_tags_and_delete() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let sid = app.seed_session(&conv, true, None, None, None, 0).await;

    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions/batch",
            Some(&admin),
            Some(json!({"sessionIds": [sid], "action": "update_priority", "data": {"priority": "urgent"}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions/batch",
            Some(&admin),
            Some(json!({"sessionIds": [sid], "action": "add_tags", "data": {"tags": ["vip"]}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (priority, tags): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT priority, tags FROM conversation_sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(priority.as_deref(), Some("urgent"));
    assert_eq!(tags.as_deref(), Some("[\"vip\"]"));

    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions/batch",
            Some(&admin),
            Some(json!({"sessionIds": [sid], "action": "delete"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn batch_validation_and_authorization_errors() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let agent = agent_token(&app, "agent@test.dev", None).await;
    let sid = uuid::Uuid::new_v4().to_string();

    // Admin only.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions/batch",
            Some(&agent),
            Some(json!({"sessionIds": [sid], "action": "close"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Empty list, oversized list, malformed id, invalid action, missing data.
    let oversized: Vec<String> = (0..101).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    for body in [
        json!({"sessionIds": [], "action": "close"}),
        json!({"sessionIds": oversized, "action": "close"}),
        json!({"sessionIds": ["nope"], "action": "close"}),
        json!({"sessionIds": [sid], "action": "explode"}),
        json!({"sessionIds": [sid], "action": "update_priority"}),
        json!({"sessionIds": [sid], "action": "add_tags"}),
    ] {
        let (status, resp, _) = app
            .request("POST", "/api/sessions/batch", Some(&admin), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    }
}

// ----------------------------------------------------------------- get-or-create

#[tokio::test]
async fn get_or_create_starts_first_session_then_continues() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions/get-or-create",
            Some(&token),
            Some(json!({"conversation_id": conv, "messageContent": "hello", "senderType": "customer"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first = body["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["isActive"], json!(true));

    // A fresh active session continues: same id, refreshed last-activity.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions/get-or-create",
            Some(&token),
            Some(json!({"conversation_id": conv, "messageContent": "more", "senderType": "customer"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["id"], json!(first));
}

#[tokio::test]
async fn get_or_create_closes_stale_session_and_opens_new_segment() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    let stale_activity = iso_minutes_ago(45); // beyond the 30-minute gap (CRD 480)
    let stale = app
        .seed_session(&conv, true, None, None, Some(&stale_activity), 1)
        .await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions/get-or-create",
            Some(&token),
            Some(json!({"conversation_id": conv, "messageContent": "my order is broken", "senderType": "customer"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let new_id = body["data"]["id"].as_str().unwrap();
    assert_ne!(new_id, stale);
    // The new segment carries a suggested topic (CRD 450).
    assert!(body["data"]["topic"].is_string());

    let (active, ended): (i64, Option<String>) =
        sqlx::query_as("SELECT is_active, ended_at FROM conversation_sessions WHERE id = $1")
            .bind(&stale)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(active, 0);
    assert!(ended.is_some());
}

#[tokio::test]
async fn get_or_create_requires_all_three_fields() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    for body in [
        json!({"messageContent": "x", "senderType": "customer"}),
        json!({"conversation_id": conv, "senderType": "customer"}),
        json!({"conversation_id": conv, "messageContent": "x"}),
    ] {
        let (status, _, _) = app
            .request(
                "POST",
                "/api/sessions/get-or-create",
                Some(&token),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn get_or_create_denies_agent_without_conversation_team_access() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let agent = agent_token(&app, "agent@test.dev", Some(team_a)).await;
    let conv = seed_conv(&app, Some(team_b)).await;

    let (status, _, _) = app
        .request(
            "POST",
            "/api/sessions/get-or-create",
            Some(&agent),
            Some(json!({"conversation_id": conv, "messageContent": "hello", "senderType": "customer"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------- detect boundary

#[tokio::test]
async fn detect_boundary_reports_reasons_without_state_changes() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;

    // No current session -> first_session, confidence ~1.0.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions/detect-boundary",
            Some(&token),
            Some(json!({"messageContent": "hello", "senderType": "customer"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["shouldCreateNew"], json!(true));
    assert_eq!(body["data"]["reason"], json!("first_session"));
    assert_eq!(body["data"]["confidence"], json!(1.0));
    assert!(body["data"]["suggestedTopic"].is_string());

    // Fresh active session -> continue.
    let fresh = app.seed_session(&conv, true, None, None, None, 1).await;
    let (_, body, _) = app
        .request(
            "POST",
            "/api/sessions/detect-boundary",
            Some(&token),
            Some(json!({"currentSessionId": fresh, "messageContent": "ok thanks", "senderType": "customer"})),
        )
        .await;
    assert_eq!(body["data"]["shouldCreateNew"], json!(false));

    // Customer topic-change cue (CRD 480).
    let (_, body, _) = app
        .request(
            "POST",
            "/api/sessions/detect-boundary",
            Some(&token),
            Some(json!({"currentSessionId": fresh, "messageContent": "by the way, another question", "senderType": "customer"})),
        )
        .await;
    assert_eq!(body["data"]["shouldCreateNew"], json!(true));
    assert_eq!(body["data"]["reason"], json!("topic_change"));

    // Message-limit boundary.
    let full = app.seed_session(&conv, true, None, None, None, 50).await;
    let (_, body, _) = app
        .request(
            "POST",
            "/api/sessions/detect-boundary",
            Some(&token),
            Some(
                json!({"currentSessionId": full, "messageContent": "ok", "senderType": "customer"}),
            ),
        )
        .await;
    assert_eq!(body["data"]["reason"], json!("message_limit"));

    // Missing fields -> 400.
    for body in [
        json!({"senderType": "customer"}),
        json!({"messageContent": "x"}),
    ] {
        let (status, _, _) = app
            .request(
                "POST",
                "/api/sessions/detect-boundary",
                Some(&token),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

// ------------------------------------------------------------------ topic family

#[tokio::test]
async fn topic_stats_analyze_and_suggest() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let conv = seed_conv(&app, None).await;
    app.seed_session(&conv, true, Some("Billing"), None, None, 0)
        .await;

    let (status, body, _) = app
        .request("GET", "/api/sessions/topics/stats", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["total"], json!(1));
    assert_eq!(body["data"]["topics"][0]["topic"], json!("Billing"));

    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions/topics/analyze",
            Some(&token),
            Some(json!({"messageContent": "there is an error in the app"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["topic"], json!("Technical Support"));
    assert!(body["data"]["confidence"].is_number());

    let (status, body, _) = app
        .request(
            "POST",
            "/api/sessions/topics/suggest",
            Some(&token),
            Some(json!({"messageContent": "refund for my broken order", "limit": 2})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], json!(2));
    assert_eq!(body["data"].as_array().unwrap().len(), 2);

    // Missing content -> 400 on both analyze and suggest.
    for path in [
        "/api/sessions/topics/analyze",
        "/api/sessions/topics/suggest",
    ] {
        let (status, _, _) = app
            .request("POST", path, Some(&token), Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

// ---------------------------------------------------- module-wide gates & fallback

#[tokio::test]
async fn unknown_module_path_lists_available_endpoints() {
    let app = spawn_app().await;
    let (status, body, _) = app
        .request("GET", "/api/sessions/foo/definitely-not-real", None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["success"], json!(false));
    assert!(body["availableEndpoints"].as_array().unwrap().len() > 10);
}

#[tokio::test]
async fn oversized_body_returns_413() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let huge = "x".repeat(1024 * 1024 + 1);
    let (status, body) = app
        .request_raw("POST", "/api/sessions", Some(&token), &huge)
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn mutating_endpoints_are_rate_limited_per_client() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    // 60 requests pass the limiter (handler then rejects the body); the 61st is 429.
    for _ in 0..60 {
        let (status, _, _) = app
            .request("POST", "/api/sessions", Some(&token), Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _, headers) = app
        .request("POST", "/api/sessions", Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.get("Retry-After").is_some());
    assert!(headers.get("X-RateLimit-Limit").is_some());

    // Read endpoints stay unaffected.
    let (status, _, _) = app
        .request("GET", "/api/sessions", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

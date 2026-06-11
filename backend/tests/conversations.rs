//! Conversations (Agent Side) behavior tests (CRD §2.1, lines 651-830).

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{spawn_app, TestApp};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn admin_token(app: &TestApp) -> String {
    app.seed_agent("admin@test.dev", "Secret123!", "admin").await;
    app.login("admin@test.dev", "Secret123!").await.0
}

async fn agent_token(app: &TestApp, email: &str, team_id: i64) -> (String, String) {
    let id = app.seed_agent(email, "Secret123!", "agent").await;
    app.add_membership(&id, team_id, "member", true).await;
    let token = app.login(email, "Secret123!").await.0;
    (token, id)
}

async fn set_updated_at(app: &TestApp, conversation_id: &str, when: &str) {
    sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
        .bind(when)
        .bind(conversation_id)
        .execute(&app.state.db)
        .await
        .unwrap();
}

// ------------------------------------------------------------------------- list

#[tokio::test]
async fn list_orders_by_updated_desc_with_preview_and_unread() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let older = app.seed_conversation(cust, Some(team), "active").await;
    let newer = app.seed_conversation(cust, None, "active").await;
    set_updated_at(&app, &older, "2026-01-01T00:00:00.000Z").await;
    set_updated_at(&app, &newer, "2026-02-01T00:00:00.000Z").await;
    app.seed_message(&newer, "customer", "hello there", Some("2026-02-01T00:00:00.000Z")).await;

    let (status, body, _) = app.request("GET", "/api/conversations", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], json!(newer));
    assert_eq!(items[1]["id"], json!(older));
    // Preview + unread for the conversation with a customer message.
    assert_eq!(items[0]["lastMessage"]["content"], json!("hello there"));
    assert_eq!(items[0]["lastMessageContent"], json!("hello there"));
    assert_eq!(items[0]["unreadCount"], json!(1));
    assert_eq!(items[0]["customerName"], json!("Alice"));
    assert_eq!(items[0]["customer"]["displayName"], json!("Alice"));
    assert_eq!(items[1]["assignedTeam"]["name"], json!("Support"));
    assert!(items[1]["lastMessage"].is_null());
    assert_eq!(items[1]["unreadCount"], json!(0));
}

#[tokio::test]
async fn list_scopes_agents_to_their_teams_plus_unassigned_pool() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (token, _) = agent_token(&app, "agent@test.dev", team_a).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let unassigned = app.seed_conversation(cust, None, "active").await;
    let mine = app.seed_conversation(cust, Some(team_a), "assigned").await;
    let _other = app.seed_conversation(cust, Some(team_b), "assigned").await;

    let (status, body, _) = app.request("GET", "/api/conversations", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> =
        body["data"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&unassigned.as_str()));
    assert!(ids.contains(&mine.as_str()));
}

#[tokio::test]
async fn list_filters_by_tags_direct_and_via_customer_ignoring_non_numeric() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let admin: String = sqlx::query_scalar("SELECT id FROM agents LIMIT 1")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let tagged_cust = app.seed_customer("line", "U1", "Alice", None).await;
    let plain_cust = app.seed_customer("line", "U2", "Bob", None).await;
    let direct = app.seed_conversation(plain_cust, None, "active").await;
    let indirect = app.seed_conversation(tagged_cust, None, "active").await;
    let _unmatched = app.seed_conversation(plain_cust, None, "active").await;
    let tag = app.seed_tag("vip", &admin).await;
    app.add_customer_tag(tagged_cust, tag, &admin).await;
    sqlx::query(
        "INSERT INTO conversation_tags (conversation_id, tag_id, assigned_by, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&direct)
    .bind(tag)
    .bind(&admin)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, body, _) = app
        .request("GET", &format!("/api/conversations?tagIds=abc,{tag}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> =
        body["data"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&direct.as_str()));
    assert!(ids.contains(&indirect.as_str()));
}

#[tokio::test]
async fn list_filters_by_search_and_updated_window() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let alice = app.seed_customer("line", "U1", "Alice Wonderland", None).await;
    let bob = app.seed_customer("line", "U2", "Bob", None).await;
    let c_alice = app.seed_conversation(alice, None, "active").await;
    let c_bob = app.seed_conversation(bob, None, "active").await;
    set_updated_at(&app, &c_alice, "2026-01-15T00:00:00.000Z").await;
    set_updated_at(&app, &c_bob, "2026-03-15T00:00:00.000Z").await;

    // Case-insensitive customer-name substring search (CRD 668).
    let (status, body, _) =
        app.request("GET", "/api/conversations?search=wonder", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["id"], json!(c_alice));

    // customerName filter behaves the same way (CRD 669).
    let (_, body, _) =
        app.request("GET", "/api/conversations?customerName=BOB", Some(&token), None).await;
    assert_eq!(body["data"][0]["id"], json!(c_bob));

    // Inclusive updated window (CRD 670-671).
    let (_, body, _) = app
        .request(
            "GET",
            "/api/conversations?updatedAfter=2026-02-01T00:00:00.000Z&updatedBefore=2026-04-01T00:00:00.000Z",
            Some(&token),
            None,
        )
        .await;
    let ids: Vec<&str> =
        body["data"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![c_bob.as_str()]);
}

// ----------------------------------------------------------------------- detail

#[tokio::test]
async fn detail_returns_extended_shape() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team), "assigned").await;
    app.seed_message(&conv, "customer", "hi", None).await;

    let (status, body, _) =
        app.request("GET", &format!("/api/conversations/{conv}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["id"], json!(conv));
    assert_eq!(body["data"]["assignedTeam"]["name"], json!("Support"));
    assert_eq!(body["data"]["customer"]["platformUserId"], json!("U1"));
    assert!(body["data"]["customer"].get("email").is_some());
    assert_eq!(body["data"]["unreadCount"], json!(1));
    assert_eq!(body["data"]["lastMessageContent"], json!("hi"));
}

#[tokio::test]
async fn detail_denies_agent_outside_assigned_team() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (token, _) = agent_token(&app, "agent@test.dev", team_a).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team_b), "assigned").await;

    let (status, body, _) =
        app.request("GET", &format!("/api/conversations/{conv}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], json!("Permission denied"));
}

#[tokio::test]
async fn detail_missing_conversation_is_404() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let (status, body, _) =
        app.request("GET", "/api/conversations/nope", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["success"], json!(false));
    assert_eq!(body["error"], json!("Conversation not found"));
}

// ------------------------------------------------------------------ mark as read

#[tokio::test]
async fn mark_read_records_marker_and_reduces_unread() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    app.seed_message(&conv, "customer", "hi", Some("2026-01-01T00:00:00.000Z")).await;

    let (status, body, _) =
        app.request("PUT", &format!("/api/conversations/{conv}/read"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["lastReadAt"].is_string());

    let (_, body, _) =
        app.request("GET", &format!("/api/conversations/{conv}"), Some(&token), None).await;
    assert_eq!(body["data"]["unreadCount"], json!(0));
}

#[tokio::test]
async fn mark_read_succeeds_even_for_missing_conversation() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let (status, body, _) =
        app.request("PUT", "/api/conversations/ghost/read", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn mark_read_denied_outside_team_scope() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (token, _) = agent_token(&app, "agent@test.dev", team_a).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team_b), "assigned").await;
    let (status, _, _) =
        app.request("PUT", &format!("/api/conversations/{conv}/read"), Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------- assign

#[tokio::test]
async fn assign_sets_team_status_history_and_reversible_audit() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/assign"),
            Some(&token),
            Some(json!({ "teamId": team, "reason": "routing" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"], json!("Conversation assigned successfully"));
    assert_eq!(body["data"]["status"], json!("assigned"));
    assert_eq!(body["data"]["assignedTeam"]["id"], json!(team));
    assert_eq!(body["data"]["customer"]["name"], json!("Alice"));

    // Reason supplied -> one routing-history record (CRD 706).
    let transfers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_transfers WHERE conversation_id = ? AND transfer_type = 'assign'",
    )
    .bind(&conv)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(transfers, 1);

    // Reversible audit entry with old/new team snapshots (CRD 704, 808).
    let details: String = sqlx::query_scalar(
        "SELECT details FROM activity_logs WHERE action = 'conversation assign' AND resource_id = ?",
    )
    .bind(&conv)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let details: Value = serde_json::from_str(&details).unwrap();
    assert_eq!(details["reversible"], json!(true));
    assert_eq!(details["old"]["teamId"], json!(null));
    assert_eq!(details["new"]["teamId"], json!(team));
}

#[tokio::test]
async fn assign_without_reason_skips_history() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/assign"),
            Some(&token),
            Some(json!({ "teamId": team })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let transfers: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_transfers WHERE conversation_id = ?")
            .bind(&conv)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(transfers, 0);
}

#[tokio::test]
async fn assign_requires_team_id() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let (status, body, _) = app
        .request("POST", &format!("/api/conversations/{conv}/assign"), Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("Team ID is required for assignment"));
}

#[tokio::test]
async fn assign_missing_conversation_is_404() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let (status, body, _) = app
        .request("POST", "/api/conversations/ghost/assign", Some(&token), Some(json!({"teamId": team})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], json!("Conversation not found"));
}

#[tokio::test]
async fn assign_denied_for_agent_outside_team() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (token, _) = agent_token(&app, "agent@test.dev", team_a).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team_b), "assigned").await;
    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/assign"),
            Some(&token),
            Some(json!({"teamId": team_a})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], json!("Permission denied"));
}

#[tokio::test]
async fn assign_can_be_restored_via_activity_restore() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/assign"),
            Some(&token),
            Some(json!({"teamId": team})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let activity_id: i64 = sqlx::query_scalar(
        "SELECT id FROM activity_logs WHERE action = 'conversation assign' AND resource_id = ?",
    )
    .bind(&conv)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let (status, body, _) = app
        .request("POST", &format!("/api/activities/{activity_id}/restore"), Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (team_id, conv_status): (Option<i64>, String) =
        sqlx::query_as("SELECT team_id, status FROM conversations WHERE id = ?")
            .bind(&conv)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(team_id, None);
    assert_eq!(conv_status, "active");
}

// -------------------------------------------------------------------- unassign

#[tokio::test]
async fn unassign_clears_team_and_resets_active() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team), "assigned").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/unassign"),
            Some(&token),
            Some(json!({"reason": "wrong team"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"], json!("Conversation unassigned successfully"));
    assert!(body["data"]["assignedTeam"].is_null());
    assert_eq!(body["data"]["status"], json!("active"));

    let transfers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_transfers WHERE conversation_id = ? AND transfer_type = 'unassign'",
    )
    .bind(&conv)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(transfers, 1);
}

#[tokio::test]
async fn unassign_tolerates_missing_body() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team), "assigned").await;
    let (status, _, _) = app
        .request("POST", &format!("/api/conversations/{conv}/unassign"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn unassign_rejects_unassigned_conversation() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let (status, body, _) = app
        .request("POST", &format!("/api/conversations/{conv}/unassign"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("Conversation is not assigned"));
}

#[tokio::test]
async fn unassign_missing_conversation_is_404() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let (status, _, _) =
        app.request("POST", "/api/conversations/ghost/unassign", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// -------------------------------------------------------------------- transfer

#[tokio::test]
async fn transfer_always_writes_history_and_returns_no_data() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team_a), "assigned").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/transfer"),
            Some(&token),
            Some(json!({"fromTeamId": team_a, "toTeamId": team_b})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"], json!("Conversation transferred successfully"));
    assert!(body.get("data").is_none());

    let (team_id, conv_status): (Option<i64>, String) =
        sqlx::query_as("SELECT team_id, status FROM conversations WHERE id = ?")
            .bind(&conv)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(team_id, Some(team_b));
    assert_eq!(conv_status, "active");

    // History is written even without a reason (CRD 726).
    let (from, to): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT from_team_id, to_team_id FROM conversation_transfers
         WHERE conversation_id = ? AND transfer_type = 'transfer'",
    )
    .bind(&conv)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(from, Some(team_a));
    assert_eq!(to, Some(team_b));
}

#[tokio::test]
async fn transfer_requires_target_team() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let (status, body, _) = app
        .request("POST", &format!("/api/conversations/{conv}/transfer"), Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("Target team ID is required for transfer"));
}

#[tokio::test]
async fn transfer_missing_conversation_is_404() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("A").await;
    let (status, _, _) = app
        .request("POST", "/api/conversations/ghost/transfer", Some(&token), Some(json!({"toTeamId": team})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ----------------------------------------------------------- conversation labels

#[tokio::test]
async fn conversation_tags_roundtrip() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let admin: String =
        sqlx::query_scalar("SELECT id FROM agents LIMIT 1").fetch_one(&app.state.db).await.unwrap();
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let tag = app.seed_tag("vip", &admin).await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/tags"),
            Some(&token),
            Some(json!({"tagIds": [tag]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], json!("Tags added to conversation successfully"));

    let (_, body, _) =
        app.request("GET", &format!("/api/conversations/{conv}/tags"), Some(&token), None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["name"], json!("vip"));

    let (status, body, _) = app
        .request(
            "DELETE",
            &format!("/api/conversations/{conv}/tags"),
            Some(&token),
            Some(json!({"tagIds": [tag]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], json!("Tags removed from conversation successfully"));
}

// ------------------------------------------------------------------- messages list

#[tokio::test]
async fn list_messages_paginates_newest_first_with_attachments() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let m1 = app.seed_message(&conv, "customer", "first", Some("2026-01-01T00:00:00.000Z")).await;
    let m2 = app.seed_message(&conv, "agent", "second", Some("2026-01-02T00:00:00.000Z")).await;
    sqlx::query(
        "INSERT INTO attachments (id, message_id, conversation_id, file_name, content_type, file_size, file_url, storage_key, created_at)
         VALUES ('att-1', ?, ?, 'doc.pdf', 'application/pdf', 42, '/uploads/doc.pdf', 'missing-key', ?)",
    )
    .bind(&m2)
    .bind(&conv)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/conversations/{conv}/messages?page=1&pageSize=1"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["total"], json!(2));
    assert_eq!(data["totalPages"], json!(2));
    assert_eq!(data["hasMore"], json!(true));
    let item = &data["items"][0];
    assert_eq!(item["id"], json!(m2));
    assert_eq!(item["senderType"], json!("agent"));
    assert!(item["createdAt"].is_i64());
    assert_eq!(item["attachments"][0]["filename"], json!("doc.pdf"));
    assert_eq!(item["attachments"][0]["url"], json!("/uploads/doc.pdf"));
    // No stored object on disk -> no force-download URL (CRD 763).
    assert!(item["attachments"][0]["downloadUrl"].is_null());

    // Customer sender is surfaced as "user" with the customer's name (CRD 760).
    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/conversations/{conv}/messages?page=2&pageSize=1"),
            Some(&token),
            None,
        )
        .await;
    let item = &body["data"]["items"][0];
    assert_eq!(item["id"], json!(m1));
    assert_eq!(item["senderType"], json!("user"));
    assert_eq!(item["senderName"], json!("Alice"));
    assert_eq!(body["data"]["hasMore"], json!(false));
}

#[tokio::test]
async fn list_messages_permission_and_not_found_errors() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (agent, _) = agent_token(&app, "agent@test.dev", team_a).await;
    let admin = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team_b), "assigned").await;

    let (status, body, _) = app
        .request("GET", &format!("/api/conversations/{conv}/messages"), Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], json!("Permission denied"));

    let (status, body, _) =
        app.request("GET", "/api/conversations/ghost/messages", Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], json!("Conversation not found"));
}

// ----------------------------------------------------------------- send message

async fn wait_for_delivery(app: &TestApp, message_id: &str) -> String {
    for _ in 0..100 {
        let status: String =
            sqlx::query_scalar("SELECT delivery_status FROM messages WHERE id = ?")
                .bind(message_id)
                .fetch_one(&app.state.db)
                .await
                .unwrap();
        if status != "pending" {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("delivery never resolved for {message_id}");
}

#[tokio::test]
async fn send_message_returns_pending_then_delivers_on_line() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let agent_id: String =
        sqlx::query_scalar("SELECT id FROM agents LIMIT 1").fetch_one(&app.state.db).await.unwrap();
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/messages"),
            Some(&token),
            Some(json!({"content": "hello", "senderId": agent_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"], json!("Message queued for delivery"));
    assert_eq!(body["data"]["deliveryStatus"], json!("pending"));
    assert_eq!(body["data"]["isSent"], json!(false));
    assert_eq!(body["data"]["senderType"], json!("agent"));
    assert!(body["data"]["platformMessageId"].is_null());
    assert!(body["data"]["createdAt"].is_i64());

    // The send response returns before delivery confirms; the pending -> sent
    // transition is observable on later reads (CRD 773).
    let message_id = body["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(wait_for_delivery(&app, &message_id).await, "sent");
    let (is_sent, pmid): (i64, Option<String>) =
        sqlx::query_as("SELECT is_sent, platform_message_id FROM messages WHERE id = ?")
            .bind(&message_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(is_sent, 1);
    assert!(pmid.unwrap().starts_with("stub-line-"));

    // Conversation last-message/update times advanced (CRD 769).
    let last: Option<String> =
        sqlx::query_scalar("SELECT last_message_at FROM conversations WHERE id = ?")
            .bind(&conv)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(last.is_some());
}

#[tokio::test]
async fn send_message_to_unsupported_platform_ends_failed() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let agent_id: String =
        sqlx::query_scalar("SELECT id FROM agents LIMIT 1").fetch_one(&app.state.db).await.unwrap();
    let cust = app.seed_customer("facebook", "F1", "Bob", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/messages"),
            Some(&token),
            Some(json!({"content": "hello", "senderId": agent_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let message_id = body["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(wait_for_delivery(&app, &message_id).await, "failed");
}

#[tokio::test]
async fn send_message_links_uploaded_attachments() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let agent_id: String =
        sqlx::query_scalar("SELECT id FROM agents LIMIT 1").fetch_one(&app.state.db).await.unwrap();
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let (status, body) =
        upload(&app, &format!("/api/conversations/{conv}/attachments"), &token, "f.txt", b"hi").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let attachment_id = body["data"]["attachmentId"].as_str().unwrap().to_string();

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/messages"),
            Some(&token),
            Some(json!({"senderId": agent_id, "attachmentIds": [attachment_id]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let message_id = body["data"]["id"].as_str().unwrap().to_string();
    let linked: Option<String> =
        sqlx::query_scalar("SELECT message_id FROM attachments WHERE id = ?")
            .bind(&attachment_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(linked, Some(message_id));
}

#[tokio::test]
async fn send_message_validation_errors() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let agent_id: String =
        sqlx::query_scalar("SELECT id FROM agents LIMIT 1").fetch_one(&app.state.db).await.unwrap();
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    // Missing content and no attachments (CRD 772).
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/messages"),
            Some(&token),
            Some(json!({"senderId": agent_id})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Missing sender id.
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/messages"),
            Some(&token),
            Some(json!({"content": "hi"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Missing conversation.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/conversations/ghost/messages",
            Some(&token),
            Some(json!({"content": "hi", "senderId": agent_id})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn send_message_denied_with_role_specific_message() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (token, agent_id) = agent_token(&app, "agent@test.dev", team_a).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, Some(team_b), "assigned").await;
    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/messages"),
            Some(&token),
            Some(json!({"content": "hi", "senderId": agent_id})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("Agents"));
}

// ------------------------------------------------------------ attachment upload

async fn upload(
    app: &TestApp,
    path: &str,
    token: &str,
    filename: &str,
    bytes: &[u8],
) -> (StatusCode, Value) {
    let boundary = "XTESTBOUNDARY";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    let resp = app.router.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn upload_attachment_stores_file_and_record() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    let (status, body) =
        upload(&app, &format!("/api/conversations/{conv}/attachments"), &token, "hello.txt", b"hello").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["filename"], json!("hello.txt"));
    assert_eq!(body["data"]["size"], json!(5));
    let url = body["data"]["url"].as_str().unwrap();
    assert!(url.starts_with("/uploads/"));

    // The stored object exists and the row is unlinked until a send (CRD 783).
    let (message_id, storage_key): (Option<String>, String) =
        sqlx::query_as("SELECT message_id, storage_key FROM attachments WHERE id = ?")
            .bind(body["data"]["attachmentId"].as_str().unwrap())
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(message_id.is_none());
    let stored = std::path::Path::new(&app.state.config.upload_dir).join(&storage_key);
    assert!(stored.exists());
}

#[tokio::test]
async fn upload_attachment_error_conditions() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (agent, _) = agent_token(&app, "agent@test.dev", team_a).await;
    let admin = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;
    let foreign = app.seed_conversation(cust, Some(team_b), "assigned").await;

    // Missing conversation.
    let (status, body) =
        upload(&app, "/api/conversations/ghost/attachments", &admin, "f.txt", b"x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], json!("Conversation not found"));

    // Team-scope gate (CRD 778, 782).
    let (status, body) =
        upload(&app, &format!("/api/conversations/{foreign}/attachments"), &agent, "f.txt", b"x").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], json!("You do not have access to this conversation"));

    // Empty file.
    let (status, body) =
        upload(&app, &format!("/api/conversations/{conv}/attachments"), &admin, "f.txt", b"").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("No file provided"));

    // Over the 10 MB cap.
    let big = vec![b'x'; 10 * 1024 * 1024 + 1];
    let (status, body) =
        upload(&app, &format!("/api/conversations/{conv}/attachments"), &admin, "big.bin", &big).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("File too large (max 10MB)"));
}

// -------------------------------------------------------------------- bulk ops

#[tokio::test]
async fn bulk_assign_and_set_priority() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Support").await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let c1 = app.seed_conversation(cust, None, "active").await;
    let c2 = app.seed_conversation(cust, None, "active").await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/conversations/bulk",
            Some(&token),
            Some(json!({"operation": "assign", "conversationIds": [c1, c2], "data": {"teamId": team}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"], json!("Bulk assign completed successfully"));
    assert_eq!(body["data"]["affectedCount"], json!(2));
    let (team_id, st): (Option<i64>, String) =
        sqlx::query_as("SELECT team_id, status FROM conversations WHERE id = ?")
            .bind(&c1)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(team_id, Some(team));
    assert_eq!(st, "assigned");

    let (status, _, _) = app
        .request(
            "POST",
            "/api/conversations/bulk",
            Some(&token),
            Some(json!({"operation": "set_priority", "conversationIds": [c1], "data": {"priority": "high"}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let priority: String = sqlx::query_scalar("SELECT priority FROM conversations WHERE id = ?")
        .bind(&c1)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(priority, "high");
}

#[tokio::test]
async fn bulk_tag_operations_are_idempotent() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let admin: String =
        sqlx::query_scalar("SELECT id FROM agents LIMIT 1").fetch_one(&app.state.db).await.unwrap();
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let c1 = app.seed_conversation(cust, None, "active").await;
    let tag = app.seed_tag("vip", &admin).await;

    for _ in 0..2 {
        let (status, _, _) = app
            .request(
                "POST",
                "/api/conversations/bulk",
                Some(&token),
                Some(json!({"operation": "add_tags", "conversationIds": [c1], "data": {"tagIds": [tag]}})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_tags WHERE conversation_id = ?")
            .bind(&c1)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(count, 1);

    let (status, _, _) = app
        .request(
            "POST",
            "/api/conversations/bulk",
            Some(&token),
            Some(json!({"operation": "remove_tags", "conversationIds": [c1], "data": {"tagIds": [tag]}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_tags WHERE conversation_id = ?")
            .bind(&c1)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn bulk_denies_whole_batch_on_any_unauthorized_conversation() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let (token, _) = agent_token(&app, "agent@test.dev", team_a).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let mine = app.seed_conversation(cust, Some(team_a), "assigned").await;
    let other = app.seed_conversation(cust, Some(team_b), "assigned").await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/conversations/bulk",
            Some(&token),
            Some(json!({"operation": "set_priority", "conversationIds": [mine, other], "data": {"priority": "high"}})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("1 conversation"));
}

#[tokio::test]
async fn bulk_validation_errors() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let cust = app.seed_customer("line", "U1", "Alice", None).await;
    let conv = app.seed_conversation(cust, None, "active").await;

    // Empty id list.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/conversations/bulk",
            Some(&token),
            Some(json!({"operation": "assign", "conversationIds": []})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Missing data.teamId.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/conversations/bulk",
            Some(&token),
            Some(json!({"operation": "assign", "conversationIds": [conv]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // close / reopen rejected as no-longer-supported (CRD 792).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/conversations/bulk",
            Some(&token),
            Some(json!({"operation": "close", "conversationIds": [conv]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("no longer supported"));

    // Unknown operation lists the valid ones.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/conversations/bulk",
            Some(&token),
            Some(json!({"operation": "explode", "conversationIds": [conv]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("assign"));
}

// ------------------------------------------------------------------- auth gate

#[tokio::test]
async fn conversations_require_authentication() {
    let app = spawn_app().await;
    let (status, _, _) = app.request("GET", "/api/conversations", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

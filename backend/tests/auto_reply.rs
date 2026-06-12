//! Auto-Reply per CRD §2.5 (lines 1334-1451).

mod common;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use common::{spawn_app, TestApp};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

fn line_sig(body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-line-secret").unwrap();
    mac.update(body.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

async fn post_line(app: &TestApp, body: &str) -> StatusCode {
    use axum::body::Body;
    use tower::ServiceExt;
    let sig = line_sig(body);
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/webhook")
        .header("Content-Type", "application/json")
        .header("x-line-signature", sig)
        .body(Body::from(body.to_string()))
        .unwrap();
    app.router.clone().oneshot(req).await.unwrap().status()
}

fn line_text(user: &str, mid: &str, text: &str) -> String {
    json!({
        "destination": "d",
        "events": [{
            "type": "message", "timestamp": 1,
            "source": {"userId": user},
            "message": {"id": mid, "type": "text", "text": text}
        }]
    })
    .to_string()
}

/// Seed an agent with a primary team and log in.
async fn operator(app: &TestApp) -> (String, i64) {
    let team = app.seed_team("AR Team").await;
    let id = app.seed_agent("ar@test.dev", "pw123456", "agent").await;
    app.add_membership(&id, team, "member", true).await;
    let (token, _, _) = app.login("ar@test.dev", "pw123456").await;
    (token, team)
}

// ---------------------------------------------------------------- rules CRUD

#[tokio::test]
async fn rule_create_validates_inputs() {
    let app = spawn_app().await;
    let (token, _) = operator(&app).await;

    let cases = [
        (json!({"triggerType": "keyword"}), "name"),
        (json!({"name": "  ", "triggerType": "keyword"}), "name"),
        (json!({"name": "r", "triggerType": "frobnicate"}), "trigger"),
        (json!({"name": "r", "triggerType": "keyword",
                "conditions": [{"conditionType": "telepathy", "value": "x"}]}), "condition"),
        (json!({"name": "r", "triggerType": "keyword",
                "actions": [{"actionType": "carrier-pigeon", "content": "x"}]}), "action"),
    ];
    for (body, label) in cases {
        let (status, _, _) = app.request("POST", "/api/auto-reply/rules", Some(&token), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label}");
    }

    // No resolvable team and not global -> 400.
    let lone = app.seed_agent("lone@test.dev", "pw123456", "agent").await;
    let _ = lone;
    let (lone_token, _, _) = app.login("lone@test.dev", "pw123456").await;
    let (status, _, _) = app
        .request("POST", "/api/auto-reply/rules", Some(&lone_token),
            Some(json!({"name": "r", "triggerType": "keyword"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rule_crud_round_trip_with_scope_and_priority_order() {
    let app = spawn_app().await;
    let (token, team) = operator(&app).await;

    let (status, body, _) = app
        .request("POST", "/api/auto-reply/rules", Some(&token),
            Some(json!({
                "name": "Price keyword",
                "triggerType": "keyword",
                "priority": 10,
                "conditions": [{"conditionType": "contains", "value": "price"}],
                "actions": [{"actionType": "text", "content": json!({"text": "Our prices start at $10"}).to_string()}],
            })))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let rule = &body["data"];
    assert_eq!(rule["teamId"], team);
    assert_eq!(rule["priority"], 10);
    assert_eq!(rule["isActive"], true);
    assert_eq!(rule["allowPushFallback"], false);
    assert_eq!(rule["conditions"][0]["conditionType"], "contains");
    assert_eq!(rule["actions"][0]["sortOrder"], 0);
    let rule_id = rule["id"].as_i64().unwrap();

    // Global rule via scope=global; listed separately from team rules.
    let (status, gbody, _) = app
        .request("POST", "/api/auto-reply/rules?scope=global", Some(&token),
            Some(json!({"name": "Global fallback", "triggerType": "fallback", "priority": 5,
                        "actions": [{"actionType": "text", "content": json!({"text": "We'll get back to you"}).to_string()}]})))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(gbody["data"]["teamId"].is_null());

    let (_, list, _) = app.request("GET", "/api/auto-reply/rules", Some(&token), None).await;
    let items = list["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "team listing excludes global rules");
    let (_, glist, _) = app
        .request("GET", "/api/auto-reply/rules?scope=global", Some(&token), None)
        .await;
    assert_eq!(glist["data"]["items"].as_array().unwrap().len(), 1);

    // Priority ascending ordering.
    app.request("POST", "/api/auto-reply/rules", Some(&token),
        Some(json!({"name": "Lower prio", "triggerType": "fallback", "priority": 200}))).await;
    let (_, list, _) = app.request("GET", "/api/auto-reply/rules", Some(&token), None).await;
    let items = list["data"]["items"].as_array().unwrap();
    assert_eq!(items[0]["priority"], 10);
    assert_eq!(items[1]["priority"], 200);

    // Partial update + wholesale condition replace (CRD 1364).
    let (status, upd, _) = app
        .request("PUT", &format!("/api/auto-reply/rules/{rule_id}"), Some(&token),
            Some(json!({"priority": 7, "conditions": []})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(upd["data"]["priority"], 7);
    assert_eq!(upd["data"]["conditions"].as_array().unwrap().len(), 0, "empty array clears all");
    assert_eq!(upd["data"]["name"], "Price keyword", "absent fields unchanged");

    let (status, _, _) = app
        .request("PUT", "/api/auto-reply/rules/abc", Some(&token), Some(json!({"priority": 1})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("PUT", "/api/auto-reply/rules/99999", Some(&token), Some(json!({"priority": 1})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Soft delete: echoes id, second delete 404, excluded from listing.
    let (status, del, _) = app
        .request("DELETE", &format!("/api/auto-reply/rules/{rule_id}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(del["data"]["id"], rule_id);
    let (status, _, _) = app
        .request("DELETE", &format!("/api/auto-reply/rules/{rule_id}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, list, _) = app.request("GET", "/api/auto-reply/rules", Some(&token), None).await;
    assert!(list["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["id"] != rule_id));
    let still_there: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM auto_reply_rules WHERE id = ?")
            .bind(rule_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(still_there, 1, "soft delete retains the row");
}

// ---------------------------------------------------------------- schedules

#[tokio::test]
async fn schedules_validate_and_replace_wholesale() {
    let app = spawn_app().await;
    let (token, team) = operator(&app).await;

    let bad = [
        (json!({}), "missing schedules"),
        (json!({"schedules": []}), "empty schedules"),
        (json!({"schedules": [{"dayOfWeek": 7, "startTime": "09:00", "endTime": "18:00"}]}), "day 7"),
        (json!({"schedules": [{"dayOfWeek": 1, "startTime": "25:00", "endTime": "18:00"}]}), "hour 25"),
        (json!({"schedules": [{"dayOfWeek": 1, "startTime": "09:00", "endTime": "9pm"}]}), "format"),
    ];
    for (body, label) in bad {
        let (status, _, _) = app
            .request("POST", "/api/auto-reply/schedules", Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label}");
    }

    let (status, body, _) = app
        .request("POST", "/api/auto-reply/schedules", Some(&token),
            Some(json!({"timezone": "Asia/Taipei", "schedules": [
                {"dayOfWeek": 1, "startTime": "09:00", "endTime": "18:00"},
                {"dayOfWeek": 2, "startTime": "09:00", "endTime": "18:00"},
            ]})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert_eq!(body["data"][0]["teamId"], team);
    assert_eq!(body["data"][0]["timezone"], "Asia/Taipei");

    // Wholesale replace: a day not in the new payload becomes absent.
    let (_, body, _) = app
        .request("POST", "/api/auto-reply/schedules", Some(&token),
            Some(json!({"schedules": [{"dayOfWeek": 5, "startTime": "10:00", "endTime": "16:00"}]})))
        .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["dayOfWeek"], 5);

    let (status, body, _) = app.request("GET", "/api/auto-reply/schedules", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------- health

#[tokio::test]
async fn health_endpoints_report_healthy() {
    let app = spawn_app().await;
    let (token, _) = operator(&app).await;
    for path in [
        "/api/auto-reply/rules/health",
        "/api/auto-reply/schedules/health",
        "/api/auto-reply/logs/health",
    ] {
        let (status, body, _) = app.request("GET", path, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body["data"]["status"], "healthy");
    }
}

// ------------------------------------------------------- engine via webhooks

/// Route a platform user to the operator's team so webhook conversations are
/// team-assigned and team rules apply.
async fn route_user(app: &TestApp, user: &str, team: i64) {
    sqlx::query(
        "INSERT INTO customer_team_assignments (id, platform_user_id, team_id, source, assigned_at)
         VALUES (?, ?, ?, 'manual', ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(user)
    .bind(team)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();
}

#[tokio::test]
async fn keyword_rule_fires_once_per_platform_message() {
    let app = spawn_app().await;
    let (token, team) = operator(&app).await;
    route_user(&app, "U-kw", team).await;

    app.request("POST", "/api/auto-reply/rules", Some(&token),
        Some(json!({
            "name": "Price", "triggerType": "keyword",
            "conditions": [{"conditionType": "contains", "value": "price"}],
            "actions": [{"actionType": "text", "content": json!({"text": "From $10"}).to_string()}],
        })))
        .await;

    // Case-insensitive contains match.
    assert_eq!(post_line(&app, &line_text("U-kw", "kw-1", "What is the PRICE?")).await, StatusCode::OK);

    let (status, method, sent_at): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, delivery_method, sent_at FROM auto_reply_deliveries
         WHERE platform = 'line' AND platform_message_id = 'kw-1'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(status, "success");
    assert_eq!(method.as_deref(), Some("push"));
    assert!(sent_at.is_some());

    let replies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE sender_type = 'system' AND content = 'From $10'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(replies, 1, "system-authored reply stored");

    let logs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auto_reply_logs")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(logs, 1, "audit log written");

    // Webhook redelivery: at-most-once successful auto-reply (CRD 1422).
    assert_eq!(post_line(&app, &line_text("U-kw", "kw-1", "What is the PRICE?")).await, StatusCode::OK);
    let replies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE sender_type = 'system'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(replies, 1, "no second send on redelivery");
    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auto_reply_deliveries WHERE platform_message_id = 'kw-1'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn first_match_wins_and_keyword_without_conditions_never_matches() {
    let app = spawn_app().await;
    let (token, team) = operator(&app).await;
    route_user(&app, "U-fm", team).await;

    // Conditionless keyword rule at top priority can never match (CRD 1416);
    // the fallback rule below it catches everything.
    app.request("POST", "/api/auto-reply/rules", Some(&token),
        Some(json!({"name": "Empty kw", "triggerType": "keyword", "priority": 1,
                    "actions": [{"actionType": "text", "content": json!({"text": "kw"}).to_string()}]})))
        .await;
    app.request("POST", "/api/auto-reply/rules", Some(&token),
        Some(json!({"name": "Catch-all", "triggerType": "fallback", "priority": 50,
                    "actions": [{"actionType": "text", "content": json!({"text": "fallback reply"}).to_string()}]})))
        .await;

    post_line(&app, &line_text("U-fm", "fm-1", "anything at all")).await;
    let content: String = sqlx::query_scalar(
        "SELECT content FROM messages WHERE sender_type = 'system' LIMIT 1",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(content, "fallback reply");

    let (matched, _rule): (String, Option<i64>) = sqlx::query_as(
        "SELECT matched_condition, rule_id FROM auto_reply_logs LIMIT 1",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert!(matched.contains("fallback"), "non-keyword matches record the trigger: {matched}");
}

#[tokio::test]
async fn off_hours_rule_respects_business_hours_and_absent_schedule() {
    let app = spawn_app().await;
    let (token, team) = operator(&app).await;
    route_user(&app, "U-oh", team).await;

    app.request("POST", "/api/auto-reply/rules", Some(&token),
        Some(json!({"name": "OOO", "triggerType": "off_hours",
                    "actions": [{"actionType": "text", "content": json!({"text": "We are closed"}).to_string()}]})))
        .await;

    // No schedule at all: always within hours, off-hours rules never fire (CRD 1417).
    post_line(&app, &line_text("U-oh", "oh-1", "hello")).await;
    let replies: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE sender_type = 'system'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(replies, 0, "no schedule -> off-hours suppressed");

    // A one-minute window at 00:00 leaves the rest of the day outside hours.
    // (Use UTC so "now" is deterministic relative to the stored window.)
    app.request("POST", "/api/auto-reply/schedules", Some(&token),
        Some(json!({"timezone": "UTC", "schedules": (0..7).map(|d| json!({
            "dayOfWeek": d, "startTime": "00:00", "endTime": "00:01"
        })).collect::<Vec<_>>()})))
        .await;
    // Outside the window for almost the entire day; tolerate the one minute.
    let now = chrono::Utc::now().format("%H:%M").to_string();
    post_line(&app, &line_text("U-oh", "oh-2", "hello again")).await;
    let replies: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE sender_type = 'system'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    if now != "00:00" {
        assert_eq!(replies, 1, "outside business hours -> off-hours rule fires");
    }

    // Full-day windows put us inside hours: rule suppressed again.
    app.request("POST", "/api/auto-reply/schedules", Some(&token),
        Some(json!({"timezone": "UTC", "schedules": (0..7).map(|d| json!({
            "dayOfWeek": d, "startTime": "00:00", "endTime": "23:59"
        })).collect::<Vec<_>>()})))
        .await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE sender_type = 'system'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    post_line(&app, &line_text("U-oh", "oh-3", "third")).await;
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE sender_type = 'system'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(before, after, "within business hours -> off-hours rule suppressed");
}

#[tokio::test]
async fn welcome_rule_replaces_default_follow_greeting() {
    let app = spawn_app().await;
    let (token, team) = operator(&app).await;
    route_user(&app, "U-wel", team).await;

    app.request("POST", "/api/auto-reply/rules", Some(&token),
        Some(json!({"name": "Greet", "triggerType": "welcome",
                    "actions": [{"actionType": "text", "content": json!({"text": "Welcome aboard!"}).to_string()}]})))
        .await;

    let follow = json!({
        "destination": "d",
        "events": [{"type": "follow", "timestamp": 1, "source": {"userId": "U-wel"}}]
    })
    .to_string();
    assert_eq!(post_line(&app, &follow).await, StatusCode::OK);

    let contents: Vec<(String,)> =
        sqlx::query_as("SELECT content FROM messages WHERE sender_type = 'system'")
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    assert_eq!(contents.len(), 1, "rule reply replaces the default welcome");
    assert_eq!(contents[0].0, "Welcome aboard!");

    let matched: String = sqlx::query_scalar("SELECT matched_condition FROM auto_reply_logs LIMIT 1")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(matched.contains("welcome"), "{matched}");
}

#[tokio::test]
async fn welcome_rules_do_not_fire_on_ordinary_messages() {
    let app = spawn_app().await;
    let (token, team) = operator(&app).await;
    route_user(&app, "U-nw", team).await;
    app.request("POST", "/api/auto-reply/rules", Some(&token),
        Some(json!({"name": "Greet", "triggerType": "welcome", "priority": 1,
                    "actions": [{"actionType": "text", "content": json!({"text": "hi"}).to_string()}]})))
        .await;
    post_line(&app, &line_text("U-nw", "nw-1", "ordinary text")).await;
    let replies: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE sender_type = 'system'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(replies, 0, "greeting rules fire only via the follow path (CRD 1415)");
}

// ---------------------------------------------------------------- audit logs

#[tokio::test]
async fn logs_listing_filters_and_validates() {
    let app = spawn_app().await;
    let (token, team) = operator(&app).await;
    route_user(&app, "U-log", team).await;
    app.request("POST", "/api/auto-reply/rules", Some(&token),
        Some(json!({"name": "CA", "triggerType": "fallback",
                    "actions": [{"actionType": "text", "content": json!({"text": "ok"}).to_string()}]})))
        .await;
    post_line(&app, &line_text("U-log", "log-1", "ping")).await;

    let (status, body, _) = app.request("GET", "/api/auto-reply/logs", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"];
    assert_eq!(data["total"], 1);
    assert_eq!(data["todayCount"], 1);
    let item = &data["items"][0];
    assert_eq!(item["ruleName"], "CA");
    assert_eq!(item["platform"], "line");
    assert_eq!(item["triggerContent"], "ping");

    // Validation errors.
    let (status, _, _) = app
        .request("GET", "/api/auto-reply/logs?ruleId=abc", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("GET", "/api/auto-reply/logs?platform=telegram", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("GET", "/api/auto-reply/logs?dateFrom=not-a-date", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Platform filter and a deleted rule's logs still display its name.
    let rule_id: i64 = sqlx::query_scalar("SELECT id FROM auto_reply_rules LIMIT 1")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    app.request("DELETE", &format!("/api/auto-reply/rules/{rule_id}"), Some(&token), None).await;
    let (_, body, _) = app
        .request("GET", "/api/auto-reply/logs?platform=line", Some(&token), None)
        .await;
    assert_eq!(body["data"]["items"][0]["ruleName"], "CA", "left-join keeps deleted rule name");
}

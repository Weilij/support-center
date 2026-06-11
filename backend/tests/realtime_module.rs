//! Realtime module & latest-message cache tests (CRD §5.5 lines 3974-4226):
//! the `/api/realtime` management/monitoring surface, the programmatic
//! event-publishing contract, and the latest-message cache guarantees.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::ws::{mint, serve, wait_for_event, ws_connect, Ws};
use common::{spawn_app, TestApp};
use mcss_backend::middleware::auth::AuthUser;
use mcss_backend::realtime::{latest, module};
use mcss_backend::state::TeamMembership;
use serde_json::json;

struct Seeded {
    admin_token: String,
    agent_token: String,
    lead_token: String,
}

async fn seed_agent_with_id(app: &TestApp, id: &str, email: &str, role: &str) {
    sqlx::query(
        "INSERT INTO agents (id, email, password_hash, display_name, role, is_active, created_at)
         VALUES (?, ?, 'x', ?, ?, 1, ?)",
    )
    .bind(id)
    .bind(email)
    .bind(format!("{role} {id}"))
    .bind(role)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();
}

async fn seed(app: &TestApp) -> Seeded {
    let admin_id = app.seed_agent("admin@rtm.io", "Secret123!", "admin").await;
    let agent_id = app.seed_agent("agent@rtm.io", "Secret123!", "agent").await;
    let lead_id = app.seed_agent("lead@rtm.io", "Secret123!", "agent").await;
    let team_id = app.seed_team("RTM Team").await;
    app.add_membership(&agent_id, team_id, "member", true).await;
    app.add_membership(&lead_id, team_id, "lead", true).await;
    Seeded {
        admin_token: mint(&admin_id, "admin", 3600),
        agent_token: mint(&agent_id, "agent", 3600),
        lead_token: mint(&lead_id, "agent", 3600),
    }
}

fn auth_user(id: &str, role: &str, team_role: Option<(i64, &str)>) -> AuthUser {
    AuthUser {
        id: id.to_string(),
        email: format!("{id}@rtm.io"),
        display_name: id.to_string(),
        role: role.to_string(),
        primary_team_id: team_role.map(|(t, _)| t),
        teams: team_role
            .map(|(team_id, role)| {
                vec![TeamMembership { team_id, role: role.to_string(), is_primary: true }]
            })
            .unwrap_or_default(),
        jti: None,
        token_type: "access".into(),
        context_team_id: None,
    }
}

/// Personal-channel session subscribed to a conversation: the audience for
/// conversation-routed events.
async fn subscriber(app: &TestApp, addr: std::net::SocketAddr, conv: &str) -> Ws {
    seed_agent_with_id(app, "9001", "sub9001@rtm.io", "agent").await;
    let token = mint("9001", "agent", 3600);
    let mut ws = ws_connect(
        addr,
        &format!("/api/realtime/session/websocket?token={token}&role=agent&userId=9001"),
    )
    .await
    .unwrap();
    wait_for_event(&mut ws, "user_connected").await;
    app.state.realtime.subscribe("9001", conv);
    ws
}

// ------------------------------------------ lightweight endpoints (CRD 3984-3999)

#[tokio::test]
async fn typing_and_broadcast_acknowledge_with_validation() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Bearer auth is required on the whole surface (CRD 3981).
    let (status, _, _) = app.request("POST", "/api/realtime/typing", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Missing conversation id -> 400 (CRD 3990).
    let (status, body, _) = app
        .request("POST", "/api/realtime/typing", Some(&s.agent_token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Conversation ID is required");

    // Numeric or string conversation ids are both accepted (CRD 3986).
    for conv in [json!(12), json!("conv-x")] {
        let (status, body, _) = app
            .request(
                "POST",
                "/api/realtime/typing",
                Some(&s.agent_token),
                Some(json!({ "conversationId": conv })),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], json!(true));
        assert!(body["message"].is_string());
    }

    // Broadcast requires both fields (CRD 3997).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcast",
            Some(&s.agent_token),
            Some(json!({ "conversationId": 5 })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Conversation ID and event are required");
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcast",
            Some(&s.agent_token),
            Some(json!({ "conversationId": 5, "event": { "type": "x" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn conversation_status_and_online_status_acknowledge() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Static informational response with a server timestamp (CRD block
    // "Get conversation real-time status").
    let (status, body, _) = app
        .request("GET", "/api/realtime/conversation/77/status", Some(&s.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["message"].is_string());
    assert!(body["data"]["timestamp"].is_string());

    // Presence acknowledgement echoes the supplied flag.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/online-status",
            Some(&s.agent_token),
            Some(json!({ "isOnline": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["isOnline"], json!(false));
}

// --------------------------------------------- configuration (CRD 4000-4017)

#[tokio::test]
async fn config_is_admin_only_and_merges_runtime_scoped() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Non-admin -> unauthorized "Admin access required" (CRD 4001).
    for (method, body) in [("GET", None), ("PUT", Some(json!({})))] {
        let (status, resp, _) = app
            .request(method, "/api/realtime/config", Some(&s.lead_token), body)
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(resp["error"], "Admin access required");
    }

    // Full configuration shape (CRD 4003).
    let (status, body, _) =
        app.request("GET", "/api/realtime/config", Some(&s.admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let cfg = &body["data"];
    for key in [
        "deliveryVersion",
        "eventDrivenProcessing",
        "queueProcessing",
        "heartbeatInterval",
        "connectionTimeout",
        "maxRetries",
        "eventStorageTtl",
    ] {
        assert!(!cfg[key].is_null(), "missing config key {key}");
    }

    // Partial update merges over the current configuration (CRD 4008).
    let (status, body, _) = app
        .request(
            "PUT",
            "/api/realtime/config",
            Some(&s.admin_token),
            Some(json!({ "deliveryVersion": "current", "maxRetries": 7 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    let (_, body, _) =
        app.request("GET", "/api/realtime/config", Some(&s.admin_token), None).await;
    assert_eq!(body["data"]["deliveryVersion"], "current");
    assert_eq!(body["data"]["maxRetries"], json!(7));
    assert_eq!(body["data"]["queueProcessing"], json!(true)); // untouched
}

#[tokio::test]
async fn stats_and_health_role_tiers() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Plain member -> "Insufficient permissions" (CRD 4014).
    let (status, body, _) =
        app.request("GET", "/api/realtime/stats", Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Insufficient permissions");

    // Elevated/team role admitted (CRD 4014).
    let (status, body, _) =
        app.request("GET", "/api/realtime/stats", Some(&s.lead_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["currentConfig"]["deliveryVersion"].is_string());
    assert!(body["data"]["timestamp"].is_string());

    // Health is open to any authenticated caller (CRD 4020).
    let (status, body, _) =
        app.request("GET", "/api/realtime/health", Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let d = &body["data"];
    assert_eq!(d["status"], "healthy");
    assert!(d["deliveryVersion"].is_string());
    assert!(d["eventDrivenProcessing"].is_boolean());
    assert!(d["queueProcessing"].is_boolean());
    assert!(d["timestamp"].is_string());
}

// ----------------------------------------------- monitoring (CRD 4026-4060)

#[tokio::test]
async fn monitoring_dashboard_metrics_and_version_info() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Monitoring reads require the elevated tier (CRD 4027, 4226).
    for path in [
        "/api/realtime/monitoring/dashboard",
        "/api/realtime/monitoring/metrics",
        "/api/realtime/monitoring/alerts",
        "/api/realtime/monitoring/health",
        "/api/realtime/monitoring/config",
    ] {
        let (status, body, _) = app.request("GET", path, Some(&s.agent_token), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(body["error"], "Insufficient permissions", "{path}");
    }

    // Dashboard blocks (CRD 4029): aggregate connection counters report zero.
    let (status, body, _) = app
        .request("GET", "/api/realtime/monitoring/dashboard", Some(&s.lead_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let d = &body["data"];
    assert_eq!(d["service"]["status"], "running");
    assert!(d["service"]["uptime"].is_number());
    assert_eq!(d["connections"]["total"], json!(0));
    assert!(d["events"]["totalEvents"].is_number());
    assert!(d["events"]["byType"].is_object());
    assert!(d["latestMetrics"].is_null()); // no points collected yet
    assert_eq!(d["capabilities"]["realtimeChannel"], json!(true));

    // Metrics: latest point plus bounded history (CRD 4034).
    let (_, body, _) = app
        .request("GET", "/api/realtime/monitoring/metrics?limit=1", Some(&s.admin_token), None)
        .await;
    let d = &body["data"];
    assert!(d["latest"]["connections"].is_object());
    assert!(d["latest"]["eventProcessing"].is_object());
    assert!(d["latest"]["queue"].is_object());
    assert!(d["latest"]["resources"].is_object());
    assert!(d["latest"]["timestamp"].is_string());
    assert!(d["latest"]["collectionPeriod"].is_number());
    assert_eq!(d["history"].as_array().unwrap().len(), 1);
    assert_eq!(d["totalPoints"], json!(1));

    // Version info (CRD 4058).
    let (_, body, _) = app
        .request("GET", "/api/realtime/monitoring/config", Some(&s.lead_token), None)
        .await;
    let d = &body["data"];
    assert!(d["currentVersion"].is_string());
    assert!(d["availableVersions"].is_array());
    assert!(d["capabilities"].is_object());
    assert!(d["recommendations"].is_object());
    // POST is accepted at the same path (CRD 4057).
    let (status, _, _) = app
        .request("POST", "/api/realtime/monitoring/config", Some(&s.lead_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn monitoring_alerts_listing_and_resolution() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    let a1 = app.state.realtime.module.raise_alert("warning", "errorRate", 0.05, 0.2, "high error rate");
    let _a2 = app.state.realtime.module.raise_alert("critical", "latency", 1000.0, 5000.0, "slow");

    // Listing with summary (CRD 4040-4042).
    let (status, body, _) = app
        .request("GET", "/api/realtime/monitoring/alerts", Some(&s.lead_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let d = &body["data"];
    assert_eq!(d["alerts"].as_array().unwrap().len(), 2);
    assert_eq!(d["summary"]["total"], json!(2));
    assert_eq!(d["summary"]["byLevel"]["warning"], json!(1));
    assert_eq!(d["summary"]["last24Hours"], json!(2));
    let alert = &d["alerts"][0];
    for key in ["id", "level", "metric", "threshold", "currentValue", "message", "timestamp", "resolved"] {
        assert!(!alert[key].is_null(), "alert missing {key}");
    }

    // Missing id -> 400 (CRD 4048).
    let (status, body, _) = app
        .request("POST", "/api/realtime/monitoring/alerts", Some(&s.lead_token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Alert ID is required");

    // Unknown id -> 404 (CRD 4048).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/monitoring/alerts",
            Some(&s.lead_token),
            Some(json!({ "alertId": "nope" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Alert not found or already resolved");

    // Resolve once (CRD 4047), then the repeat answers 404 (already resolved).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/monitoring/alerts",
            Some(&s.lead_token),
            Some(json!({ "alertId": a1 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"], json!({ "alertId": a1, "resolved": true }));
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/monitoring/alerts",
            Some(&s.lead_token),
            Some(json!({ "alertId": a1 })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // active=true filters resolved alerts out (CRD 4039).
    let (_, body, _) = app
        .request("GET", "/api/realtime/monitoring/alerts?active=true", Some(&s.lead_token), None)
        .await;
    let alerts = body["data"]["alerts"].as_array().unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["metric"], "latency");
}

#[tokio::test]
async fn monitoring_health_reports_dependency_checks() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    let (status, body, _) = app
        .request("GET", "/api/realtime/monitoring/health", Some(&s.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let d = &body["data"];
    assert_eq!(d["checks"]["database"]["status"], "healthy");
    assert!(d["checks"]["database"]["responseTime"].is_number());
    assert_eq!(d["checks"]["kv"]["status"], "healthy");
    assert_eq!(d["checks"]["queue"]["status"], "healthy");
    // The legacy streaming transport has been removed and always reports
    // down (CRD 4054, 4189), so one dependency down -> degraded (CRD 4208).
    assert_eq!(d["checks"]["legacyStreaming"]["status"], "down");
    assert_eq!(d["status"], "degraded");
}

// ----------------------------- programmatic publishing (CRD 4068-4127)

#[tokio::test]
async fn publish_message_event_validates_and_routes_to_the_conversation() {
    let app = spawn_app().await;
    seed(&app).await;
    let addr = serve(&app).await;
    let caller = auth_user("caller", "agent", None);

    // Validation failure -> "Invalid message event data" (CRD 4074).
    for bad in [
        json!({}),
        json!({ "messageId": "not-num", "conversationId": 1, "content": "x", "messageType": "text", "senderType": "agent" }),
        json!({ "messageId": 1, "conversationId": 1, "content": "x", "messageType": "video", "senderType": "agent" }),
        json!({ "messageId": 1, "conversationId": 1, "content": "x", "messageType": "text", "senderType": "bot" }),
    ] {
        let err = module::publish_message_event(&app.state, &caller, &bad).unwrap_err();
        assert_eq!(err.to_string(), "Invalid message event data");
    }

    let mut ws = subscriber(&app, addr, "41").await;
    let payload = json!({
        "messageId": 10, "conversationId": 41, "content": "hi",
        "messageType": "text", "senderType": "customer",
    });
    let out = module::publish_message_event(&app.state, &caller, &payload).unwrap();
    assert!(out["eventId"].is_string());
    assert!(out["processingTime"].is_number());
    let ev = wait_for_event(&mut ws, "new_message").await;
    assert_eq!(ev["payload"]["content"], "hi");

    // Statistics recorded — a successful or failed metric per attempt
    // (CRD 4073, 4120): 4 failed validations above plus 1 success.
    let elevated = auth_user("lead", "agent", Some((1, "lead")));
    let stats = module::event_stats(&app.state, &elevated).unwrap();
    assert_eq!(stats["byType"]["new_message"], json!(5));
    assert_eq!(stats["succeeded"], json!(1));
    assert_eq!(stats["failed"], json!(4));
}

#[tokio::test]
async fn publish_typing_event_excludes_the_typing_user() {
    let app = spawn_app().await;
    seed(&app).await;
    let addr = serve(&app).await;
    let caller = auth_user("caller", "agent", None);

    let err = module::publish_typing_event(&app.state, &caller, &json!({ "conversationId": 1 }))
        .unwrap_err();
    assert_eq!(err.to_string(), "Invalid typing event data");

    // The typing user (9001) holds the subscribed session: excluded from
    // delivery (CRD 4080).
    let mut ws = subscriber(&app, addr, "55").await;
    module::publish_typing_event(
        &app.state,
        &caller,
        &json!({ "conversationId": 55, "userId": 9001, "username": "u", "isTyping": true }),
    )
    .unwrap();
    // A follow-up from someone else is delivered; the excluded event never
    // arrives first.
    module::publish_typing_event(
        &app.state,
        &caller,
        &json!({ "conversationId": 55, "userId": 7, "username": "v", "isTyping": false }),
    )
    .unwrap();
    let ev = wait_for_event(&mut ws, "typing_stopped").await;
    assert_eq!(ev["payload"]["userId"], json!(7));
}

#[tokio::test]
async fn publish_status_assignment_notification_and_system_events() {
    let app = spawn_app().await;
    seed(&app).await;
    let addr = serve(&app).await;
    let member = auth_user("member", "agent", None);
    let elevated = auth_user("lead", "agent", Some((1, "lead")));
    let admin = auth_user("root", "admin", None);

    // Status change: validation + normal-priority routing (CRD 4083-4088).
    let err = module::publish_status_change(&app.state, &member, &json!({ "conversationId": 3 }))
        .unwrap_err();
    assert_eq!(err.to_string(), "Invalid status event data");
    let mut ws = subscriber(&app, addr, "61").await;
    module::publish_status_change(
        &app.state,
        &member,
        &json!({ "conversationId": 61, "previousStatus": "open", "newStatus": "closed", "changedBy": 4 }),
    )
    .unwrap();
    let ev = wait_for_event(&mut ws, "status_changed").await;
    assert_eq!(ev["payload"]["newStatus"], "closed");

    // Assignment change requires the elevated tier (CRD 4104).
    let payload = json!({
        "conversationId": 61,
        "assignedTo": { "type": "user", "id": 9001, "name": "Sub" },
        "previousAssignee": { "type": "user", "id": 7, "name": "Prev" },
        "assignedBy": 4,
    });
    let err = module::publish_assignment_change(&app.state, &member, &payload).unwrap_err();
    assert_eq!(err.to_string(), "Insufficient permissions");
    let err = module::publish_assignment_change(
        &app.state,
        &elevated,
        &json!({ "conversationId": 61, "assignedBy": 4, "assignedTo": { "type": "robot", "id": 1, "name": "x" } }),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Invalid assignment event data");
    module::publish_assignment_change(&app.state, &elevated, &payload).unwrap();
    // The new assignee (a user target, CRD 4106) is notified directly.
    let ev = wait_for_event(&mut ws, "assignment_changed").await;
    assert_eq!(ev["payload"]["assignedTo"]["id"], json!(9001));

    // Notification publishing: elevated only, non-empty numeric targets
    // (CRD 4111-4115).
    let err = module::publish_notification(&app.state, &member, &json!({})).unwrap_err();
    assert_eq!(err.to_string(), "Insufficient permissions");
    let err = module::publish_notification(
        &app.state,
        &elevated,
        &json!({ "notificationId": 1, "type": "t", "title": "T", "content": "c", "targetUsers": [] }),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Invalid notification event data");
    let out = module::publish_notification(
        &app.state,
        &elevated,
        &json!({ "notificationId": 1, "type": "t", "title": "T", "content": "c", "targetUsers": [9001] }),
    )
    .unwrap();
    assert_eq!(out["targetCount"], json!(1));
    let ev = wait_for_event(&mut ws, "notification").await;
    assert_eq!(ev["payload"]["title"], "T");

    // System broadcast: admin only; severity drives priority (CRD 4119-4122).
    let err = module::publish_system_event(
        &app.state,
        &elevated,
        &json!({ "type": "info", "message": "m", "severity": "low" }),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Admin access required");
    let err = module::publish_system_event(
        &app.state,
        &admin,
        &json!({ "type": "party", "message": "m", "severity": "low" }),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Invalid system event data");
    module::publish_system_event(
        &app.state,
        &admin,
        &json!({ "type": "maintenance", "message": "down at 5", "severity": "critical" }),
    )
    .unwrap();
    let ev = wait_for_event(&mut ws, "system_announcement").await;
    assert_eq!(ev["payload"]["message"], "down at 5");

    // Stats reflect priorities; reset is admin-only (CRD 4121-4125).
    let stats = module::event_stats(&app.state, &elevated).unwrap();
    assert_eq!(stats["byPriority"]["urgent"], json!(1));
    assert!(stats["cacheRefreshes"].is_object());
    let err = module::event_stats(&app.state, &member).unwrap_err();
    assert_eq!(err.to_string(), "Insufficient permissions");
    let err = module::reset_event_stats(&app.state, &elevated).unwrap_err();
    assert_eq!(err.to_string(), "Admin access required");
    module::reset_event_stats(&app.state, &admin).unwrap();
    let stats = module::event_stats(&app.state, &elevated).unwrap();
    assert_eq!(stats["totalEvents"], json!(0));
}

// ------------------------------------- latest-message cache (CRD 4129-4174)

async fn seed_conv_with_messages(app: &TestApp, n: usize) -> (String, Vec<String>) {
    let customer = app.seed_customer("line", &format!("U-{}", uuid()), "C", None).await;
    let conv = app.seed_conversation(customer, None, "active").await;
    let mut ids = Vec::new();
    for i in 0..n {
        let at = format!("2026-06-01T10:0{i}:00.000Z");
        ids.push(app.seed_message(&conv, "customer", &format!("msg {i}"), Some(&at)).await);
    }
    (conv, ids)
}

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tokio::test]
async fn latest_message_read_through_and_batch_reads() {
    let app = spawn_app().await;
    let (conv, ids) = seed_conv_with_messages(&app, 3).await;
    let (empty_conv, _) = seed_conv_with_messages(&app, 0).await;

    // Cache miss derives from storage, stores and returns (CRD 4137).
    assert!(app.state.realtime.latest.peek(&conv).is_none());
    let snapshot = latest::get_latest(&app.state, &conv).await.unwrap();
    assert_eq!(snapshot["messageId"], json!(ids[2])); // single most recent
    assert_eq!(snapshot["content"], "msg 2");
    assert_eq!(snapshot["senderType"], "customer");
    assert!(snapshot["cachedAt"].is_string());
    assert!(app.state.realtime.latest.peek(&conv).is_some()); // populated

    // No messages -> absence (CRD 4137).
    assert!(latest::get_latest(&app.state, &empty_conv).await.is_none());

    // Batch read omits conversations with no messages (CRD 4144).
    let map = latest::get_latest_many(
        &app.state,
        &[conv.clone(), empty_conv.clone(), "ghost".into()],
    )
    .await;
    assert_eq!(map.len(), 1);
    assert_eq!(map[&conv]["messageId"], json!(ids[2]));

    // Invalidation removes the snapshot; the next read repopulates from
    // authoritative data (CRD 4156, 4172).
    app.state.realtime.latest.invalidate(&conv);
    assert!(app.state.realtime.latest.peek(&conv).is_none());
    assert!(latest::get_latest(&app.state, &conv).await.is_some());
}

#[tokio::test]
async fn refresh_emits_latest_message_updated_to_subscribers() {
    let app = spawn_app().await;
    seed(&app).await;
    let addr = serve(&app).await;
    let (conv, _) = seed_conv_with_messages(&app, 2).await;
    let mut ws = subscriber(&app, addr, &conv).await;

    assert!(latest::refresh(&app.state, &conv).await);
    // Exact frame shape (CRD 4180-4182): top-level type/conversationId/data/
    // timestamp — not the hub's framed envelope.
    let ev = wait_for_event(&mut ws, "latest_message_updated").await;
    assert_eq!(ev["conversationId"], json!(conv));
    assert_eq!(ev["data"]["content"], "msg 1");
    assert!(ev["data"]["createdAt"].is_string());
    assert_eq!(ev["data"]["senderType"], "customer");
    assert!(ev["timestamp"].is_string());
    assert!(ev.get("payload").is_none());
    assert!(app.state.realtime.latest.peek(&conv).is_some());

    // A refresh with no resulting message completes silently (CRD 4205).
    let (empty_conv, _) = seed_conv_with_messages(&app, 0).await;
    assert!(latest::refresh(&app.state, &empty_conv).await);
    assert!(app.state.realtime.latest.peek(&empty_conv).is_none());
}

#[tokio::test]
async fn schedule_refresh_coalesces_duplicates() {
    let app = spawn_app().await;
    let (conv, ids) = seed_conv_with_messages(&app, 1).await;

    // Two requests in the same tick coalesce into one refresh (CRD 4185).
    latest::schedule_refresh(app.state.clone(), conv.clone());
    latest::schedule_refresh(app.state.clone(), conv.clone());
    let mut done = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (processed, succeeded, failed) = app.state.realtime.latest.refresh_counters();
        if processed == 1 {
            assert_eq!((succeeded, failed), (1, 0));
            done = true;
            break;
        }
    }
    assert!(done, "refresh did not complete");
    let snapshot = app.state.realtime.latest.peek(&conv).unwrap();
    assert_eq!(snapshot["messageId"], json!(ids[0]));
}

#[tokio::test]
async fn warm_up_populates_most_recently_active_conversations() {
    let app = spawn_app().await;
    let (conv_a, _) = seed_conv_with_messages(&app, 1).await;
    let (conv_b, _) = seed_conv_with_messages(&app, 2).await;
    let (conv_empty, _) = seed_conv_with_messages(&app, 0).await;

    // Warming covers up to the limit (default 50, CRD 4160-4163); empty
    // conversations contribute nothing.
    let warmed = latest::warm_up(&app.state, None).await;
    assert_eq!(warmed, 2);
    assert!(app.state.realtime.latest.peek(&conv_a).is_some());
    assert!(app.state.realtime.latest.peek(&conv_b).is_some());
    assert!(app.state.realtime.latest.peek(&conv_empty).is_none());

    // Explicit limit bounds the candidate set.
    let warmed = latest::warm_up(&app.state, Some(1)).await;
    assert!(warmed <= 1);
}

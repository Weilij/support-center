//! Collaboration tests (CRD §3.4 lines 2321-2446): conversation viewer
//! presence, typing indicators, availability presence, statistics, cleanup,
//! health, and the live events emitted into the conversation's room.

mod common;

use axum::http::StatusCode;
use common::ws::{mint, serve, wait_for_event, ws_connect, Ws};
use common::{spawn_app, TestApp};
use serde_json::{json, Value};

struct Seeded {
    /// Agent with a numeric identifier (listable as a viewer).
    num_token: String,
    /// Agent with a UUID identifier (omitted from viewer listings).
    uuid_id: String,
    uuid_token: String,
    admin_token: String,
}

async fn seed_agent_with_id(app: &TestApp, id: &str, email: &str, role: &str) {
    sqlx::query(
        "INSERT INTO agents (id, email, password_hash, display_name, role, is_active, created_at)
         VALUES (?, ?, 'x', ?, ?, 1, ?)",
    )
    .bind(id)
    .bind(email)
    .bind(format!("Agent {id}"))
    .bind(role)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();
}

async fn seed(app: &TestApp) -> Seeded {
    seed_agent_with_id(app, "101", "num@collab.io", "agent").await;
    let uuid_id = app.seed_agent("uuid@collab.io", "Secret123!", "agent").await;
    let admin_id = app.seed_agent("admin@collab.io", "Secret123!", "admin").await;
    Seeded {
        num_token: mint("101", "agent", 3600),
        uuid_token: mint(&uuid_id, "agent", 3600),
        uuid_id,
        admin_token: mint(&admin_id, "admin", 3600),
    }
}

/// Personal-channel session subscribed to the collaboration conversation:
/// the audience for the room events (CRD 2436-2443).
async fn room_listener(app: &TestApp, addr: std::net::SocketAddr, conv: &str) -> Ws {
    seed_agent_with_id(app, "9001", "listener@collab.io", "agent").await;
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

// --------------------------------------------------- auth & validation gates

#[tokio::test]
async fn all_routes_require_authentication() {
    let app = spawn_app().await;
    for (method, path) in [
        ("GET", "/api/collaboration/conversations/1/state"),
        ("GET", "/api/collaboration/conversations/1/viewers"),
        ("POST", "/api/collaboration/conversations/1/join"),
        ("POST", "/api/collaboration/conversations/1/leave"),
        ("POST", "/api/collaboration/typing"),
        ("POST", "/api/collaboration/presence"),
        ("GET", "/api/collaboration/stats"),
        ("POST", "/api/collaboration/cleanup"),
        ("GET", "/api/collaboration/health"),
    ] {
        let (status, _, _) = app.request(method, path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn conversation_id_must_be_a_positive_integer() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    for (method, path) in [
        ("GET", "/api/collaboration/conversations/abc/state"),
        ("GET", "/api/collaboration/conversations/0/viewers"),
        ("POST", "/api/collaboration/conversations/-3/join"),
        ("POST", "/api/collaboration/conversations/1.5/leave"),
    ] {
        let (status, body, _) = app.request(method, path, Some(&s.num_token), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {path}");
        assert_eq!(body["success"], json!(false));
    }
}

#[tokio::test]
async fn unsupported_protocol_yields_machine_coded_error() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    // websocket (or none) is the only effective transport (CRD 2332, 2340).
    let (status, body, _) = app
        .request(
            "GET",
            "/api/collaboration/conversations/1/state?protocol=carrier-pigeon",
            Some(&s.num_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "PROTOCOL_NOT_SUPPORTED");
    let (status, _, _) = app
        .request(
            "GET",
            "/api/collaboration/conversations/1/state?protocol=websocket",
            Some(&s.num_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

// ----------------------------------------- join / viewers / state / leave

#[tokio::test]
async fn join_view_state_and_leave_lifecycle_with_events() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut listener = room_listener(&app, addr, "88").await;

    // Empty/degraded snapshot rather than an error before anyone joins
    // (CRD 2335).
    let (status, body, _) = app
        .request("GET", "/api/collaboration/conversations/88/state", Some(&s.num_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["viewers"], json!([]));
    assert_eq!(body["data"]["typingUsers"], json!([]));
    assert_eq!(body["data"]["protocol"], "websocket");

    // Join: identity comes from the session, body optional (CRD 2360).
    let (status, body, _) = app
        .request("POST", "/api/collaboration/conversations/88/join", Some(&s.num_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"], Value::Null);
    // A user-joined event reaches the room's other participants (CRD 2364,
    // 2437).
    let ev = wait_for_event(&mut listener, "user_joined").await;
    assert_eq!(ev["payload"]["userId"], "101");

    // Viewer listing is normalized (CRD 2349): numeric user id, username and
    // display label fallbacks, role, transport, typing flag, timestamps.
    let (_, body, _) = app
        .request("GET", "/api/collaboration/conversations/88/viewers", Some(&s.num_token), None)
        .await;
    let viewers = body["data"]["viewers"].as_array().unwrap();
    assert_eq!(viewers.len(), 1);
    let v = &viewers[0];
    assert_eq!(v["userId"], json!(101.0));
    assert_eq!(v["username"], "num@collab.io");
    assert_eq!(v["displayName"], "Agent 101");
    assert_eq!(v["role"], "agent");
    assert_eq!(v["protocol"], "websocket");
    assert_eq!(v["isTyping"], json!(false));
    assert!(v["joinedAt"].is_string());
    assert!(v["lastActivity"].is_string());

    // A viewer whose identifier is not a finite number is omitted from
    // listings (CRD 2352) while still occupying the room.
    let (status, _, _) = app
        .request("POST", "/api/collaboration/conversations/88/join", Some(&s.uuid_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let ev = wait_for_event(&mut listener, "user_joined").await;
    assert_eq!(ev["payload"]["userId"], json!(s.uuid_id));
    let (_, body, _) = app
        .request("GET", "/api/collaboration/conversations/88/viewers", Some(&s.num_token), None)
        .await;
    assert_eq!(body["data"]["viewers"].as_array().unwrap().len(), 1);

    // State snapshot aggregates viewers/typing/connection count (CRD 2336).
    let (_, body, _) = app
        .request("GET", "/api/collaboration/conversations/88/state", Some(&s.num_token), None)
        .await;
    let d = &body["data"];
    assert_eq!(d["conversationId"], json!(88));
    assert_eq!(d["viewers"].as_array().unwrap().len(), 1);
    assert!(d["connectionCount"].is_number());
    assert!(d["metadata"]["recentMessageCount"].is_number());

    // Leave: inverse of join, idempotent (CRD 2369-2374).
    for _ in 0..2 {
        let (status, body, _) = app
            .request("POST", "/api/collaboration/conversations/88/leave", Some(&s.num_token), None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], json!(true));
        let ev = wait_for_event(&mut listener, "user_left").await;
        assert_eq!(ev["payload"]["userId"], "101");
    }
    let (_, body, _) = app
        .request("GET", "/api/collaboration/conversations/88/viewers", Some(&s.num_token), None)
        .await;
    assert_eq!(body["data"]["viewers"], json!([]));
}

#[tokio::test]
async fn join_rejects_when_the_room_is_full() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    // Fill the room to its fifty-viewer capacity (CRD 2367).
    for i in 0..50 {
        app.state
            .realtime
            .collab
            .join(99, &format!("filler-{i}"), "f", "F", "agent")
            .ok()
            .unwrap();
    }
    let (status, body, _) = app
        .request("POST", "/api/collaboration/conversations/99/join", Some(&s.num_token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "ROOM_FULL");
    assert_eq!(body["success"], json!(false));
}

// --------------------------------------------------------- typing indicator

#[tokio::test]
async fn typing_indicator_contract_and_events() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut listener = room_listener(&app, addr, "70").await;

    // Missing fields -> 400 (CRD 2381).
    for body in [json!({}), json!({ "conversationId": 70 }), json!({ "status": "start" })] {
        let (status, resp, _) = app
            .request("POST", "/api/collaboration/typing", Some(&s.num_token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(resp["error"].as_str().unwrap().contains("Missing required fields"));
    }
    // Invalid status -> 400 (CRD 2381).
    let (status, resp, _) = app
        .request(
            "POST",
            "/api/collaboration/typing",
            Some(&s.num_token),
            Some(json!({ "conversationId": 70, "status": "pause" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(resp["error"].as_str().unwrap().contains("Invalid status"));

    // Start: indicator visible in state with expiry metadata; typing_start
    // event to others (CRD 2379-2380, 2438). Numeric-string ids accepted.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/collaboration/typing",
            Some(&s.num_token),
            Some(json!({ "conversationId": "70", "status": "start" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["message"].as_str().unwrap().contains("start"));
    let ev = wait_for_event(&mut listener, "typing_start").await;
    assert_eq!(ev["payload"]["userId"], "101");
    let (_, body, _) = app
        .request("GET", "/api/collaboration/conversations/70/state", Some(&s.num_token), None)
        .await;
    let typing = body["data"]["typingUsers"].as_array().unwrap();
    assert_eq!(typing.len(), 1);
    assert_eq!(typing[0]["userId"], "101");
    assert_eq!(typing[0]["conversationId"], json!(70));
    assert!(typing[0]["startedAt"].is_string());
    assert!(typing[0]["expiresAt"].is_string());

    // Stop cancels the indicator (CRD 2382, 2418).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/collaboration/typing",
            Some(&s.num_token),
            Some(json!({ "conversationId": 70, "status": "stop" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let ev = wait_for_event(&mut listener, "typing_stop").await;
    assert_eq!(ev["payload"]["userId"], "101");
    let (_, body, _) = app
        .request("GET", "/api/collaboration/conversations/70/state", Some(&s.num_token), None)
        .await;
    assert_eq!(body["data"]["typingUsers"], json!([]));
}

// ----------------------------------------------------- presence / availability

#[tokio::test]
async fn presence_contract_and_room_notification() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut listener = room_listener(&app, addr, "44").await;

    // Missing status -> 400 (CRD 2389).
    let (status, body, _) = app
        .request("POST", "/api/collaboration/presence", Some(&s.num_token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("status"));

    // Status outside the allowed set -> 400 listing the values (CRD 2389).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/collaboration/presence",
            Some(&s.num_token),
            Some(json!({ "status": "sleeping" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("online, away, busy, offline"));

    // Plain update without a focused conversation: recorded, no room event
    // (CRD 2386).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/collaboration/presence",
            Some(&s.num_token),
            Some(json!({ "status": "online" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    let p = app.state.realtime.collab.presence_snapshot("101").unwrap();
    assert_eq!(p["status"], "online");

    // With a focused conversation: presence_update into that room
    // (CRD 2388, 2441); metadata retained.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/collaboration/presence",
            Some(&s.num_token),
            Some(json!({ "status": "busy", "currentConversation": "44", "metadata": { "note": "call" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let ev = wait_for_event(&mut listener, "presence_update").await;
    assert_eq!(ev["payload"]["userId"], "101");
    assert_eq!(ev["payload"]["status"], "busy");
    let p = app.state.realtime.collab.presence_snapshot("101").unwrap();
    assert_eq!(p["status"], "busy");
    assert_eq!(p["currentConversation"], json!(44));
    assert_eq!(p["metadata"]["note"], "call");
}

// ----------------------------------------------------------------- statistics

#[tokio::test]
async fn stats_aggregate_and_rank_active_conversations() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Two viewers in room 1, one in rooms 2-7 (ranking capped to a small
    // top-N, CRD 2404).
    app.state.realtime.collab.join(1, "201", "a", "A", "agent").ok().unwrap();
    app.state.realtime.collab.join(1, "202", "b", "B", "agent").ok().unwrap();
    for room in 2..=7 {
        app.state.realtime.collab.join(room, "203", "c", "C", "agent").ok().unwrap();
    }
    app.state.realtime.collab.set_typing(1, "201", "a", "A", "agent", true);

    let (status, body, _) =
        app.request("GET", "/api/collaboration/stats", Some(&s.num_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let d = &body["data"];
    assert_eq!(d["totalViewers"], json!(8));
    assert_eq!(d["totalTyping"], json!(1));
    assert_eq!(d["totalRooms"], json!(7));
    assert!(d["connectionsByProtocol"]["websocket"].is_number());
    let ranked = d["mostActiveConversations"].as_array().unwrap();
    assert_eq!(ranked.len(), 5); // capped top-N
    assert_eq!(ranked[0], json!({ "conversationId": 1, "viewerCount": 2 }));

    // Unsupported explicit transport (CRD 2405).
    let (status, body, _) = app
        .request("GET", "/api/collaboration/stats?protocol=sse", Some(&s.num_token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "PROTOCOL_NOT_SUPPORTED");
}

// ------------------------------------------------------------ cleanup & health

#[tokio::test]
async fn cleanup_is_admin_only_and_reports_removed_count() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Authenticated non-administrator -> 403 (CRD 2428).
    let (status, body, _) =
        app.request("POST", "/api/collaboration/cleanup", Some(&s.num_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "Insufficient permissions");

    // Administrator: safe to invoke repeatedly (CRD 2429).
    for _ in 0..2 {
        let (status, body, _) =
            app.request("POST", "/api/collaboration/cleanup", Some(&s.admin_token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"]["cleanedCount"].is_number());
    }
}

#[tokio::test]
async fn health_reports_lazy_initialization() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Cold path: not yet initialized, with an explanatory note (CRD 2433);
    // the probe itself never initializes (CRD 2435).
    for _ in 0..2 {
        let (status, body, _) =
            app.request("GET", "/api/collaboration/health", Some(&s.num_token), None).await;
        assert_eq!(status, StatusCode::OK);
        let d = &body["data"];
        assert_eq!(d["status"], "not_initialized");
        assert!(d["note"].is_string());
        assert_eq!(d["config"]["defaultProtocol"], "websocket");
        assert_eq!(d["config"]["websocketEnabled"], json!(true));
        assert_eq!(d["availableProtocols"], json!(["websocket"]));
        assert!(d["timestamp"].is_string());
    }

    // First business request initializes lazily (CRD 2391, 2421).
    let (status, _, _) = app
        .request("POST", "/api/collaboration/conversations/5/join", Some(&s.num_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body, _) =
        app.request("GET", "/api/collaboration/health", Some(&s.num_token), None).await;
    assert_eq!(body["data"]["status"], "healthy");
    assert!(body["data"]["note"].is_null());
}

//! WebSocket gateway & realtime hub tests (CRD §5.1, §1.3 WS gate).

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use mcss_backend::domain::auth::tokens::{self, Claims};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Serve the app on a real port; HTTP assertions still go through the shared
/// router (same AppState/hub), WS connections through the bound listener.
async fn serve(app: &TestApp) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

fn mint(sub: &str, role: &str, ttl_secs: i64) -> String {
    let claims = Claims::new(sub, role, "access", ttl_secs);
    tokens::sign(&claims, "test-secret").unwrap()
}

async fn ws_connect(addr: SocketAddr, query: &str) -> Result<Ws, WsError> {
    let url = format!("ws://{addr}/api/websocket/connect{query}");
    tokio_tungstenite::connect_async(url).await.map(|(ws, _)| ws)
}

/// Expect the handshake to be rejected; return (status, close-code header, body).
async fn connect_rejected(addr: SocketAddr, query: &str) -> (u16, Option<String>, Value) {
    match ws_connect(addr, query).await {
        Err(WsError::Http(resp)) => {
            let status = resp.status().as_u16();
            let close_code = resp
                .headers()
                .get("x-websocket-close-code")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let body = resp
                .body()
                .as_ref()
                .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                .unwrap_or(Value::Null);
            (status, close_code, body)
        }
        Ok(_) => panic!("handshake unexpectedly accepted for {query}"),
        Err(e) => panic!("unexpected websocket error: {e}"),
    }
}

async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("stream ended")
            .expect("websocket error");
        if let Message::Text(t) = msg {
            return serde_json::from_str(t.as_str()).expect("frame is not JSON");
        }
    }
}

async fn wait_for_event(ws: &mut Ws, event: &str) -> Value {
    for _ in 0..20 {
        let v = next_json(ws).await;
        if v["type"] == event {
            return v;
        }
    }
    panic!("did not receive event {event}");
}

async fn send_json(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

struct Seeded {
    admin_id: String,
    admin_token: String,
    agent_id: String,
    agent_token: String,
    team_id: i64,
    /// Conversation assigned to `team_id`.
    team_conv: String,
    /// Conversation assigned to `other_team_id` (agent has no access).
    foreign_conv: String,
    /// Unassigned (shared pool) conversation.
    pool_conv: String,
}

async fn seed(app: &TestApp) -> Seeded {
    let admin_id = app.seed_agent("admin@rt.io", "Secret123!", "admin").await;
    let agent_id = app.seed_agent("agent@rt.io", "Secret123!", "agent").await;
    let team_id = app.seed_team("Realtime Team").await;
    let other_team_id = app.seed_team("Other Team").await;
    app.add_membership(&agent_id, team_id, "member", true).await;
    let customer = app.seed_customer("line", "U-rt-1", "RT Customer", Some(team_id)).await;
    let team_conv = app.seed_conversation(customer, Some(team_id), "assigned").await;
    let foreign_conv = app.seed_conversation(customer, Some(other_team_id), "assigned").await;
    let pool_conv = app.seed_conversation(customer, None, "active").await;
    let (admin_token, _, _) = app.login("admin@rt.io", "Secret123!").await;
    let (agent_token, _, _) = app.login("agent@rt.io", "Secret123!").await;
    let _ = other_team_id;
    Seeded {
        admin_id,
        admin_token,
        agent_id,
        agent_token,
        team_id,
        team_conv,
        foreign_conv,
        pool_conv,
    }
}

// ------------------------------------------------------ handshake gate (CRD §1.3)

#[tokio::test]
async fn connect_rejects_missing_token() {
    let app = spawn_app().await;
    let addr = serve(&app).await;
    let (status, close, body) = connect_rejected(addr, "").await;
    assert_eq!(status, 401);
    assert_eq!(close.as_deref(), Some("4401"));
    assert_eq!(body["code"], 4401);
    assert_eq!(body["error"], "NO_TOKEN");
    assert!(body["timestamp"].is_string());
    assert!(body["action"].is_string());
}

#[tokio::test]
async fn connect_rejects_malformed_token() {
    let app = spawn_app().await;
    let addr = serve(&app).await;
    let (status, close, body) = connect_rejected(addr, "?token=not-a-jwt").await;
    assert_eq!(status, 401);
    assert_eq!(close.as_deref(), Some("4402"));
    assert_eq!(body["error"], "INVALID_TOKEN_FORMAT");
}

#[tokio::test]
async fn connect_rejects_invalid_signature() {
    let app = spawn_app().await;
    let addr = serve(&app).await;
    let claims = Claims::new("someone", "agent", "access", 3600);
    let forged = tokens::sign(&claims, "wrong-secret").unwrap();
    let (status, _, body) = connect_rejected(addr, &format!("?token={forged}")).await;
    assert_eq!(status, 401);
    assert_eq!(body["code"], 4403);
    assert_eq!(body["error"], "INVALID_TOKEN");
}

#[tokio::test]
async fn connect_rejects_expired_token() {
    let app = spawn_app().await;
    let addr = serve(&app).await;
    let token = mint("someone", "agent", -3600);
    let (status, close, body) = connect_rejected(addr, &format!("?token={token}")).await;
    assert_eq!(status, 401);
    assert_eq!(close.as_deref(), Some("4404"));
    assert_eq!(body["error"], "TOKEN_EXPIRED");
    assert!(body["expiredAt"].is_number());
    assert!(body["currentTime"].is_number());
}

#[tokio::test]
async fn connect_rejects_token_expiring_within_margin() {
    let app = spawn_app().await;
    let addr = serve(&app).await;
    let token = mint("someone", "agent", 10);
    let (status, _, body) = connect_rejected(addr, &format!("?token={token}")).await;
    assert_eq!(status, 401);
    assert_eq!(body["code"], 4405);
    assert_eq!(body["error"], "TOKEN_EXPIRING_SOON");
    assert!(body["secondsRemaining"].is_number());
}

#[tokio::test]
async fn connect_rejects_disallowed_role() {
    let app = spawn_app().await;
    let addr = serve(&app).await;
    let token = mint("someone", "customer", 3600);
    let (status, _, body) = connect_rejected(addr, &format!("?token={token}")).await;
    assert_eq!(status, 401);
    assert_eq!(body["code"], 4407);
    assert_eq!(body["error"], "INVALID_ROLE");
    assert!(body["allowedRoles"].is_array());
}

#[tokio::test]
async fn connect_rejects_unknown_or_zero_account() {
    let app = spawn_app().await;
    let addr = serve(&app).await;
    // Zero account identifier (CRD 618).
    let token = mint("0", "agent", 3600);
    let (status, _, body) = connect_rejected(addr, &format!("?token={token}")).await;
    assert_eq!(status, 401);
    assert_eq!(body["error"], "INVALID_USER_DATA");
    // Unknown account.
    let token = mint("no-such-user", "agent", 3600);
    let (_, _, body) = connect_rejected(addr, &format!("?token={token}")).await;
    assert_eq!(body["code"], 4406);
}

#[tokio::test]
async fn agent_is_denied_foreign_conversation_but_allowed_pool() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    // Foreign team conversation -> 403, code 4403, CONVERSATION_ACCESS_DENIED.
    let (status, close, body) = connect_rejected(
        addr,
        &format!("?token={}&conversationId={}", s.agent_token, s.foreign_conv),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(close.as_deref(), Some("4403"));
    assert_eq!(body["error"], "CONVERSATION_ACCESS_DENIED");
    // Unassigned shared-pool conversation is allowed (CRD 3240).
    let mut ws = ws_connect(
        addr,
        &format!("?token={}&conversationId={}", s.agent_token, s.pool_conv),
    )
    .await
    .expect("pool conversation should be accessible");
    let welcome = wait_for_event(&mut ws, "connection_established").await;
    assert_eq!(welcome["payload"]["conversationId"], json!(s.pool_conv));
}

// ----------------------------------------------------- protocol (CRD 3410-3419)

#[tokio::test]
async fn personal_channel_welcome_ping_pong_and_unknown_type() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = ws_connect(addr, &format!("?token={}", s.admin_token)).await.unwrap();

    // Personal-channel welcome (CRD 3442).
    let welcome = wait_for_event(&mut ws, "user_connected").await;
    assert_eq!(welcome["payload"]["userId"], json!(s.admin_id));
    assert!(welcome["payload"]["connectionId"].is_string());
    assert!(welcome["payload"]["subscriptions"].is_array());
    assert!(welcome["payload"]["preferences"].is_object());

    // Keepalive (CRD 3412).
    send_json(&mut ws, json!({ "type": "ping", "timestamp": "t-1" })).await;
    let pong = wait_for_event(&mut ws, "pong").await;
    assert_eq!(pong["payload"]["echo"], "t-1");

    // Unknown frame type (CRD 3417).
    send_json(&mut ws, json!({ "type": "bogus" })).await;
    let err = wait_for_event(&mut ws, "error").await;
    assert_eq!(err["payload"]["message"], "Unknown message type: bogus");

    // Unparseable frame (CRD 3411).
    ws.send(Message::Text("not json".into())).await.unwrap();
    let err = wait_for_event(&mut ws, "error").await;
    assert_eq!(err["payload"]["message"], "Invalid message format");
}

#[tokio::test]
async fn room_welcome_and_join_leave_events() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    let mut admin_ws = ws_connect(
        addr,
        &format!("?token={}&conversationId={}", s.admin_token, s.team_conv),
    )
    .await
    .unwrap();
    let welcome = wait_for_event(&mut admin_ws, "connection_established").await;
    assert_eq!(welcome["payload"]["conversationId"], json!(s.team_conv));
    assert_eq!(welcome["payload"]["roomMode"], "full");
    assert!(welcome["payload"]["participants"]
        .as_array()
        .unwrap()
        .contains(&json!(s.admin_id)));
    // user_joined is broadcast to all room connections, joiner included
    // (CRD 3497).
    let joined = wait_for_event(&mut admin_ws, "user_joined").await;
    assert_eq!(joined["payload"]["userId"], json!(s.admin_id));

    // Second participant joins; the first connection observes it.
    let mut agent_ws = ws_connect(
        addr,
        &format!("?token={}&conversationId={}", s.agent_token, s.team_conv),
    )
    .await
    .unwrap();
    let joined = wait_for_event(&mut admin_ws, "user_joined").await;
    assert_eq!(joined["payload"]["userId"], json!(s.agent_id));
    assert_eq!(joined["payload"]["participantCount"], 2);

    // Last-connection departure produces one user_left (CRD 3577).
    agent_ws.close(None).await.unwrap();
    let left = wait_for_event(&mut admin_ws, "user_left").await;
    assert_eq!(left["payload"]["userId"], json!(s.agent_id));
    assert_eq!(left["payload"]["participantCount"], 1);
}

#[tokio::test]
async fn room_chat_broadcast_typing_relay_and_sync() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let q_admin = format!("?token={}&conversationId={}", s.admin_token, s.team_conv);
    let q_agent = format!("?token={}&conversationId={}", s.agent_token, s.team_conv);
    let mut a = ws_connect(addr, &q_admin).await.unwrap();
    wait_for_event(&mut a, "connection_established").await;
    let mut b = ws_connect(addr, &q_agent).await.unwrap();
    wait_for_event(&mut b, "connection_established").await;

    // Chat message: ordered, broadcast to all participants (CRD 3414, 3561).
    send_json(&mut a, json!({ "type": "message", "content": "hello room" })).await;
    let msg = wait_for_event(&mut b, "message_sent").await;
    assert_eq!(msg["payload"]["content"], "hello room");
    assert_eq!(msg["payload"]["senderId"], json!(s.admin_id));
    assert_eq!(msg["payload"]["metadata"]["sequence"], 1);
    // The sender receives its own broadcast too.
    let own = wait_for_event(&mut a, "message_sent").await;
    assert_eq!(own["payload"]["metadata"]["sequence"], 1);

    // Typing events are relayed to the other participants only (CRD 3415).
    send_json(&mut a, json!({ "type": "event", "event": "typing_start" })).await;
    let typing = wait_for_event(&mut b, "typing_start").await;
    assert_eq!(typing["payload"]["userId"], json!(s.admin_id));

    // Reconnection sync returns messages after the supplied time (CRD 3416).
    send_json(
        &mut b,
        json!({ "type": "sync", "since": "1970-01-01T00:00:00.000Z" }),
    )
    .await;
    let sync = wait_for_event(&mut b, "sync_response").await;
    assert_eq!(sync["payload"]["missedCount"], 1);
    assert!(sync["payload"]["lastMessageAt"].is_string());
}

#[tokio::test]
async fn personal_channel_subscriptions_and_chat_ack() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = ws_connect(addr, &format!("?token={}", s.agent_token)).await.unwrap();
    wait_for_event(&mut ws, "user_connected").await;

    // Subscribe is permission-checked (CRD 3413): foreign team denied.
    send_json(&mut ws, json!({ "type": "subscribe", "conversationId": s.foreign_conv })).await;
    let err = wait_for_event(&mut ws, "error").await;
    assert_eq!(
        err["payload"]["message"],
        "Permission denied to subscribe to this conversation"
    );

    // Accessible conversation acknowledged with the subscription count.
    send_json(&mut ws, json!({ "type": "subscribe", "conversationId": s.team_conv })).await;
    let ack = wait_for_event(&mut ws, "subscription_added").await;
    assert_eq!(ack["payload"]["conversationId"], json!(s.team_conv));
    assert_eq!(ack["payload"]["subscriptionCount"], 1);

    // Chat on the personal channel requires a subscribed target (CRD 3414).
    send_json(
        &mut ws,
        json!({ "type": "message", "conversationId": s.team_conv, "messageId": "m-1" }),
    )
    .await;
    let ack = wait_for_event(&mut ws, "message_acknowledged").await;
    assert_eq!(ack["payload"]["messageId"], "m-1");
    assert_eq!(ack["payload"]["userId"], json!(s.agent_id));

    // Chat to a non-subscribed conversation is rejected.
    send_json(&mut ws, json!({ "type": "message", "conversationId": s.pool_conv })).await;
    let err = wait_for_event(&mut ws, "error").await;
    assert_eq!(err["payload"]["message"], "Not subscribed to this conversation");

    // Unsubscribe always succeeds and is acknowledged (CRD 3413).
    send_json(&mut ws, json!({ "type": "unsubscribe", "conversationId": s.team_conv })).await;
    let ack = wait_for_event(&mut ws, "subscription_removed").await;
    assert_eq!(ack["payload"]["subscriptionCount"], 0);
}

// ------------------------------------- domain fan-out through the hub (CRD 3448-3461)

#[tokio::test]
async fn http_send_message_broadcasts_to_room_and_team_list_views() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    // Admin watches the conversation room; the team agent's personal channel
    // stands in for the conversation-list view (CRD 3449).
    let mut room_ws = ws_connect(
        addr,
        &format!("?token={}&conversationId={}", s.admin_token, s.team_conv),
    )
    .await
    .unwrap();
    wait_for_event(&mut room_ws, "connection_established").await;
    let mut list_ws = ws_connect(addr, &format!("?token={}", s.agent_token)).await.unwrap();
    wait_for_event(&mut list_ws, "user_connected").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{}/messages", s.team_conv),
            Some(&s.admin_token),
            Some(json!({ "content": "hello realtime", "senderId": s.admin_id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "send failed: {body}");

    let msg = wait_for_event(&mut room_ws, "message_sent").await;
    assert_eq!(msg["payload"]["content"], "hello realtime");
    assert_eq!(msg["payload"]["deliveryStatus"], "pending");
    assert_eq!(msg["payload"]["senderType"], "agent");

    let list = wait_for_event(&mut list_ws, "new_message").await;
    assert_eq!(list["payload"]["conversationId"], json!(s.team_conv));
    assert_eq!(list["payload"]["content"], "hello realtime");
}

#[tokio::test]
async fn conversation_assignment_events_reach_team_members() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = ws_connect(addr, &format!("?token={}", s.agent_token)).await.unwrap();
    wait_for_event(&mut ws, "user_connected").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{}/assign", s.pool_conv),
            Some(&s.admin_token),
            Some(json!({ "teamId": s.team_id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "assign failed: {body}");

    let evt = wait_for_event(&mut ws, "conversation_assigned").await;
    assert_eq!(evt["payload"]["conversationId"], json!(s.pool_conv));
    assert_eq!(evt["payload"]["teamId"], json!(s.team_id));
    assert!(evt["payload"]["assignedBy"]["id"].is_string());
}

#[tokio::test]
async fn conversation_tag_updates_reach_room_subscribers() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let tag = app.seed_tag("rt-tag", &s.admin_id).await;
    let mut ws = ws_connect(
        addr,
        &format!("?token={}&conversationId={}", s.admin_token, s.team_conv),
    )
    .await
    .unwrap();
    wait_for_event(&mut ws, "connection_established").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{}/tags", s.team_conv),
            Some(&s.admin_token),
            Some(json!({ "tagIds": [tag] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "tag add failed: {body}");

    let evt = wait_for_event(&mut ws, "conversation_tags_updated").await;
    assert_eq!(evt["payload"]["operation"], "add");
    assert_eq!(evt["payload"]["tagIds"], json!([tag]));
}

// ----------------------------------------------------- HTTP surface (CRD 3270-3408)

#[tokio::test]
async fn disconnect_endpoint_closes_the_connection() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = ws_connect(addr, &format!("?token={}", s.admin_token)).await.unwrap();
    let welcome = wait_for_event(&mut ws, "user_connected").await;
    let connection_id = welcome["payload"]["connectionId"].as_str().unwrap().to_string();

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/websocket/disconnect?token={}", s.admin_token),
            None,
            Some(json!({ "connectionId": connection_id, "reason": "test" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["connectionId"], json!(connection_id));
    assert!(body["data"]["disconnectedAt"].is_string());

    // The server closes the socket once its hub registration is removed.
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => continue,
                Some(Err(_)) => break,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "socket was not closed after disconnect");
}

#[tokio::test]
async fn disconnect_requires_the_handshake_gate() {
    let app = spawn_app().await;
    let (status, body, _) = app
        .request("POST", "/api/websocket/disconnect", None, Some(json!({"connectionId": "x"})))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 4401);
}

#[tokio::test]
async fn public_health_and_migration_status() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/websocket/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "healthy");
    assert!(body["data"]["totalConnections"].is_number());

    let (status, body, _) =
        app.request("GET", "/api/websocket/migration-status", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], true);
    assert_eq!(body["data"]["rolloutPercentage"], 100);
    assert!(body["data"]["featureFlags"].is_object());
}

#[tokio::test]
async fn authenticated_health_metrics_and_probes() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Comprehensive health when authenticated (CRD 3299-3302).
    let (status, body, _) =
        app.request("GET", "/api/websocket/health", Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["components"]["database"]["status"].is_string());

    // Metrics requires authentication (CRD 3313).
    let (status, _, _) = app.request("GET", "/api/websocket/metrics", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body, _) =
        app.request("GET", "/api/websocket/metrics", Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "ok");
    assert!(body["data"]["data"]["performance"]["latencyP95Ms"].is_number());

    let (status, body, _) =
        app.request("GET", "/api/websocket/readiness", Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["ready"], true);

    let (status, body, _) =
        app.request("GET", "/api/websocket/liveness", Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["alive"], true);
}

#[tokio::test]
async fn migration_config_admin_gate_and_validation() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Non-administrator -> forbidden (CRD 3291).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/websocket/migration-config",
            Some(&s.agent_token),
            Some(json!({ "enabled": false })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Rollout percentage outside 0-100 -> validation failure (CRD 3291).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/websocket/migration-config",
            Some(&s.admin_token),
            Some(json!({ "rolloutPercentage": 150 })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = app
        .request(
            "POST",
            "/api/websocket/migration-config",
            Some(&s.admin_token),
            Some(json!({ "rolloutPercentage": 50, "strategy": "gradual" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["rolloutPercentage"], 50);
    assert_eq!(body["data"]["strategy"], "gradual");
    assert!(body["data"]["updatedBy"].is_string());
}

#[tokio::test]
async fn disabling_the_feature_refuses_new_connections() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let (status, _, _) = app
        .request(
            "POST",
            "/api/websocket/migration-config",
            Some(&s.admin_token),
            Some(json!({ "enabled": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    match ws_connect(addr, &format!("?token={}", s.admin_token)).await {
        Err(WsError::Http(resp)) => assert_eq!(resp.status().as_u16(), 503),
        other => panic!("expected 503 refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn dashboard_endpoints_are_admin_only() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    for path in [
        "/api/websocket/dashboard/metrics",
        "/api/websocket/dashboard/connections",
        "/api/websocket/dashboard/history",
        "/api/websocket/dashboard/trends",
        "/api/websocket/dashboard/durable-objects",
        "/api/websocket/dashboard/alerts",
        "/api/websocket/analytics/dashboard",
    ] {
        let (status, _, _) = app.request("GET", path, Some(&s.agent_token), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "agent allowed on {path}");
        let (status, _, _) = app.request("GET", path, Some(&s.admin_token), None).await;
        assert_eq!(status, StatusCode::OK, "admin denied on {path}");
    }
    let (status, body, _) = app
        .request("GET", "/api/websocket/dashboard/connections", Some(&s.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["count"], 0);
}

#[tokio::test]
async fn analytics_records_validation_and_persistence() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Missing required fields -> bad request (CRD 3371, 3377).
    let (status, _, _) = app
        .request("POST", "/api/websocket/analytics/errors", None, Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/websocket/analytics/quality", None, Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Both record endpoints are unauthenticated trusted-system calls.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/websocket/analytics/errors",
            None,
            Some(json!({
                "timestamp": "2026-06-11T00:00:00.000Z",
                "errorCode": 4401,
                "errorType": "NO_TOKEN",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["errorId"].is_string());

    let (status, _, _) = app
        .request(
            "POST",
            "/api/websocket/analytics/quality",
            None,
            Some(json!({
                "timestamp": "2026-06-11T00:00:00.000Z",
                "userId": s.agent_id,
                "connectionId": "c-1",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Trends window must be 1-168 hours (CRD 3361-3364).
    let (status, _, _) = app
        .request(
            "GET",
            "/api/websocket/analytics/trends?timeRange=500",
            Some(&s.admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = app
        .request("GET", "/api/websocket/analytics/trends", Some(&s.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["timeRangeHours"], 24);
    assert_eq!(body["data"]["errorCount"], 1);
}

#[tokio::test]
async fn analytics_alerts_and_config() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Invalid level -> bad request (CRD 3383).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/websocket/analytics/alerts/trigger",
            Some(&s.admin_token),
            Some(json!({ "level": "loud", "title": "t", "description": "d" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = app
        .request(
            "POST",
            "/api/websocket/analytics/alerts/trigger",
            Some(&s.admin_token),
            Some(json!({ "level": "critical", "title": "High errors", "description": "spike" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["alertId"].is_string());

    let (status, body, _) = app
        .request("GET", "/api/websocket/dashboard/alerts", Some(&s.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["count"], 1);

    // Alert config defaults, then update with range validation (CRD 3389-3397).
    let (status, body, _) = app
        .request("GET", "/api/websocket/analytics/config/alerts", Some(&s.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["isDefault"], true);

    let (status, _, _) = app
        .request(
            "PUT",
            "/api/websocket/analytics/config/alerts",
            Some(&s.admin_token),
            Some(json!({
                "errorRateThreshold": 5.0,
                "latencyThreshold": 100,
                "connectionFailureThreshold": 0.2,
                "satisfactionThreshold": 0.7,
                "timeWindow": 600,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = app
        .request(
            "PUT",
            "/api/websocket/analytics/config/alerts",
            Some(&s.admin_token),
            Some(json!({
                "errorRateThreshold": 0.1,
                "latencyThreshold": 2000,
                "connectionFailureThreshold": 0.2,
                "satisfactionThreshold": 0.7,
                "timeWindow": 600,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["isDefault"], false);

    let (status, body, _) = app
        .request("GET", "/api/websocket/analytics/config/alerts", Some(&s.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["errorRateThreshold"], 0.1);
    assert_eq!(body["data"]["isDefault"], false);
}

#[tokio::test]
async fn test_connection_requires_user_id() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let (status, _, _) = app.request("GET", "/api/websocket/test-connection", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = app
        .request(
            "GET",
            &format!(
                "/api/websocket/test-connection?userId={}&conversationId={}",
                s.agent_id, s.team_conv
            ),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["components"]["userChannel"]["online"], false);
    assert!(body["data"]["components"]["conversationRoom"].is_object());
}

#[tokio::test]
async fn per_user_connection_ceiling_yields_429() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut held = Vec::new();
    for _ in 0..5 {
        let mut ws = ws_connect(addr, &format!("?token={}", s.admin_token)).await.unwrap();
        wait_for_event(&mut ws, "user_connected").await;
        held.push(ws);
    }
    match ws_connect(addr, &format!("?token={}", s.admin_token)).await {
        Err(WsError::Http(resp)) => assert_eq!(resp.status().as_u16(), 429),
        other => panic!("expected 429 at the per-user ceiling, got {other:?}"),
    }
}

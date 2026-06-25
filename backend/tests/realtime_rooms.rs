//! Conversation room & routed broadcast delivery tests (CRD §5.2 lines
//! 3469-3692): room WebSocket auth modes, ordering, reconnection sync,
//! room HTTP surface, and the routed-delivery (broadcaster) endpoints.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::ws::{
    connect_rejected, expect_silence, mint, send_json, serve, wait_for_event, ws_connect,
};
use common::{spawn_app, TestApp};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use mcss_backend::realtime::rooms::challenge_signature;

struct Seeded {
    admin_id: String,
    admin_token: String,
    agent_id: String,
    agent_token: String,
    team_id: i64,
    team_conv: String,
}

async fn seed(app: &TestApp) -> Seeded {
    let admin_id = app.seed_agent("admin@rooms.io", "Secret123!", "admin").await;
    let agent_id = app.seed_agent("agent@rooms.io", "Secret123!", "agent").await;
    let team_id = app.seed_team("Rooms Team").await;
    app.add_membership(&agent_id, team_id, "member", true).await;
    let customer = app.seed_customer("line", "U-rooms-1", "Rooms Customer", Some(team_id)).await;
    let team_conv = app.seed_conversation(customer, Some(team_id), "assigned").await;
    let (admin_token, _, _) = app.login("admin@rooms.io", "Secret123!").await;
    let (agent_token, _, _) = app.login("agent@rooms.io", "Secret123!").await;
    Seeded { admin_id, admin_token, agent_id, agent_token, team_id, team_conv }
}

fn room_ws(conv: &str, query: &str) -> String {
    format!("/api/realtime/rooms/{conv}/websocket{query}")
}

fn event(id: &str, typ: &str, data: Value) -> Value {
    json!({ "id": id, "type": typ, "timestamp": "2026-06-11T00:00:00.000Z", "data": data })
}

// -------------------------------------------------- room websocket (CRD 3478-3509)

#[tokio::test]
async fn room_ws_rejects_missing_or_invalid_auth() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    // Missing both auth methods in full mode -> 400 (CRD 3503).
    let (status, body) = connect_rejected(addr, &room_ws(&s.team_conv, "")).await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("token or challengeId"));

    // Invalid token -> 401 "Invalid token" (CRD 3504).
    let (status, body) =
        connect_rejected(addr, &room_ws(&s.team_conv, "?token=garbage")).await;
    assert_eq!(status, 401);
    assert_eq!(body["error"], "Invalid token");

    // Invalid challenge response -> 401 (CRD 3505).
    let (status, body) = connect_rejected(
        addr,
        &room_ws(&s.team_conv, "?challengeId=nope&signature=bad"),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(body["error"], "Invalid challenge response");
}

#[tokio::test]
async fn room_ws_full_mode_welcome_join_and_leave() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    let mut a = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.admin_token)))
        .await
        .unwrap();
    // Welcome only to the new socket: conversation id, connection id,
    // participants, room mode, last-message timestamp (CRD 3498, 3683).
    let welcome = wait_for_event(&mut a, "connection_established").await;
    assert_eq!(welcome["payload"]["conversationId"], json!(s.team_conv));
    assert_eq!(welcome["payload"]["roomMode"], "full");
    assert!(welcome["payload"]["connectionId"].is_string());
    assert!(welcome["payload"]["participants"].as_array().unwrap().contains(&json!(s.admin_id)));
    assert!(welcome["payload"]["lastMessageAt"].is_null());

    // user_joined goes to everyone including the joiner (CRD 3497, 3684).
    let joined = wait_for_event(&mut a, "user_joined").await;
    assert_eq!(joined["payload"]["userId"], json!(s.admin_id));
    assert_eq!(joined["payload"]["role"], "admin");
    assert_eq!(joined["payload"]["participantCount"], 1);

    let mut b = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.agent_token)))
        .await
        .unwrap();
    wait_for_event(&mut b, "connection_established").await;
    let joined = wait_for_event(&mut a, "user_joined").await;
    assert_eq!(joined["payload"]["userId"], json!(s.agent_id));
    assert_eq!(joined["payload"]["participantCount"], 2);

    // user_left fires once per user departure (CRD 3577, 3685).
    b.close(None).await.unwrap();
    let left = wait_for_event(&mut a, "user_left").await;
    assert_eq!(left["payload"]["userId"], json!(s.agent_id));
    assert_eq!(left["payload"]["participantCount"], 1);
}

#[tokio::test]
async fn room_message_ordering_is_strictly_increasing() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut a = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.admin_token)))
        .await
        .unwrap();
    wait_for_event(&mut a, "connection_established").await;
    let mut b = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.agent_token)))
        .await
        .unwrap();
    wait_for_event(&mut b, "connection_established").await;

    for i in 1..=3 {
        send_json(&mut a, json!({ "type": "message", "content": format!("m{i}") })).await;
    }
    // The receiver observes the canonical order with strictly increasing
    // sequence numbers (CRD 3559, 3567).
    for i in 1..=3u64 {
        let msg = wait_for_event(&mut b, "message_sent").await;
        assert_eq!(msg["payload"]["content"], format!("m{i}"));
        assert_eq!(msg["payload"]["metadata"]["sequence"], i);
        assert_eq!(msg["payload"]["messageType"], "text");
        assert_eq!(msg["payload"]["senderId"], json!(s.admin_id));
    }
}

#[tokio::test]
async fn room_reconnection_sync_recovers_missed_messages() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let q_admin = format!("?token={}", s.admin_token);
    let q_agent = format!("?token={}", s.agent_token);
    let mut a = ws_connect(addr, &room_ws(&s.team_conv, &q_admin)).await.unwrap();
    wait_for_event(&mut a, "connection_established").await;
    let mut b = ws_connect(addr, &room_ws(&s.team_conv, &q_agent)).await.unwrap();
    wait_for_event(&mut b, "connection_established").await;

    send_json(&mut a, json!({ "type": "message", "content": "before-drop" })).await;
    let m1 = wait_for_event(&mut b, "message_sent").await;
    let since = m1["payload"]["timestamp"].as_str().unwrap().to_string();

    // B drops; a message is sent while it is away.
    b.close(None).await.unwrap();
    wait_for_event(&mut a, "user_left").await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    send_json(&mut a, json!({ "type": "message", "content": "while-away" })).await;
    wait_for_event(&mut a, "message_sent").await;

    // B reconnects: the welcome advertises the last-message time and a sync
    // returns exactly the missed message (CRD 3569-3572, 3688).
    let mut b = ws_connect(addr, &room_ws(&s.team_conv, &q_agent)).await.unwrap();
    let welcome = wait_for_event(&mut b, "connection_established").await;
    assert!(welcome["payload"]["lastMessageAt"].is_string());
    send_json(&mut b, json!({ "type": "sync", "since": since })).await;
    let sync = wait_for_event(&mut b, "sync_response").await;
    assert_eq!(sync["payload"]["missedCount"], 1);
    assert_eq!(sync["payload"]["messages"][0]["content"], "while-away");
    assert!(sync["payload"]["syncedAt"].is_string());
    assert!(sync["payload"]["lastMessageAt"].is_string());

    // No "since" -> empty set (CRD 3571).
    send_json(&mut b, json!({ "type": "sync" })).await;
    let sync = wait_for_event(&mut b, "sync_response").await;
    assert_eq!(sync["payload"]["missedCount"], 0);
}

#[tokio::test]
async fn room_full_mode_denies_unrecognized_sender_role() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    // Full mode takes identity and role from the credential (CRD 3488); a
    // non-staff role may connect but is denied chat sends (CRD 3557).
    let customer_token = mint("cust-77", "customer", 3600);
    let mut ws = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={customer_token}")))
        .await
        .unwrap();
    wait_for_event(&mut ws, "connection_established").await;
    send_json(&mut ws, json!({ "type": "message", "content": "nope" })).await;
    let err = wait_for_event(&mut ws, "error").await;
    assert_eq!(err["payload"]["message"], "Permission denied to send messages");
}

#[tokio::test]
async fn room_typing_indicator_relayed_only_and_never_stored() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut a = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.admin_token)))
        .await
        .unwrap();
    wait_for_event(&mut a, "connection_established").await;
    let mut b = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.agent_token)))
        .await
        .unwrap();
    wait_for_event(&mut b, "connection_established").await;
    // Drain A's pending events (its own join, B's join, and B's first-session
    // presence event delivered to administrators) before asserting silence.
    wait_for_event(&mut a, "user_joined").await;
    wait_for_event(&mut a, "user_joined").await;
    wait_for_event(&mut a, "user_connected").await;

    // A typing-flagged chat frame is relayed to others only (CRD 3560, 3687).
    send_json(&mut a, json!({ "type": "message", "messageType": "typing" })).await;
    let typing = wait_for_event(&mut b, "typing").await;
    assert_eq!(typing["payload"]["userId"], json!(s.admin_id));
    expect_silence(&mut a, Duration::from_millis(300)).await;

    // Typing indicators are never stored (CRD 3567): nothing to sync.
    send_json(&mut b, json!({ "type": "sync", "since": "1970-01-01T00:00:00.000Z" })).await;
    let sync = wait_for_event(&mut b, "sync_response").await;
    assert_eq!(sync["payload"]["missedCount"], 0);
}

#[tokio::test]
async fn simplified_room_semantics() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let conv = "simplified-room-1";

    // Missing one of the three required parameters -> 400 (CRD 3506).
    let (status, _) =
        connect_rejected(addr, &room_ws(conv, "?mode=simplified&userId=u1&token=t")).await;
    assert_eq!(status, 400);

    // The three parameters suffice; the role is taken at face value
    // (CRD 3485, 3490) and the welcome reports the simplified mode with a
    // null last-message timestamp (CRD 3498).
    let q = "?mode=simplified&userId=u1&token=anything&role=guest";
    let mut a = ws_connect(addr, &room_ws(conv, q)).await.unwrap();
    let welcome = wait_for_event(&mut a, "connection_established").await;
    assert_eq!(welcome["payload"]["roomMode"], "simplified");
    assert!(welcome["payload"]["lastMessageAt"].is_null());

    let q2 = "?userId=u2&token=anything&role=guest";
    let mut b = ws_connect(addr, &room_ws(conv, q2)).await.unwrap();
    wait_for_event(&mut b, "connection_established").await;
    // Drain A's pending join events before asserting silence.
    wait_for_event(&mut a, "user_joined").await;
    wait_for_event(&mut a, "user_joined").await;

    // Subscribe / unsubscribe / sync / generic events are ignored entirely in
    // simplified mode (CRD 3548, 3550, 3551).
    send_json(&mut a, json!({ "type": "subscribe", "conversationId": conv })).await;
    send_json(&mut a, json!({ "type": "sync", "since": "1970-01-01T00:00:00.000Z" })).await;
    send_json(&mut a, json!({ "type": "event", "event": "custom" })).await;
    expect_silence(&mut a, Duration::from_millis(300)).await;
    // Heartbeat still answered (CRD 3547).
    send_json(&mut a, json!({ "type": "ping", "timestamp": "t-1" })).await;
    let pong = wait_for_event(&mut a, "pong").await;
    assert_eq!(pong["payload"]["echo"], "t-1");

    // Chat needs no staff role in simplified mode (CRD 3557) and still
    // carries the ordering number (CRD 3559).
    send_json(&mut a, json!({ "type": "message", "content": "hi" })).await;
    let msg = wait_for_event(&mut b, "message_sent").await;
    assert_eq!(msg["payload"]["content"], "hi");
    assert_eq!(msg["payload"]["metadata"]["sequence"], 1);

    // Simplified metrics omit history/uptime (CRD 3540) and the challenge
    // route does not exist (CRD 3517).
    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/realtime/rooms/{conv}/metrics"),
            Some(&s.admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["mode"], "simplified");
    assert!(body["data"].get("historyLength").is_none());
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/realtime/rooms/{conv}/challenge"),
            Some(&s.admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn challenge_flow_is_single_use_and_signature_checked() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let path = format!("/api/realtime/rooms/{}/challenge", s.team_conv);

    // Missing/invalid authorization -> 401 (CRD 3517).
    let (status, _, _) = app.request("POST", &path, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Issue a challenge (CRD 3511-3516).
    let (status, body, _) = app.request("POST", &path, Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let challenge_id = body["data"]["challengeId"].as_str().unwrap().to_string();
    assert!(body["data"]["expiresAt"].is_string());
    assert_eq!(body["data"]["ttlMs"], 30_000);

    // A wrong signature is rejected (CRD 3505).
    let (status, body) = connect_rejected(
        addr,
        &room_ws(&s.team_conv, &format!("?challengeId={challenge_id}&signature=forged")),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(body["error"], "Invalid challenge response");

    // The forged attempt consumed the challenge (single-use); issue another
    // and complete the handshake with the keyed signature (CRD 3489, 3518).
    let (_, body, _) = app.request("POST", &path, Some(&s.agent_token), None).await;
    let challenge_id = body["data"]["challengeId"].as_str().unwrap().to_string();
    let signature = challenge_signature("test-secret", &challenge_id, &s.agent_token);
    let mut ws = ws_connect(
        addr,
        &room_ws(&s.team_conv, &format!("?challengeId={challenge_id}&signature={signature}")),
    )
    .await
    .expect("challenge handshake should succeed");
    let welcome = wait_for_event(&mut ws, "connection_established").await;
    assert_eq!(welcome["payload"]["conversationId"], json!(s.team_conv));

    // Replaying the consumed challenge fails (CRD 3518).
    let (status, body) = connect_rejected(
        addr,
        &room_ws(&s.team_conv, &format!("?challengeId={challenge_id}&signature={signature}")),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(body["error"], "Invalid challenge response");
}

#[tokio::test]
async fn room_http_status_participants_metrics_broadcast_and_disconnect() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.admin_token)))
        .await
        .unwrap();
    let welcome = wait_for_event(&mut ws, "connection_established").await;
    let connection_id = welcome["payload"]["connectionId"].as_str().unwrap().to_string();
    send_json(&mut ws, json!({ "type": "message", "content": "one" })).await;
    wait_for_event(&mut ws, "message_sent").await;

    // POST /connect — status, no state change (CRD 3520-3522).
    let base = format!("/api/realtime/rooms/{}", s.team_conv);
    let (status, body, _) =
        app.request("POST", &format!("{base}/connect"), Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["activeConnections"], 1);
    assert_eq!(body["data"]["mode"], "full");

    // POST /participants (CRD 3536-3537).
    let (status, body, _) =
        app.request("POST", &format!("{base}/participants"), Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["participants"], json!([s.admin_id]));
    assert_eq!(body["data"]["activeConnections"], 1);
    assert!(body["data"]["lastActivity"].is_string());

    // POST /metrics (CRD 3539-3540): full mode reports history and uptime.
    let (status, body, _) =
        app.request("POST", &format!("{base}/metrics"), Some(&s.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["mode"], "full");
    assert_eq!(body["data"]["messageSequence"], 1);
    assert_eq!(body["data"]["participantCount"], 1);
    assert_eq!(body["data"]["active"], true);
    assert_eq!(body["data"]["historyLength"], 1);
    assert!(body["data"]["uptimeSeconds"].is_number());

    // POST /broadcast — inject an event to every room connection
    // (CRD 3530-3534); administrator only.
    let (status, _, _) = app
        .request(
            "POST",
            &format!("{base}/broadcast"),
            Some(&s.agent_token),
            Some(json!({ "type": "announcement", "data": { "note": "hello" } })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request(
            "POST",
            &format!("{base}/broadcast"),
            Some(&s.admin_token),
            Some(json!({ "type": "announcement", "data": { "note": "hello" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let evt = wait_for_event(&mut ws, "announcement").await;
    assert_eq!(evt["payload"]["data"]["note"], "hello");
    let fanout: (String, String, String) = sqlx::query_as(
        "SELECT event, targets, options FROM realtime_broadcast_fanout_events
         WHERE source_instance = $1
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(app.state.realtime.instance_id())
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let event: serde_json::Value = serde_json::from_str(&fanout.0).unwrap();
    let targets: serde_json::Value = serde_json::from_str(&fanout.1).unwrap();
    let options: serde_json::Value = serde_json::from_str(&fanout.2).unwrap();
    assert_eq!(event["type"], "announcement");
    assert_eq!(targets[0]["type"], "conversation");
    assert_eq!(targets[0]["ids"][0], s.team_conv);
    assert_eq!(options["priority"], "high");

    // POST /disconnect — removes the connection; unknown ids are a successful
    // no-op (CRD 3524-3528).
    let (status, _, _) = app
        .request(
            "POST",
            &format!("{base}/disconnect"),
            Some(&s.admin_token),
            Some(json!({ "connectionId": "does-not-exist" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app
        .request("POST", &format!("{base}/disconnect"), Some(&s.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request(
            "POST",
            &format!("{base}/disconnect"),
            Some(&s.admin_token),
            Some(json!({ "connectionId": connection_id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
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
    assert!(closed.is_ok(), "socket was not closed after forced disconnect");
}

// ------------------------------------------- routed delivery (CRD 3581-3660)

#[tokio::test]
async fn queue_event_validates_routes_by_priority_and_flushes() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = ws_connect(
        addr,
        &format!("/api/realtime/session/websocket?token={}&role=agent&userId={}", s.agent_token, s.agent_id),
    )
    .await
    .unwrap();
    wait_for_event(&mut ws, "user_connected").await;

    // Malformed event -> 400 "Invalid event format" (CRD 3587).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast",
            Some(&s.admin_token),
            Some(json!({ "event": { "type": "incomplete" } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Invalid event format");

    // Trusted surface: administrators only.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast",
            Some(&s.agent_token),
            Some(json!({ "event": event("e0", "x", json!({})) })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // High priority enters the high-priority queue (normal depth stays 0)
    // and the fast loop delivers it without an explicit flush (CRD 3585, 3692).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast",
            Some(&s.admin_token),
            Some(json!({
                "event": event("e-high", "urgent_alert", json!({ "n": 1 })),
                "targets": [ { "type": "user", "ids": [s.agent_id] } ],
                "options": { "priority": "high" },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["eventId"], "e-high");
    assert!(body["data"]["queuedAt"].is_string());
    assert_eq!(body["data"]["queueSize"], 0);
    let evt = wait_for_event(&mut ws, "urgent_alert").await;
    assert_eq!(evt["payload"]["eventId"], "e-high");

    // Default priority enters the normal queue (depth includes the insert);
    // /queue-event is an identical alias (CRD 3588); /flush-queue forces
    // processing (CRD 3640-3643).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/queue-event",
            Some(&s.admin_token),
            Some(json!({
                "event": event("e-normal", "list_refresh", json!({ "n": 2 })),
                "targets": [ { "type": "user", "ids": [s.agent_id] } ],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["queueSize"].as_u64().unwrap() >= 1);
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/flush-queue",
            Some(&s.admin_token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["remainingEvents"], 0);
    let evt = wait_for_event(&mut ws, "list_refresh").await;
    assert_eq!(evt["payload"]["eventId"], "e-normal");
}

#[tokio::test]
async fn normal_queue_overflow_evicts_oldest_entries() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    use mcss_backend::realtime::broadcaster::NORMAL_QUEUE_CAP;

    // Bulk insert one past the cap under a single lock: the oldest entry is
    // evicted and counted (CRD 3585, 3588, 3678).
    let depth = app.state.realtime.queue.enqueue_batch((0..=NORMAL_QUEUE_CAP).map(|i| {
        (
            event(&format!("ev-{i}"), "bulk", json!({})),
            vec![json!({ "type": "user", "ids": [] })],
            json!({}),
        )
    }));
    assert_eq!(depth, NORMAL_QUEUE_CAP);

    let (status, body, _) = app
        .request("POST", "/api/realtime/broadcaster/metrics", Some(&s.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["evicted"].as_u64().unwrap() >= 1);
    assert_eq!(body["data"]["totalEvents"], NORMAL_QUEUE_CAP + 1);
}

#[tokio::test]
async fn routed_broadcaster_events_relay_across_instances() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut room = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.admin_token)))
        .await
        .unwrap();
    wait_for_event(&mut room, "connection_established").await;

    sqlx::query(
        "INSERT INTO realtime_broadcast_fanout_events
         (id, source_instance, event, targets, options, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind("broadcast-peer-1")
    .bind("peer-instance")
    .bind(event("remote-broadcast-1", "remote_conv_event", json!({ "k": "remote" })).to_string())
    .bind(json!([{ "type": "conversation", "ids": [s.team_conv] }]).to_string())
    .bind(json!({ "priority": "normal" }).to_string())
    .bind(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    .execute(&app.state.db)
    .await
    .unwrap();

    let processed =
        mcss_backend::realtime::broadcaster::process_remote_broadcast_events(&app.state, 10)
            .await
            .unwrap();
    assert_eq!(processed, 1);
    let evt = wait_for_event(&mut room, "remote_conv_event").await;
    assert_eq!(evt["payload"]["k"], "remote");
    assert_eq!(evt["payload"]["eventId"], "remote-broadcast-1");

    let ack: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM realtime_broadcast_fanout_acks
         WHERE event_id = 'broadcast-peer-1' AND instance_id = $1",
    )
    .bind(app.state.realtime.instance_id())
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(ack, 1);
}

#[tokio::test]
async fn queued_broadcaster_events_are_published_for_peer_instances() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/queue-event",
            Some(&s.admin_token),
            Some(json!({
                "event": event("fanout-local-1", "fanout_event", json!({ "k": "local" })),
                "targets": [{ "type": "conversation", "ids": [s.team_conv] }],
                "options": { "priority": "normal" },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let row: (String, String, String) = sqlx::query_as(
        "SELECT event, targets, source_instance
         FROM realtime_broadcast_fanout_events
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(row.2, app.state.realtime.instance_id());
    let event_body: Value = serde_json::from_str(&row.0).unwrap();
    assert_eq!(event_body["id"], "fanout-local-1");
    let targets: Value = serde_json::from_str(&row.1).unwrap();
    assert_eq!(targets[0]["type"], "conversation");
    assert_eq!(targets[0]["ids"][0], s.team_conv);
}

#[tokio::test]
async fn broadcast_to_conversations_and_users() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut room = ws_connect(addr, &room_ws(&s.team_conv, &format!("?token={}", s.admin_token)))
        .await
        .unwrap();
    wait_for_event(&mut room, "connection_established").await;
    let mut personal =
        ws_connect(addr, &format!("/api/websocket/connect?token={}", s.agent_token))
            .await
            .unwrap();
    wait_for_event(&mut personal, "user_connected").await;

    // Missing event or non-array targets -> 400 (CRD 3594).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast-to-conversations",
            Some(&s.admin_token),
            Some(json!({ "conversationIds": [s.team_conv] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast-to-conversations",
            Some(&s.admin_token),
            Some(json!({ "event": event("e1", "x", json!({})), "conversationIds": "nope" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Synchronous delivery with per-target outcomes (CRD 3590-3593).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast-to-conversations",
            Some(&s.admin_token),
            Some(json!({
                "event": event("e1", "conv_event", json!({ "k": "v" })),
                "conversationIds": [s.team_conv],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["targetCount"], 1);
    assert_eq!(body["data"]["successful"], 1);
    assert_eq!(body["data"]["failed"], 0);
    assert!(body["data"]["processingTimeMs"].is_number());
    let evt = wait_for_event(&mut room, "conv_event").await;
    assert_eq!(evt["payload"]["k"], "v");

    // User-targeted variant (CRD 3596-3598).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast-to-users",
            Some(&s.admin_token),
            Some(json!({
                "event": event("e2", "user_event", json!({ "k": "u" })),
                "userIds": [s.agent_id],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["successful"], 1);
    let evt = wait_for_event(&mut personal, "user_event").await;
    assert_eq!(evt["payload"]["k"], "u");
}

#[tokio::test]
async fn broadcast_to_teams_and_admin_inclusion() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut agent_ws =
        ws_connect(addr, &format!("/api/websocket/connect?token={}", s.agent_token))
            .await
            .unwrap();
    wait_for_event(&mut agent_ws, "user_connected").await;
    let mut admin_ws =
        ws_connect(addr, &format!("/api/websocket/connect?token={}", s.admin_token))
            .await
            .unwrap();
    wait_for_event(&mut admin_ws, "user_connected").await;

    // Team delivery resolves active members from storage (CRD 3600-3603).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast-to-teams",
            Some(&s.admin_token),
            Some(json!({
                "event": event("t1", "team_event", json!({ "k": 1 })),
                "teamIds": [s.team_id],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["successful"], 1);
    let evt = wait_for_event(&mut agent_ws, "team_event").await;
    assert_eq!(evt["payload"]["eventId"], "t1");
    // The admin is not a team member: nothing is delivered to them.
    expect_silence(&mut admin_ws, Duration::from_millis(300)).await;

    // Teams-and-admins delivers to the teams plus every active administrator
    // unless suppressed (CRD 3605-3609).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast-to-teams-and-admins",
            Some(&s.admin_token),
            Some(json!({
                "event": event("t2", "team_admin_event", json!({})),
                "teamIds": [s.team_id],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["teamCount"], 1);
    assert_eq!(body["data"]["includeAdmins"], true);
    wait_for_event(&mut agent_ws, "team_admin_event").await;
    wait_for_event(&mut admin_ws, "team_admin_event").await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast-to-teams-and-admins",
            Some(&s.admin_token),
            Some(json!({
                "event": event("t3", "team_only_event", json!({})),
                "teamIds": [s.team_id],
                "includeAdmins": false,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["includeAdmins"], false);
    wait_for_event(&mut agent_ws, "team_only_event").await;
    expect_silence(&mut admin_ws, Duration::from_millis(300)).await;
}

#[tokio::test]
async fn broadcast_global_batch_and_system_notification() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut agent_ws =
        ws_connect(addr, &format!("/api/websocket/connect?token={}", s.agent_token))
            .await
            .unwrap();
    wait_for_event(&mut agent_ws, "user_connected").await;
    let mut admin_ws =
        ws_connect(addr, &format!("/api/websocket/connect?token={}", s.admin_token))
            .await
            .unwrap();
    wait_for_event(&mut admin_ws, "user_connected").await;

    // Global delivery reaches everyone (CRD 3611-3614).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/broadcast-global",
            Some(&s.admin_token),
            Some(json!({ "event": event("g1", "global_event", json!({})) })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["successful"], 1);
    wait_for_event(&mut agent_ws, "global_event").await;
    wait_for_event(&mut admin_ws, "global_event").await;

    // Batch: one target per event, first target reused when absent
    // (CRD 3617-3621).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/batch-broadcast",
            Some(&s.admin_token),
            Some(json!({ "events": [event("b0", "x", json!({}))] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/batch-broadcast",
            Some(&s.admin_token),
            Some(json!({
                "events": [
                    event("b1", "batch_one", json!({})),
                    event("b2", "batch_two", json!({})),
                ],
                "targets": [ { "type": "user", "ids": [s.agent_id] } ],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["processed"], 2);
    wait_for_event(&mut agent_ws, "batch_one").await;
    wait_for_event(&mut agent_ws, "batch_two").await;

    // System notification addressed to everyone (CRD 3645-3648); high
    // priority rides the fast loop.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/system-broadcast",
            Some(&s.admin_token),
            Some(json!({ "message": "maintenance at noon", "priority": "high" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["eventId"].is_string());
    let evt = wait_for_event(&mut agent_ws, "system_notification").await;
    assert_eq!(evt["payload"]["message"], "maintenance at noon");
    wait_for_event(&mut admin_ws, "system_notification").await;
}

#[tokio::test]
async fn reachability_registry_filters_metrics_and_health() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = ws_connect(addr, &format!("/api/websocket/connect?token={}", s.agent_token))
        .await
        .unwrap();
    wait_for_event(&mut ws, "user_connected").await;

    // Register / unregister reachable endpoints (CRD 3623-3633).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/register-connection",
            Some(&s.admin_token),
            Some(json!({ "type": "room", "id": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/register-connection",
            Some(&s.admin_token),
            Some(json!({ "type": "conversation", "id": "conv-A" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let after_register = body["data"]["activeConnections"].as_i64().unwrap();
    assert!(after_register >= 2); // the live websocket + the registration

    // The debug snapshot lists both registered and live endpoints
    // (CRD 3656-3657).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/debug-connections",
            Some(&s.admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["conversations"].as_array().unwrap().contains(&json!("conv-A")));
    assert!(body["data"]["users"].as_array().unwrap().contains(&json!(s.agent_id)));

    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/unregister-connection",
            Some(&s.admin_token),
            Some(json!({ "type": "conversation", "id": "conv-A" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["activeConnections"].as_i64().unwrap(), after_register - 1);

    // Subscription filters are replaced wholesale (CRD 3635-3638).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/update-filters",
            Some(&s.admin_token),
            Some(json!({ "filters": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/broadcaster/update-filters",
            Some(&s.admin_token),
            Some(json!({ "targetKey": "user:abc", "filters": [ { "eventType": "new_message" } ] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Metrics & health (CRD 3650-3654): staff-accessible snapshots.
    let (status, body, _) = app
        .request("POST", "/api/realtime/broadcaster/metrics", Some(&s.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["totalEvents"].is_number());
    assert!(body["data"]["normalQueueDepth"].is_number());
    assert!(body["data"]["highPriorityQueueDepth"].is_number());
    assert!(body["data"]["uptimeSeconds"].is_number());
    assert!(body["data"]["reachableUsers"].is_number());

    for path in ["/api/realtime/broadcaster/status", "/api/realtime/broadcaster/health"] {
        let (status, body, _) = app.request("POST", path, Some(&s.agent_token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["healthy"], true);
        assert_eq!(body["data"]["status"], "healthy");
        assert!(body["data"]["queueSize"].is_number());
        assert!(body["data"]["errorRate"].is_number());
        assert!(body["data"]["timestamp"].is_string());
    }
}

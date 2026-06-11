//! User real-time session tests (CRD §5.3 lines 3694-3845): session open
//! contract, presence, subscriptions, preference persistence, security gate,
//! and per-user fan-out of pushed events.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::ws::{
    connect_rejected, mint, send_json, serve, wait_for_event, ws_connect, Ws,
};
use common::{spawn_app, TestApp};
use futures_util::StreamExt;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

struct Seeded {
    admin_id: String,
    admin_token: String,
    agent_id: String,
    agent_token: String,
    team_conv: String,
    team_conv2: String,
    foreign_conv: String,
    pool_conv: String,
}

async fn seed(app: &TestApp) -> Seeded {
    let admin_id = app.seed_agent("admin@us.io", "Secret123!", "admin").await;
    let agent_id = app.seed_agent("agent@us.io", "Secret123!", "agent").await;
    let team_id = app.seed_team("Sessions Team").await;
    let other_team_id = app.seed_team("Other Sessions Team").await;
    app.add_membership(&agent_id, team_id, "member", true).await;
    let customer = app.seed_customer("line", "U-us-1", "US Customer", Some(team_id)).await;
    let team_conv = app.seed_conversation(customer, Some(team_id), "assigned").await;
    let team_conv2 = app.seed_conversation(customer, Some(team_id), "assigned").await;
    let foreign_conv = app.seed_conversation(customer, Some(other_team_id), "assigned").await;
    let pool_conv = app.seed_conversation(customer, None, "active").await;
    let (admin_token, _, _) = app.login("admin@us.io", "Secret123!").await;
    let (agent_token, _, _) = app.login("agent@us.io", "Secret123!").await;
    Seeded {
        admin_id,
        admin_token,
        agent_id,
        agent_token,
        team_conv,
        team_conv2,
        foreign_conv,
        pool_conv,
    }
}

fn session_ws(token: &str, role: &str, user_id: &str) -> String {
    format!("/api/realtime/session/websocket?token={token}&role={role}&userId={user_id}")
}

async fn open_session(addr: std::net::SocketAddr, token: &str, role: &str, user_id: &str) -> Ws {
    let mut ws = ws_connect(addr, &session_ws(token, role, user_id)).await.unwrap();
    wait_for_event(&mut ws, "user_connected").await;
    ws
}

// -------------------------------------------------- session open (CRD 3701-3719)

#[tokio::test]
async fn session_open_parameter_and_auth_contract() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    // Missing token or role -> 400 "Missing required parameters" (CRD 3714).
    let (status, body) =
        connect_rejected(addr, "/api/realtime/session/websocket?role=agent").await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "Missing required parameters");
    let (status, body) = connect_rejected(
        addr,
        &format!("/api/realtime/session/websocket?token={}", s.agent_token),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "Missing required parameters");

    // Missing userId -> 400 "Missing userId parameter" (CRD 3715).
    let (status, body) = connect_rejected(
        addr,
        &format!("/api/realtime/session/websocket?token={}&role=agent", s.agent_token),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "Missing userId parameter");

    // Invalid token -> 401 "Unauthorized" (CRD 3716).
    let (status, body) =
        connect_rejected(addr, &session_ws("garbage", "agent", &s.agent_id)).await;
    assert_eq!(status, 401);
    assert_eq!(body["error"], "Unauthorized");

    // Token identity must equal the supplied user -> 401 (CRD 3709, 3716).
    let (status, body) =
        connect_rejected(addr, &session_ws(&s.agent_token, "agent", &s.admin_id)).await;
    assert_eq!(status, 401);
    assert_eq!(body["error"], "Unauthorized");

    // Unknown account in a validly signed token -> 401.
    let ghost = mint("ghost-user", "agent", 3600);
    let (status, _) = connect_rejected(addr, &session_ws(&ghost, "agent", "ghost-user")).await;
    assert_eq!(status, 401);

    // Valid open: welcome event with session id, followed conversations,
    // preferences and statistics (CRD 3710, 3833).
    let mut ws = ws_connect(addr, &session_ws(&s.agent_token, "agent", &s.agent_id))
        .await
        .unwrap();
    let welcome = wait_for_event(&mut ws, "user_connected").await;
    assert_eq!(welcome["payload"]["userId"], json!(s.agent_id));
    assert!(welcome["payload"]["connectionId"].is_string());
    assert_eq!(welcome["payload"]["subscriptions"], json!([]));
    assert_eq!(
        welcome["payload"]["preferences"]["notificationSettings"]["newMessage"],
        true
    );
    assert_eq!(welcome["payload"]["stats"]["totalSessions"], 1);
}

#[tokio::test]
async fn session_cap_is_five_simultaneous_sessions() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut held = Vec::new();
    for _ in 0..5 {
        held.push(open_session(addr, &s.agent_token, "agent", &s.agent_id).await);
    }
    // Per-user simultaneous-session cap -> 429 "Connection limit reached"
    // (CRD 3717, 3719).
    let (status, body) =
        connect_rejected(addr, &session_ws(&s.agent_token, "agent", &s.agent_id)).await;
    assert_eq!(status, 429);
    assert_eq!(body["error"], "Connection limit reached");
}

#[tokio::test]
async fn session_token_expiry_forces_close_with_refresh_code() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let expiry = chrono::Utc::now().timestamp() + 2;
    let mut ws = ws_connect(
        addr,
        &format!("{}&tokenExpiry={expiry}", session_ws(&s.agent_token, "agent", &s.agent_id)),
    )
    .await
    .unwrap();
    wait_for_event(&mut ws, "user_connected").await;
    // The session is force-closed at the supplied expiry with the dedicated
    // refresh-and-reconnect close code (CRD 3708, 3719, 3827).
    let close = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(frame))) => return frame,
                None => return None,
                _ => continue,
            }
        }
    })
    .await
    .expect("session was not closed at token expiry");
    let frame = close.expect("close frame should carry the refresh code");
    assert_eq!(u16::from(frame.code), 4401);
}

// ------------------------------------- subscriptions over HTTP (CRD 3721-3741)

#[tokio::test]
async fn http_subscribe_unsubscribe_permissions_and_session_events() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;

    // Missing conversation id -> 400.
    let (status, _, _) = app
        .request("POST", "/api/realtime/session/connect", Some(&s.agent_token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // View permission is required (CRD 3724): foreign-team conversations are
    // denied, and an unassigned conversation grants read but NOT view
    // (CRD 3816).
    for conv in [&s.foreign_conv, &s.pool_conv] {
        let (status, body, _) = app
            .request(
                "POST",
                "/api/realtime/session/connect",
                Some(&s.agent_token),
                Some(json!({ "conversationId": conv })),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "Permission denied");
    }

    // Follow an accessible conversation: success payload plus a
    // "conversation subscribed" event on every live session (CRD 3725-3727).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/session/connect",
            Some(&s.agent_token),
            Some(json!({ "conversationId": s.team_conv })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["conversationId"], json!(s.team_conv));
    assert_eq!(body["data"]["subscriptionCount"], 1);
    let evt = wait_for_event(&mut ws, "conversation_subscribed").await;
    assert_eq!(evt["payload"]["conversationId"], json!(s.team_conv));
    assert_eq!(evt["payload"]["subscriptionCount"], 1);

    // The /subscribe alias behaves identically (CRD 3721).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/session/subscribe",
            Some(&s.agent_token),
            Some(json!({ "conversationId": s.team_conv2 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["subscriptionCount"], 2);
    wait_for_event(&mut ws, "conversation_subscribed").await;

    // Unfollow: no permission check, event fan-out, success (CRD 3733-3739).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/session/disconnect",
            Some(&s.agent_token),
            Some(json!({ "conversationId": s.team_conv })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["subscriptionCount"], 1);
    let evt = wait_for_event(&mut ws, "conversation_unsubscribed").await;
    assert_eq!(evt["payload"]["conversationId"], json!(s.team_conv));

    // Removing a conversation that is not followed is a successful no-op
    // (CRD 3741), via the /unsubscribe alias.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/session/unsubscribe",
            Some(&s.agent_token),
            Some(json!({ "conversationId": s.foreign_conv })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["subscriptionCount"], 1);
}

#[tokio::test]
async fn subscription_ceiling_is_silently_capped_over_http_and_errors_over_ws() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    // Fill the followed set to the cap (CRD 3731: cap is 50).
    for i in 0..50 {
        assert!(app.state.realtime.subscribe(&s.admin_id, &format!("conv-{i}")).is_some());
    }
    let mut ws = open_session(addr, &s.admin_token, "admin", &s.admin_id).await;

    // The HTTP add is silently capped: success, count unchanged (CRD 3731).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/session/connect",
            Some(&s.admin_token),
            Some(json!({ "conversationId": s.pool_conv })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["subscriptionCount"], 50);

    // The inbound subscribe frame surfaces the cap as an error (CRD 3802).
    send_json(&mut ws, json!({ "type": "subscribe", "conversationId": s.team_conv })).await;
    let err = wait_for_event(&mut ws, "error").await;
    assert_eq!(err["payload"]["message"], "Maximum subscriptions reached");
}

// ----------------------------------------- preferences & state (CRD 3750-3760)

#[tokio::test]
async fn preferences_defaults_shallow_merge_and_method_contract() {
    let app = spawn_app().await;
    let s = seed(&app).await;

    // Defaults: all toggles enabled (CRD 3813).
    let (status, body, _) = app
        .request("GET", "/api/realtime/session/preferences", Some(&s.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    for key in ["newMessage", "messageRecall", "conversationAssignment", "systemNotifications"] {
        assert_eq!(body["data"]["notificationSettings"][key], true, "default {key}");
    }

    // Shallow merge of supplied fields (CRD 3757-3759).
    let (status, body, _) = app
        .request(
            "PUT",
            "/api/realtime/session/preferences",
            Some(&s.agent_token),
            Some(json!({
                "notificationSettings": {
                    "newMessage": false,
                    "messageRecall": true,
                    "conversationAssignment": true,
                    "systemNotifications": false,
                },
                "sound": "chime",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["notificationSettings"]["newMessage"], false);
    assert_eq!(body["data"]["notificationSettings"]["systemNotifications"], false);
    assert_eq!(body["data"]["sound"], "chime");

    let (_, body, _) = app
        .request("GET", "/api/realtime/session/preferences", Some(&s.agent_token), None)
        .await;
    assert_eq!(body["data"]["notificationSettings"]["newMessage"], false);

    // Any other method on this path -> 405 "Method not allowed" (CRD 3760).
    let (status, body, _) = app
        .request("POST", "/api/realtime/session/preferences", Some(&s.agent_token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body["error"], "Method not allowed");
}

#[tokio::test]
async fn followed_set_preferences_and_stats_survive_reconnect() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    let mut ws = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;
    send_json(&mut ws, json!({ "type": "subscribe", "conversationId": s.team_conv })).await;
    let ack = wait_for_event(&mut ws, "subscription_added").await;
    assert_eq!(ack["payload"]["subscriptionCount"], 1);
    let (status, _, _) = app
        .request(
            "PUT",
            "/api/realtime/session/preferences",
            Some(&s.agent_token),
            Some(json!({ "notificationSettings": {
                "newMessage": false,
                "messageRecall": true,
                "conversationAssignment": true,
                "systemNotifications": true,
            }})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Close the only session; the final snapshot is re-persisted (CRD 3828).
    ws.close(None).await.unwrap();
    let mut offline = false;
    for _ in 0..50 {
        let (_, body, _) = app
            .request("GET", "/api/realtime/session/status", Some(&s.agent_token), None)
            .await;
        if body["data"]["sessionCount"] == 0 && body["data"]["online"] == false {
            offline = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(offline, "user never transitioned to offline after closing the last session");

    // State is restored from persistence on the next session: the welcome
    // carries the followed set, merged preferences and cumulative statistics
    // (CRD 3812-3815, 3833).
    let mut ws = ws_connect(addr, &session_ws(&s.agent_token, "agent", &s.agent_id))
        .await
        .unwrap();
    let welcome = wait_for_event(&mut ws, "user_connected").await;
    assert_eq!(welcome["payload"]["subscriptions"], json!([s.team_conv]));
    assert_eq!(
        welcome["payload"]["preferences"]["notificationSettings"]["newMessage"],
        false
    );
    assert_eq!(welcome["payload"]["stats"]["totalSessions"], 2);
}

// ------------------------------------ presence / status / metrics (CRD 3743-3770)

#[tokio::test]
async fn presence_heartbeat_status_and_metrics_snapshots() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;

    // Heartbeat marks the user online and refreshes last-seen (CRD 3743-3748).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/session/presence",
            Some(&s.agent_token),
            Some(json!({ "status": "available" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["online"], true);
    assert!(body["data"]["lastSeen"].is_string());

    let _ws = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;

    // Status snapshot (CRD 3762-3765).
    let (status, body, _) = app
        .request("GET", "/api/realtime/session/status", Some(&s.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["userId"], json!(s.agent_id));
    assert_eq!(body["data"]["online"], true);
    assert_eq!(body["data"]["sessionCount"], 1);
    assert!(body["data"]["lastSeen"].is_string());
    assert!(body["data"]["stats"]["totalSessions"].is_number());
    assert_eq!(body["data"]["subscriptionCount"], 0);

    // Metrics snapshot adds a derived uptime (CRD 3767-3770).
    let (status, body, _) = app
        .request("GET", "/api/realtime/session/metrics", Some(&s.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["uptimeSeconds"].is_number());
    assert_eq!(body["data"]["sessionCount"], 1);

    // The whole session surface requires authentication.
    let (status, _, _) = app.request("GET", "/api/realtime/session/status", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ----------------------------------------- pushed delivery (CRD 3772-3786)

#[tokio::test]
async fn broadcast_pushes_to_every_live_session_of_the_user() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut a = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;
    let mut b = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;

    let (status, _, _) = app
        .request("POST", "/api/realtime/session/broadcast", Some(&s.agent_token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The supplied message reaches every live session (CRD 3772-3777, 3840).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/session/broadcast",
            Some(&s.agent_token),
            Some(json!({ "message": { "type": "custom_alert", "payload": { "n": 7 } } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["delivered"], 2);
    let evt = wait_for_event(&mut a, "custom_alert").await;
    assert_eq!(evt["payload"]["n"], 7);
    wait_for_event(&mut b, "custom_alert").await;

    // Only administrators may target another user.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/session/broadcast",
            Some(&s.agent_token),
            Some(json!({ "userId": s.admin_id, "message": { "type": "x" } })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request(
            "POST",
            "/api/realtime/session/broadcast",
            Some(&s.admin_token),
            Some(json!({ "userId": s.agent_id, "message": { "type": "admin_note", "payload": {} } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    wait_for_event(&mut a, "admin_note").await;
    wait_for_event(&mut b, "admin_note").await;
}

#[tokio::test]
async fn batch_events_validation_delivery_and_received_counter() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;

    // Missing or non-array events -> 400 "Invalid events format" (CRD 3785).
    for bad in [json!({}), json!({ "events": "nope" })] {
        let (status, body, _) = app
            .request("POST", "/api/realtime/session/batch-events", Some(&s.agent_token), Some(bad))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "Invalid events format");
    }

    // Each event is wrapped, preserving its conversation association and
    // timestamp (defaulting to now), and fanned out (CRD 3779-3783).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/realtime/session/batch-events",
            Some(&s.agent_token),
            Some(json!({ "events": [
                {
                    "type": "new_message",
                    "conversationId": s.team_conv,
                    "timestamp": "2026-06-11T01:02:03.000Z",
                    "data": { "preview": "hello" },
                },
                { "type": "conversation_updated", "data": {} },
            ]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["eventsProcessed"], 2);
    assert_eq!(body["data"]["userId"], json!(s.agent_id));
    assert_eq!(body["data"]["activeSessions"], 1);

    let evt = wait_for_event(&mut ws, "new_message").await;
    assert_eq!(evt["payload"]["conversationId"], json!(s.team_conv));
    assert_eq!(evt["payload"]["eventTimestamp"], "2026-06-11T01:02:03.000Z");
    assert_eq!(evt["payload"]["preview"], "hello");
    let evt = wait_for_event(&mut ws, "conversation_updated").await;
    assert!(evt["payload"]["eventTimestamp"].is_string());

    // The received-messages counter advances (CRD 3814).
    let (_, body, _) = app
        .request("GET", "/api/realtime/session/status", Some(&s.agent_token), None)
        .await;
    assert_eq!(body["data"]["stats"]["messagesReceived"], 2);
}

// ----------------------------------------- inbound security gate (CRD 3795-3799)

#[tokio::test]
async fn inbound_frames_are_size_and_rate_limited() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut ws = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;

    // Frames over the size ceiling are rejected, not processed (CRD 3797).
    let oversized = json!({ "type": "ping", "padding": "x".repeat(11_000) });
    send_json(&mut ws, oversized).await;
    let err = wait_for_event(&mut ws, "error").await;
    assert_eq!(err["payload"]["message"], "Message too large. Maximum size is 10240 bytes");

    // At most 10 frames per 1-second window; the excess frame yields an error
    // and is not processed (CRD 3796).
    for i in 0..11 {
        send_json(&mut ws, json!({ "type": "ping", "timestamp": format!("p{i}") })).await;
    }
    let err = wait_for_event(&mut ws, "error").await;
    assert_eq!(err["payload"]["message"], "Rate limit exceeded. Please slow down.");
}

// -------------------------------- session frame fan-out & counters (CRD 3800-3806)

#[tokio::test]
async fn typing_events_rebroadcast_to_own_sessions_and_chat_ack_counts() {
    let app = spawn_app().await;
    let s = seed(&app).await;
    let addr = serve(&app).await;
    let mut a = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;
    let mut b = open_session(addr, &s.agent_token, "agent", &s.agent_id).await;

    // Typing events are re-broadcast to all of the user's own live sessions
    // (CRD 3805, 3838).
    send_json(
        &mut a,
        json!({ "type": "event", "event": "typing_start", "conversationId": s.team_conv }),
    )
    .await;
    let evt = wait_for_event(&mut b, "typing_start").await;
    assert_eq!(evt["payload"]["userId"], json!(s.agent_id));

    // Chat frames require a followed conversation and are acknowledged to the
    // requesting session only; the sent counter increments (CRD 3804).
    send_json(&mut a, json!({ "type": "subscribe", "conversationId": s.team_conv })).await;
    wait_for_event(&mut a, "subscription_added").await;
    send_json(
        &mut a,
        json!({ "type": "message", "conversationId": s.team_conv, "messageId": "m-9" }),
    )
    .await;
    let ack = wait_for_event(&mut a, "message_acknowledged").await;
    assert_eq!(ack["payload"]["messageId"], "m-9");
    assert_eq!(ack["payload"]["conversationId"], json!(s.team_conv));

    let (_, body, _) = app
        .request("GET", "/api/realtime/session/status", Some(&s.agent_token), None)
        .await;
    assert_eq!(body["data"]["stats"]["messagesSent"], 1);
}

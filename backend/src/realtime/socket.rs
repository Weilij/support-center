//! WebSocket connect endpoint and the inbound client message protocol
//! (CRD §5.1 lines 3230-3258 and 3410-3419).

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::AppError;
use crate::state::AppState;

use super::gate;
use super::hub::{
    frame, ConnIdentity, RegisterError, INBOUND_FRAMES_PER_SEC, MAX_INBOUND_FRAME_BYTES,
};

#[derive(Deserialize)]
pub struct ConnectQuery {
    pub token: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "deviceId")]
    pub device_id: Option<String>,
}

/// Open a real-time connection — GET /api/websocket/connect (CRD 3230-3258).
pub async fn connect(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ConnectQuery>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    // Handshake gate first (CRD 600-610): token checks, role, identity,
    // conversation access.
    let outcome =
        match gate::authorize(&state, q.token.as_deref(), q.conversation_id.as_deref()).await {
            Ok(o) => o,
            Err(resp) => return *resp,
        };

    // Real-time feature must be enabled (CRD 3239, 3254).
    if !state.realtime.config().enabled {
        return AppError::ServiceUnavailable(
            "Realtime feature is disabled".into(),
            "REALTIME_DISABLED",
        )
        .into_response();
    }

    // Must be a protocol-upgrade request (CRD 3231, 3255).
    let ws = match ws {
        Ok(ws) => ws,
        Err(_) => {
            return AppError::BadRequest("WebSocket upgrade required".into()).into_response()
        }
    };

    // Restore the persisted per-user realtime state (subscriptions,
    // preferences, statistics) before registering so the welcome event
    // reflects it across reconnects (CRD 3812-3815).
    super::user_sessions::hydrate(&state, &outcome.identity.user_id).await;

    // Connection ceilings are enforced before accepting (CRD 3241, 3256).
    let conversation_id = q.conversation_id.filter(|c| !c.is_empty());
    let registration = match state.realtime.register(
        outcome.identity.clone(),
        conversation_id.clone(),
        q.device_id.clone(),
    ) {
        Ok(r) => r,
        Err(RegisterError::CeilingReached(reason)) => {
            return AppError::TooManyRequests { message: reason.to_string(), retry_after: 30 }
                .into_response()
        }
    };

    // Best-effort connection-quality analytics record (CRD 610, 645).
    {
        let db = state.db.clone();
        let user_id = outcome.identity.user_id.clone();
        let connection_id = registration.connection_id.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "INSERT INTO realtime_quality_samples (id, timestamp, user_id, connection_id, details, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(crate::db::now_iso())
            .bind(user_id)
            .bind(connection_id)
            .bind(json!({ "event": "handshake_authenticated" }).to_string())
            .bind(crate::db::now_iso())
            .execute(&db)
            .await;
        });
    }

    let exp = outcome.exp;
    let identity = outcome.identity;
    ws.on_upgrade(move |socket| {
        run_socket(state, socket, registration, identity, conversation_id, exp)
    })
}

/// Drive one accepted socket: forward hub broadcasts out, dispatch inbound
/// frames, force-close at credential expiry (CRD 3242, 3431), reap on idle.
/// Shared by the gateway connect, the conversation-room endpoint (§5.2) and
/// the user-session endpoint (§5.3).
pub(crate) async fn run_socket(
    state: Arc<AppState>,
    socket: WebSocket,
    registration: super::hub::Registration,
    identity: ConnIdentity,
    conversation_id: Option<String>,
    exp: i64,
) {
    let connection_id = registration.connection_id;
    let mut rx = registration.rx;
    let (mut sink, mut stream) = socket.split();

    // Forced close at the moment the credential would expire (CRD 3242):
    // the client receives close code 4401 and should refresh + reconnect.
    let until_expiry = (exp - chrono::Utc::now().timestamp()).max(0) as u64;
    let expiry = tokio::time::sleep(Duration::from_secs(until_expiry));
    tokio::pin!(expiry);

    // Inbound rate limiting: ~10 frames per second per connection (CRD 3419).
    let mut recent: VecDeque<Instant> = VecDeque::new();
    let mut idle_check = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = &mut expiry => {
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code: 4401,
                        reason: "Token expired".into(),
                    })))
                    .await;
                break;
            }
            _ = idle_check.tick() => {
                // Inactivity reap (~5 minutes idle, CRD 3431).
                if state.realtime.idle_connections().contains(&connection_id) {
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: 4000,
                            reason: "Idle timeout".into(),
                        })))
                        .await;
                    break;
                }
            }
            out = rx.recv() => {
                match out {
                    Some(text) => {
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    // Hub dropped the sender (forced disconnect endpoint).
                    None => {
                        let _ = sink
                            .send(Message::Close(Some(CloseFrame {
                                code: 1000,
                                reason: "Disconnected".into(),
                            })))
                            .await;
                        break;
                    }
                }
            }
            inbound = stream.next() => {
                let Some(Ok(msg)) = inbound else { break };
                match msg {
                    Message::Text(text) => {
                        state.realtime.touch(&connection_id);
                        // Frame size ceiling (CRD 3419).
                        if text.len() > MAX_INBOUND_FRAME_BYTES {
                            send_error(
                                &state, &connection_id,
                                &format!(
                                    "Message too large. Maximum size is {MAX_INBOUND_FRAME_BYTES} bytes"
                                ),
                            );
                            continue;
                        }
                        // Per-connection inbound rate limit (CRD 3419, 3796).
                        let now = Instant::now();
                        while recent
                            .front()
                            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
                        {
                            recent.pop_front();
                        }
                        if recent.len() >= INBOUND_FRAMES_PER_SEC {
                            send_error(
                                &state, &connection_id,
                                "Rate limit exceeded. Please slow down.",
                            );
                            continue;
                        }
                        recent.push_back(now);
                        handle_frame(
                            &state,
                            &connection_id,
                            &identity,
                            conversation_id.as_deref(),
                            text.as_str(),
                        )
                        .await;
                    }
                    Message::Close(_) => break,
                    // Transport-level pings are answered by axum automatically.
                    _ => state.realtime.touch(&connection_id),
                }
            }
        }
    }

    // On the user's last connection the final state snapshot (offline,
    // last-seen refreshed) is re-persisted (CRD 3824, 3828).
    if let Some(snapshot) = state.realtime.unregister(&connection_id) {
        super::user_sessions::persist_snapshot(&state.db, &snapshot).await;
    }
}

/// Send an error frame to one connection (CRD 3411, 3690).
fn send_error(state: &Arc<AppState>, connection_id: &str, message: &str) {
    state
        .realtime
        .to_connection(connection_id, frame("error", json!({ "message": message })));
}

fn send_event(state: &Arc<AppState>, connection_id: &str, event: &str, payload: Value) {
    state.realtime.to_connection(connection_id, frame(event, payload));
}

fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).or_else(|| v.get("payload").and_then(|p| p.get(key)))
}

fn field_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    field(v, key).and_then(Value::as_str)
}

/// Dispatch one parsed inbound frame per the documented protocol (CRD 3410-3417).
async fn handle_frame(
    state: &Arc<AppState>,
    connection_id: &str,
    identity: &ConnIdentity,
    room: Option<&str>,
    text: &str,
) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        send_error(state, connection_id, "Invalid message format");
        return;
    };
    let Some(typ) = v.get("type").and_then(Value::as_str) else {
        send_error(state, connection_id, "Invalid message format");
        return;
    };

    match typ {
        // Keepalive: pong echoing a timestamp (CRD 3412).
        "ping" => {
            let echo = v.get("timestamp").cloned().unwrap_or(Value::Null);
            send_event(state, connection_id, "pong", json!({ "echo": echo }));
        }

        // Subscribe / unsubscribe (CRD 3413, 3548). In full-mode rooms these
        // are accepted no-ops (membership is implicit); in simplified rooms
        // they are ignored entirely (CRD 3548); on the personal channel
        // subscribe is permission-checked and capped.
        "subscribe" => {
            if let Some(cid) = room {
                if state.realtime.room_mode(cid) != "simplified" {
                    send_event(
                        state,
                        connection_id,
                        "subscription_added",
                        json!({ "conversationId": field_str(&v, "conversationId") }),
                    );
                }
                return;
            }
            let Some(cid) = field_str(&v, "conversationId").map(str::to_string) else {
                send_error(state, connection_id, "Conversation ID is required");
                return;
            };
            let team_ids = identity.team_ids.clone();
            if !can_view(state, &identity.user_id, &identity.role, &team_ids, &cid).await {
                send_error(
                    state,
                    connection_id,
                    "Permission denied to subscribe to this conversation",
                );
                return;
            }
            match state.realtime.subscribe(&identity.user_id, &cid) {
                Some(count) => {
                    // The followed set is persisted so it survives reconnects
                    // (CRD 3802, 3812).
                    super::user_sessions::persist_user(state, &identity.user_id).await;
                    send_event(
                        state,
                        connection_id,
                        "subscription_added",
                        json!({ "conversationId": cid, "subscriptionCount": count }),
                    )
                }
                None => send_error(state, connection_id, "Maximum subscriptions reached"),
            }
        }
        "unsubscribe" => {
            if let Some(cid) = room {
                if state.realtime.room_mode(cid) != "simplified" {
                    send_event(
                        state,
                        connection_id,
                        "subscription_removed",
                        json!({ "conversationId": field_str(&v, "conversationId") }),
                    );
                }
                return;
            }
            let Some(cid) = field_str(&v, "conversationId").map(str::to_string) else {
                send_error(state, connection_id, "Conversation ID is required");
                return;
            };
            let count = state.realtime.unsubscribe(&identity.user_id, &cid);
            super::user_sessions::persist_user(state, &identity.user_id).await;
            send_event(
                state,
                connection_id,
                "subscription_removed",
                json!({ "conversationId": cid, "subscriptionCount": count }),
            );
        }

        // Chat message (CRD 3414, 3555-3567).
        "message" => match room {
            Some(cid) => {
                // Full mode requires a recognized staff role; simplified mode
                // performs no permission check (CRD 3557).
                if state.realtime.room_mode(cid) != "simplified"
                    && identity.role != "admin"
                    && identity.role != "agent"
                {
                    send_error(state, connection_id, "Permission denied to send messages");
                    return;
                }
                let message_type =
                    field_str(&v, "messageType").unwrap_or("text").to_string();
                // Typing indicators are relayed to other participants only,
                // never stored (CRD 3560).
                if message_type == "typing" {
                    state.realtime.relay_to_room_others(
                        cid,
                        connection_id,
                        "typing",
                        json!({ "userId": identity.user_id, "userName": identity.display_name }),
                    );
                    return;
                }
                let seq = state.realtime.next_seq(cid);
                let message_id = field_str(&v, "messageId")
                    .map(str::to_string)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let mut metadata = field(&v, "metadata").cloned().unwrap_or(json!({}));
                metadata["sequence"] = json!(seq);
                let sender_name = field_str(&v, "senderName")
                    .unwrap_or(identity.display_name.as_str())
                    .to_string();
                let payload = json!({
                    "messageId": message_id,
                    "conversationId": cid,
                    "content": field_str(&v, "content").unwrap_or(""),
                    "messageType": message_type,
                    "senderId": identity.user_id,
                    "senderName": sender_name,
                    "senderType": "agent",
                    "metadata": metadata,
                    "timestamp": crate::db::now_iso(),
                });
                // High-priority broadcast to all participants + bounded
                // history for reconnection sync (CRD 3561-3565).
                state.realtime.to_conversation_message(cid, "message_sent", payload);
                state.realtime.note_message_sent(&identity.user_id);
            }
            None => {
                // Personal channel: requires a subscribed target conversation;
                // acknowledged only (CRD 3414, 3804).
                let Some(cid) = field_str(&v, "conversationId").map(str::to_string) else {
                    send_error(state, connection_id, "Conversation ID required for chat messages");
                    return;
                };
                if !state.realtime.is_subscribed(&identity.user_id, &cid) {
                    send_error(state, connection_id, "Not subscribed to this conversation");
                    return;
                }
                state.realtime.note_message_sent(&identity.user_id);
                send_event(
                    state,
                    connection_id,
                    "message_acknowledged",
                    json!({
                        "messageId": field(&v, "messageId").cloned().unwrap_or(Value::Null),
                        "conversationId": cid,
                        "userId": identity.user_id,
                    }),
                );
            }
        },

        // Event frames: typing start/stop relayed; others acknowledged
        // (CRD 3415, 3550, 3805). Generic events are full-mode only in
        // conversation rooms (CRD 3550).
        "event" => {
            if let Some(cid) = room {
                if state.realtime.room_mode(cid) == "simplified" {
                    return;
                }
            }
            let subtype = field_str(&v, "event")
                .or_else(|| field_str(&v, "eventType"))
                .or_else(|| v.get("payload").and_then(|p| p.get("type")).and_then(Value::as_str))
                .unwrap_or("");
            match subtype {
                "typing_start" | "typing_stop" => {
                    let payload = json!({
                        "userId": identity.user_id,
                        "userName": identity.display_name,
                        "conversationId": field(&v, "conversationId").cloned().unwrap_or(Value::Null),
                    });
                    match room {
                        Some(cid) => {
                            state
                                .realtime
                                .relay_to_room_others(cid, connection_id, subtype, payload);
                        }
                        None => {
                            // Re-broadcast to the user's own live sessions (CRD 3805).
                            state.realtime.to_user(&identity.user_id, subtype, payload);
                        }
                    }
                }
                other => {
                    send_event(state, connection_id, "ack", json!({ "event": other }));
                }
            }
        }

        // Reconnection sync (full-mode conversation rooms only, CRD 3416,
        // 3551, 3569-3572). Ignored in simplified rooms.
        "sync" => match room {
            Some(cid) => {
                if state.realtime.room_mode(cid) == "simplified" {
                    return;
                }
                let since = field_str(&v, "since").map(str::to_string);
                let (missed, last_message_at) =
                    state.realtime.sync_since(cid, since.as_deref());
                let missed_count = missed.len();
                send_event(
                    state,
                    connection_id,
                    "sync_response",
                    json!({
                        "conversationId": cid,
                        "messages": missed,
                        "missedCount": missed_count,
                        "syncedAt": crate::db::now_iso(),
                        "lastMessageAt": last_message_at,
                    }),
                );
            }
            None => send_error(state, connection_id, "Unknown message type: sync"),
        },

        other => {
            send_error(state, connection_id, &format!("Unknown message type: {other}"));
        }
    }
}

/// Subscribe "view" permission (CRD 3724, 3802, 3816): administrators may view
/// any conversation; a conversation assigned to an owning team is viewable
/// only by that team's members; an unassigned conversation grants "read" but
/// NOT "view", so subscribing to it is denied; a missing conversation or any
/// evaluation error denies (fail-closed). Decisions are cached briefly under a
/// view-specific key so they never mix with the gate's broader "read" cache.
pub(crate) async fn can_view(
    state: &Arc<AppState>,
    user_id: &str,
    role: &str,
    team_ids: &[i64],
    conversation_id: &str,
) -> bool {
    if role == "admin" {
        return true;
    }
    let cache_key = format!("view:{user_id}");
    if let Some(allowed) = state.realtime.cached_access(&cache_key, conversation_id) {
        return allowed;
    }
    let team: Option<Option<i64>> =
        sqlx::query_scalar("SELECT team_id FROM conversations WHERE id = ?")
            .bind(conversation_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let allowed = match team {
        // Unassigned: read-style access only, view denied (CRD 3816).
        Some(None) => false,
        Some(Some(team_id)) => team_ids.contains(&team_id),
        None => false,
    };
    state.realtime.cache_access(&cache_key, conversation_id, allowed);
    allowed
}

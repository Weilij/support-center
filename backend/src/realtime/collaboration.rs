//! Collaboration (CRD §3.4 lines 2321-2446): live conversation viewers,
//! typing indicators, per-user availability presence, aggregate statistics,
//! cleanup and health — all ephemeral session state tied to live activity,
//! never durable rows (CRD 2414).
//!
//! Mounted under `/api/collaboration` behind the bearer middleware. Live
//! events (user_joined / user_left / typing_start / typing_stop /
//! presence_update) are emitted into the conversation's realtime room with
//! the originator excluded (CRD 2436-2443).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

/// Per-conversation viewer capacity (CRD 2367: default fifty).
pub const VIEWER_CAP: usize = 50;
/// Typing indicators auto-expire after a few seconds (CRD 2381, 2418).
pub const TYPING_TTL: Duration = Duration::from_secs(5);
/// Presence is reclaimed after several minutes of inactivity (CRD 2399, 2419).
pub const PRESENCE_TTL: Duration = Duration::from_secs(300);
/// The most-active ranking is capped to a small top-N set (CRD 2404).
pub const TOP_ACTIVE_CAP: usize = 5;

struct Viewer {
    user_id: String,
    username: String,
    display_name: String,
    role: String,
    joined_at: String,
    last_activity: String,
    /// `Some` while a typing indicator is active: (expiry, startedAt-iso).
    typing: Option<(Instant, String)>,
}

struct Presence {
    status: String,
    current_conversation: Option<i64>,
    last_seen: String,
    metadata: Value,
    expires: Instant,
}

/// Live collaboration state: viewers per conversation plus per-user presence
/// (CRD 2406-2414).
#[derive(Default)]
pub struct CollabState {
    rooms: Mutex<HashMap<i64, HashMap<String, Viewer>>>,
    presence: Mutex<HashMap<String, Presence>>,
    /// Lazy-initialization marker for the health probe (CRD 2391, 2421).
    initialized: AtomicBool,
}

pub struct RoomFull;

impl CollabState {
    fn touch_init(&self) {
        self.initialized.store(true, Ordering::Relaxed);
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Relaxed)
    }

    /// Register a viewer; enforces the per-conversation capacity (CRD 2366).
    /// Re-joining refreshes the existing entry rather than consuming capacity.
    pub fn join(
        &self,
        conversation_id: i64,
        user_id: &str,
        username: &str,
        display_name: &str,
        role: &str,
    ) -> std::result::Result<(), RoomFull> {
        self.touch_init();
        let mut rooms = self.rooms.lock();
        let room = rooms.entry(conversation_id).or_default();
        let now = crate::db::now_iso();
        if let Some(existing) = room.get_mut(user_id) {
            existing.last_activity = now;
            return Ok(());
        }
        if room.len() >= VIEWER_CAP {
            return Err(RoomFull);
        }
        room.insert(
            user_id.to_string(),
            Viewer {
                user_id: user_id.to_string(),
                username: username.to_string(),
                display_name: display_name.to_string(),
                role: role.to_string(),
                joined_at: now.clone(),
                last_activity: now,
                typing: None,
            },
        );
        Ok(())
    }

    /// Remove a viewer; leaving when not present is harmless (CRD 2374).
    pub fn leave(&self, conversation_id: i64, user_id: &str) {
        self.touch_init();
        let mut rooms = self.rooms.lock();
        if let Some(room) = rooms.get_mut(&conversation_id) {
            room.remove(user_id);
            if room.is_empty() {
                rooms.remove(&conversation_id);
            }
        }
    }

    /// Start/stop the caller's typing indicator (CRD 2376-2382). The viewer
    /// entry is created implicitly when absent so typing works without an
    /// explicit join.
    pub fn set_typing(
        &self,
        conversation_id: i64,
        user_id: &str,
        username: &str,
        display_name: &str,
        role: &str,
        active: bool,
    ) {
        self.touch_init();
        let mut rooms = self.rooms.lock();
        let room = rooms.entry(conversation_id).or_default();
        let now = crate::db::now_iso();
        let viewer = room.entry(user_id.to_string()).or_insert_with(|| Viewer {
            user_id: user_id.to_string(),
            username: username.to_string(),
            display_name: display_name.to_string(),
            role: role.to_string(),
            joined_at: now.clone(),
            last_activity: now.clone(),
            typing: None,
        });
        viewer.last_activity = now.clone();
        viewer.typing = active.then(|| (Instant::now() + TYPING_TTL, now));
    }

    /// Record the caller's availability (CRD 2384-2390).
    pub fn set_presence(
        &self,
        user_id: &str,
        status: &str,
        current_conversation: Option<i64>,
        metadata: Value,
    ) {
        self.touch_init();
        let mut presence = self.presence.lock();
        presence.insert(
            user_id.to_string(),
            Presence {
                status: status.to_string(),
                current_conversation,
                last_seen: crate::db::now_iso(),
                metadata,
                expires: Instant::now() + PRESENCE_TTL,
            },
        );
    }

    /// Current (unexpired) presence record for a user (CRD 2410).
    pub fn presence_snapshot(&self, user_id: &str) -> Option<Value> {
        let presence = self.presence.lock();
        let p = presence
            .get(user_id)
            .filter(|p| p.expires > Instant::now())?;
        Some(json!({
            "userId": user_id,
            "status": p.status,
            "currentConversation": p.current_conversation,
            "lastSeen": p.last_seen,
            "metadata": p.metadata,
        }))
    }

    fn viewer_json(v: &Viewer) -> Option<Value> {
        // Viewer identifiers must be interpretable as finite numbers;
        // anything else is omitted from listings (CRD 2349, 2407).
        let user_id: f64 = v.user_id.parse().ok().filter(|n: &f64| n.is_finite())?;
        let id_label = format!("User {}", v.user_id);
        let username = if v.username.is_empty() {
            id_label.clone()
        } else {
            v.username.clone()
        };
        let display = if !v.display_name.is_empty() {
            v.display_name.clone()
        } else {
            username.clone()
        };
        let role = if v.role.is_empty() {
            "agent".to_string()
        } else {
            v.role.clone()
        };
        Some(json!({
            "userId": user_id,
            "username": username,
            "displayName": display,
            "role": role,
            "joinedAt": v.joined_at,
            "protocol": "websocket",
            "isTyping": v.typing.as_ref().is_some_and(|(exp, _)| *exp > Instant::now()),
            "lastActivity": v.last_activity,
        }))
    }

    fn typing_json(conversation_id: i64, v: &Viewer) -> Option<Value> {
        let (expiry, started_at) = v.typing.as_ref()?;
        if *expiry <= Instant::now() {
            return None;
        }
        let remaining = expiry.saturating_duration_since(Instant::now());
        let expires_at = (chrono::Utc::now() + chrono::Duration::from_std(remaining).ok()?)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        Some(json!({
            "userId": v.user_id,
            "username": v.username,
            "displayName": v.display_name,
            "conversationId": conversation_id,
            "startedAt": started_at,
            "expiresAt": expires_at,
        }))
    }

    /// Normalized viewer listing (CRD 2347-2353).
    pub fn viewers(&self, conversation_id: i64) -> Vec<Value> {
        let rooms = self.rooms.lock();
        rooms
            .get(&conversation_id)
            .map(|room| room.values().filter_map(Self::viewer_json).collect())
            .unwrap_or_default()
    }

    /// Active (unexpired) typing entries (CRD 2336).
    pub fn typing_entries(&self, conversation_id: i64) -> Vec<Value> {
        let rooms = self.rooms.lock();
        rooms
            .get(&conversation_id)
            .map(|room| {
                room.values()
                    .filter_map(|v| Self::typing_json(conversation_id, v))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Aggregate statistics (CRD 2400-2404).
    pub fn stats(&self) -> Value {
        let rooms = self.rooms.lock();
        let total_viewers: usize = rooms.values().map(HashMap::len).sum();
        let total_typing: usize = rooms
            .values()
            .flat_map(HashMap::values)
            .filter(|v| {
                v.typing
                    .as_ref()
                    .is_some_and(|(exp, _)| *exp > Instant::now())
            })
            .count();
        let mut ranked: Vec<(i64, usize)> =
            rooms.iter().map(|(cid, room)| (*cid, room.len())).collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let most_active: Vec<Value> = ranked
            .into_iter()
            .take(TOP_ACTIVE_CAP)
            .map(|(cid, count)| json!({ "conversationId": cid, "viewerCount": count }))
            .collect();
        json!({
            "totalViewers": total_viewers,
            "totalTyping": total_typing,
            "totalRooms": rooms.len(),
            "connectionsByProtocol": { "websocket": total_viewers },
            "mostActiveConversations": most_active,
        })
    }

    /// Purge expired typing indicators and presence records; returns the
    /// count removed (CRD 2425-2429).
    pub fn cleanup(&self) -> usize {
        let mut cleaned = 0usize;
        let mut rooms = self.rooms.lock();
        for room in rooms.values_mut() {
            for v in room.values_mut() {
                if v.typing
                    .as_ref()
                    .is_some_and(|(exp, _)| *exp <= Instant::now())
                {
                    v.typing = None;
                    cleaned += 1;
                }
            }
        }
        drop(rooms);
        let mut presence = self.presence.lock();
        let before = presence.len();
        presence.retain(|_, p| p.expires > Instant::now());
        cleaned += before - presence.len();
        cleaned
    }

    fn last_activity(&self, conversation_id: i64) -> Option<String> {
        let rooms = self.rooms.lock();
        rooms
            .get(&conversation_id)
            .and_then(|room| room.values().map(|v| v.last_activity.clone()).max())
    }
}

// ------------------------------------------------------------------ helpers

/// Path conversation identifiers must be positive integers (CRD 2331).
fn parse_conversation_id(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::BadRequest("Conversation ID must be a positive integer".into()))
}

/// Error response carrying a non-taxonomy machine code (PROTOCOL_NOT_SUPPORTED
/// / ROOM_FULL, CRD 2340, 2366).
fn coded_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "success": false,
            "error": message,
            "code": code,
            "timestamp": crate::db::now_iso(),
            "requestId": envelope::request_id(),
        })),
    )
        .into_response()
}

/// The only effective transport is the realtime WebSocket; explicitly
/// requesting anything else yields a protocol-not-supported error with its
/// machine code (CRD 2340).
#[allow(clippy::result_large_err)] // the Err is the ready-to-send denial response
fn check_protocol(protocol: Option<&str>) -> std::result::Result<(), Response> {
    match protocol {
        None | Some("") | Some("websocket") => Ok(()),
        Some(other) => Err(coded_error(
            StatusCode::BAD_REQUEST,
            "PROTOCOL_NOT_SUPPORTED",
            &format!("Protocol not supported: {other}"),
        )),
    }
}

#[derive(serde::Deserialize)]
pub struct ProtocolQuery {
    pub protocol: Option<String>,
}

/// Numeric conversation identifiers also travel as numeric strings
/// (CRD 2378, 2387).
fn body_conversation_id(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

/// Broadcast one collaboration event into the conversation's realtime room,
/// excluding the originator (CRD 2436-2443). Best-effort.
fn emit(state: &AppState, conversation_id: i64, origin_user: &str, event: &str, payload: Value) {
    state.realtime.to_conversation_except_user(
        &conversation_id.to_string(),
        origin_user,
        event,
        payload,
    );
}

// ----------------------------------------------------------------- handlers

/// GET /api/collaboration/conversations/{conversationId}/state (CRD 2329-2345).
pub async fn conversation_state(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    Query(q): Query<ProtocolQuery>,
) -> Result {
    let conversation_id = parse_conversation_id(&raw_id)?;
    if let Err(resp) = check_protocol(q.protocol.as_deref()) {
        return Ok(resp);
    }
    let _ = user; // identity read from the auth context (CRD 2333)
    state.realtime.collab.touch_init();
    let viewers = state.realtime.collab.viewers(conversation_id);
    let typing = state.realtime.collab.typing_entries(conversation_id);
    // Live-room metrics enrich the snapshot; when unavailable the reduced
    // snapshot below still carries the viewer list (CRD 2335).
    let metrics = state
        .realtime
        .room_metrics_snapshot(&conversation_id.to_string());
    let connection_count = metrics["activeConnections"]
        .as_u64()
        .unwrap_or(viewers.len() as u64);
    let last_activity = state
        .realtime
        .collab
        .last_activity(conversation_id)
        .map(Value::from)
        .unwrap_or_else(|| metrics["lastActivity"].clone());
    Ok(envelope::ok(json!({
        "conversationId": conversation_id,
        "viewers": viewers,
        "typingUsers": typing,
        "connectionCount": connection_count,
        "protocol": "websocket",
        "lastActivity": last_activity,
        "metadata": {
            "recentMessageCount": metrics["historyLength"].as_u64().unwrap_or(0),
            "isActive": metrics["active"].as_bool().unwrap_or(false),
        },
    })))
}

/// GET /api/collaboration/conversations/{conversationId}/viewers
/// (CRD 2347-2356).
pub async fn viewers(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    Query(q): Query<ProtocolQuery>,
) -> Result {
    let conversation_id = parse_conversation_id(&raw_id)?;
    if let Err(resp) = check_protocol(q.protocol.as_deref()) {
        return Ok(resp);
    }
    state.realtime.collab.touch_init();
    Ok(envelope::ok(json!({
        "viewers": state.realtime.collab.viewers(conversation_id),
    })))
}

/// POST /api/collaboration/conversations/{conversationId}/join
/// (CRD 2358-2367). Body is optional; identity comes from the session.
pub async fn join(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: Option<Json<Value>>,
) -> Result {
    let conversation_id = parse_conversation_id(&raw_id)?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    if let Err(resp) = check_protocol(body.get("protocol").and_then(Value::as_str)) {
        return Ok(resp);
    }
    let joined = state.realtime.collab.join(
        conversation_id,
        &user.id,
        &user.email,
        &user.display_name,
        &user.role,
    );
    if joined.is_err() {
        // Per-conversation capacity exceeded -> room-full, 403 (CRD 2366).
        return Ok(room_full_response());
    }
    emit(
        &state,
        conversation_id,
        &user.id,
        "user_joined",
        json!({
            "userId": user.id,
            "username": user.email,
            "displayName": user.display_name,
            "role": user.role,
            "conversationId": conversation_id,
        }),
    );
    Ok(envelope::with_status(
        StatusCode::OK,
        Some(Value::Null),
        Some("Joined conversation"),
    ))
}

fn room_full_response() -> Response {
    coded_error(
        StatusCode::FORBIDDEN,
        "ROOM_FULL",
        "Conversation room is full",
    )
}

/// POST /api/collaboration/conversations/{conversationId}/leave
/// (CRD 2369-2374). Idempotent.
pub async fn leave(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let conversation_id = parse_conversation_id(&raw_id)?;
    state.realtime.collab.leave(conversation_id, &user.id);
    emit(
        &state,
        conversation_id,
        &user.id,
        "user_left",
        json!({ "userId": user.id, "conversationId": conversation_id }),
    );
    Ok(envelope::with_status(
        StatusCode::OK,
        Some(Value::Null),
        Some("Left conversation"),
    ))
}

/// POST /api/collaboration/typing (CRD 2376-2382).
pub async fn typing(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<Value>>,
) -> Result {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let conversation_id = body_conversation_id(body.get("conversationId"));
    let status = body.get("status").and_then(Value::as_str);
    let (Some(conversation_id), Some(status)) = (conversation_id, status) else {
        return Err(AppError::BadRequest(
            "Missing required fields: conversationId and status".into(),
        ));
    };
    if status != "start" && status != "stop" {
        return Err(AppError::BadRequest(
            "Invalid status. Must be \"start\" or \"stop\"".into(),
        ));
    }
    let active = status == "start";
    state.realtime.collab.set_typing(
        conversation_id,
        &user.id,
        &user.email,
        &user.display_name,
        &user.role,
        active,
    );
    let event = if active {
        "typing_start"
    } else {
        "typing_stop"
    };
    emit(
        &state,
        conversation_id,
        &user.id,
        event,
        json!({
            "userId": user.id,
            "username": user.email,
            "displayName": user.display_name,
            "conversationId": conversation_id,
        }),
    );
    Ok(envelope::with_status(
        StatusCode::OK,
        Some(Value::Null),
        Some(&format!("Typing {status} indicator sent")),
    ))
}

/// POST /api/collaboration/presence (CRD 2384-2390).
pub async fn presence(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<Value>>,
) -> Result {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let Some(status) = body
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Err(AppError::BadRequest(
            "Missing required field: status".into(),
        ));
    };
    if !["online", "away", "busy", "offline"].contains(&status) {
        return Err(AppError::BadRequest(
            "Invalid status. Must be one of: online, away, busy, offline".into(),
        ));
    }
    let current = body_conversation_id(body.get("currentConversation"));
    let metadata = body.get("metadata").cloned().unwrap_or(Value::Null);
    state
        .realtime
        .collab
        .set_presence(&user.id, status, current, metadata);
    // Focused conversation supplied -> presence-update into that room
    // (CRD 2388, 2441).
    if let Some(conversation_id) = current {
        emit(
            &state,
            conversation_id,
            &user.id,
            "presence_update",
            json!({ "userId": user.id, "status": status, "conversationId": conversation_id }),
        );
    }
    Ok(envelope::with_status(
        StatusCode::OK,
        Some(Value::Null),
        Some("Presence updated"),
    ))
}

/// GET /api/collaboration/stats (CRD 2400-2404).
pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ProtocolQuery>,
) -> Result {
    if let Err(resp) = check_protocol(q.protocol.as_deref()) {
        return Ok(resp);
    }
    state.realtime.collab.touch_init();
    Ok(envelope::ok(state.realtime.collab.stats()))
}

/// POST /api/collaboration/cleanup — administrator only (CRD 2425-2429).
pub async fn cleanup(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Insufficient permissions".into()));
    }
    let cleaned = state.realtime.collab.cleanup();
    Ok(envelope::ok(json!({ "cleanedCount": cleaned })))
}

/// GET /api/collaboration/health — non-mutating, never initializes
/// (CRD 2431-2435).
pub async fn health(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let initialized = state.realtime.collab.is_initialized();
    let mut data = json!({
        "status": if initialized { "healthy" } else { "not_initialized" },
        "config": {
            "defaultProtocol": "websocket",
            "websocketEnabled": true,
        },
        "availableProtocols": ["websocket"],
        "timestamp": crate::db::now_iso(),
    });
    if !initialized {
        data["note"] = json!("Collaboration initializes lazily on first use; retry after activity");
    }
    Ok(envelope::ok(data))
}

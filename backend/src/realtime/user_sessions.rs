//! User real-time sessions (CRD §5.3 lines 3694-3845): the per-user session
//! WebSocket open contract, conversation subscriptions, presence heartbeat,
//! notification preferences, status/metrics snapshots, and the trusted
//! per-user broadcast / batch-events delivery operations.
//!
//! Mounted under `/api/realtime/session`; every HTTP operation is implicitly
//! scoped to the authenticated user (CRD 3697). The consolidated per-user
//! state snapshot (followed conversations, preferences, statistics, presence)
//! is persisted on relevant changes and restored when state is next needed,
//! so it survives full disconnects and restarts (CRD 3812-3815).

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;

use crate::domain::auth::{store, tokens};
use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::{authenticate, AuthUser, TEAM_CACHE_TTL};
use crate::state::AppState;

use super::hub::{ConnIdentity, RegisterError};
use super::socket::{can_view, run_socket};

// ----------------------------------------------------------- persistence

/// Restore the persisted user-state snapshot into the hub when no in-memory
/// state exists (CRD 3812, 3815).
pub async fn hydrate(state: &Arc<AppState>, user_id: &str) {
    if state.realtime.has_user_state(user_id) {
        return;
    }
    type StateRow = (Option<String>, String, Option<String>, Option<String>);
    let row: Option<StateRow> = sqlx::query_as(
        "SELECT last_seen, subscriptions, preferences, stats
         FROM realtime_user_state WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let Some((last_seen, subscriptions, preferences, stats)) = row else {
        return;
    };
    let subscriptions: Vec<String> = serde_json::from_str(&subscriptions).unwrap_or_default();
    let preferences = preferences.and_then(|p| serde_json::from_str(&p).ok());
    let stats = stats.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    state.realtime.hydrate_user(
        user_id,
        last_seen,
        subscriptions,
        preferences,
        stats.as_ref(),
    );
}

/// Upsert one user-state snapshot row (CRD 3815: re-persisted on relevant
/// changes). Best-effort: persistence failures never alter request outcomes.
pub async fn persist_snapshot(db: &PgPool, snapshot: &Value) {
    let Some(user_id) = snapshot["userId"].as_str() else {
        return;
    };
    if let Err(error) = sqlx::query(
        "INSERT INTO realtime_user_state
             (user_id, online, last_seen, subscriptions, preferences, stats, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT(user_id) DO UPDATE SET
             online = excluded.online,
             last_seen = excluded.last_seen,
             subscriptions = excluded.subscriptions,
             preferences = excluded.preferences,
             stats = excluded.stats,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(snapshot["online"].as_bool().unwrap_or(false) as i64)
    .bind(snapshot["lastSeen"].as_str())
    .bind(snapshot["subscriptions"].to_string())
    .bind(snapshot["preferences"].to_string())
    .bind(snapshot["stats"].to_string())
    .bind(crate::db::now_iso())
    .execute(db)
    .await
    {
        tracing::warn!(error = %error, user_id, "realtime user-state snapshot persist failed");
    }
}

/// Snapshot the user's current hub state and persist it.
pub async fn persist_user(state: &Arc<AppState>, user_id: &str) {
    let snapshot = state.realtime.user_state_snapshot(user_id);
    persist_snapshot(&state.db, &snapshot).await;
}

// ------------------------------------------------------- session websocket

/// Ceiling applied to scheduled close timers (~30 days).
const MAX_SCHEDULED_CLOSE_SECS: i64 = 30 * 24 * 60 * 60;

#[derive(Deserialize)]
pub struct SessionConnectQuery {
    pub token: Option<String>,
    pub role: Option<String>,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    #[serde(rename = "deviceId")]
    pub device_id: Option<String>,
    #[serde(rename = "tokenExpiry")]
    pub token_expiry: Option<i64>,
}

/// Open a real-time session — GET /api/realtime/session/websocket
/// (CRD 3701-3719). Parameter and error contract per the spec: missing
/// token/role -> 400 "Missing required parameters"; missing userId -> 400
/// "Missing userId parameter"; invalid token or identity mismatch -> 401
/// "Unauthorized"; session cap -> 429 "Connection limit reached".
pub async fn session_connect(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SessionConnectQuery>,
    ws: std::result::Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    // Presence of required parameters is validated first (CRD 3710).
    let (Some(token), Some(_role)) = (
        q.token.clone().filter(|s| !s.is_empty()),
        q.role.clone().filter(|s| !s.is_empty()),
    ) else {
        return AppError::BadRequest("Missing required parameters".into()).into_response();
    };
    let Some(user_id) = q.user_id.clone().filter(|s| !s.is_empty()) else {
        return AppError::BadRequest("Missing userId parameter".into()).into_response();
    };

    // The token must verify and belong to the named user (CRD 3709, 3716).
    let claims = match tokens::verify(&token, &state.config.jwt_secret) {
        Ok(c) => c,
        Err(_) => return AppError::unauthorized().into_response(),
    };
    if claims.sub != user_id {
        return AppError::unauthorized().into_response();
    }
    let agent = match store::find_agent_by_id(&state.db, &claims.sub).await {
        Ok(Some(a)) if a.is_active != 0 => a,
        Ok(_) => return AppError::unauthorized().into_response(),
        Err(_) => return AppError::Internal("WebSocket upgrade failed".into()).into_response(),
    };
    let teams = match state.team_cache.get(&agent.id, TEAM_CACHE_TTL) {
        Some(t) => t,
        None => match store::memberships(&state.db, &agent.id).await {
            Ok(t) => {
                state.team_cache.put(&agent.id, t.clone());
                t
            }
            Err(_) => return AppError::Internal("WebSocket upgrade failed".into()).into_response(),
        },
    };
    let identity = ConnIdentity {
        user_id: agent.id.clone(),
        email: agent.email.clone(),
        display_name: agent.display_name.clone(),
        role: agent.role.clone(),
        team_ids: teams.iter().map(|t| t.team_id).collect(),
    };

    let ws = match ws {
        Ok(ws) => ws,
        Err(_) => return AppError::BadRequest("WebSocket upgrade required".into()).into_response(),
    };

    // Restore persisted state before the session registers so the welcome
    // event carries the user's followed conversations, preferences and
    // statistics across reconnects (CRD 3812-3815, 3833).
    hydrate(&state, &identity.user_id).await;

    // Per-user simultaneous-session cap (CRD 3709, 3717, 3719: cap is 5).
    let registration = match state
        .realtime
        .register(identity.clone(), None, q.device_id.clone())
    {
        Ok(r) => r,
        Err(RegisterError::CeilingReached(_)) => {
            return AppError::TooManyRequests {
                message: "Connection limit reached".into(),
                retry_after: 30,
            }
            .into_response()
        }
    };
    if let Some(transition) = &registration.presence_transition {
        super::broadcaster::publish_remote_presence_change(&state, transition).await;
    }

    // Session metadata and the recomputed user state are persisted (CRD 3710).
    persist_user(&state, &identity.user_id).await;

    // A supplied positive token expiry schedules the forced close with the
    // refresh-and-reconnect close code (CRD 3708, 3719, 3827).
    let now = chrono::Utc::now().timestamp();
    let exp = match q.token_expiry {
        Some(t) if t > 0 => t,
        _ => claims.exp,
    }
    .min(now + MAX_SCHEDULED_CLOSE_SECS);

    ws.on_upgrade(move |socket| run_socket(state, socket, registration, identity, None, exp))
}

// ----------------------------------------------------------- HTTP surface

fn conversation_id(body: &Option<Json<Value>>) -> Result<String> {
    body.as_ref()
        .and_then(|Json(v)| v.get("conversationId"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::BadRequest("conversationId is required".into()))
}

/// Subscribe the user to a conversation — POST /connect, alias /subscribe
/// (CRD 3721-3731). Requires view permission; the add is silently capped at
/// the per-account subscription ceiling.
pub async fn subscribe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    let user = authenticate(&state, &headers).await?;
    let cid = conversation_id(&body)?;
    let team_ids: Vec<i64> = user.teams.iter().map(|t| t.team_id).collect();
    if !can_view(&state, &user.id, &user.role, &team_ids, &cid).await {
        return Err(AppError::Forbidden("Permission denied".into()));
    }
    hydrate(&state, &user.id).await;
    // A `None` here means the ceiling was reached: silently capped (CRD 3731).
    let count = state
        .realtime
        .subscribe(&user.id, &cid)
        .unwrap_or_else(|| state.realtime.subscription_count(&user.id));
    persist_user(&state, &user.id).await;
    // Fan the subscription change out to every live session (CRD 3725, 3834).
    state.realtime.to_user(
        &user.id,
        "conversation_subscribed",
        json!({ "conversationId": cid, "subscriptionCount": count }),
    );
    Ok(envelope::ok(
        json!({ "conversationId": cid, "subscriptionCount": count }),
    ))
}

/// Unsubscribe the user from a conversation — POST /disconnect, alias
/// /unsubscribe (CRD 3733-3741). No permission check; removing a conversation
/// that is not followed is a successful no-op.
pub async fn unsubscribe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    let user = authenticate(&state, &headers).await?;
    let cid = conversation_id(&body)?;
    hydrate(&state, &user.id).await;
    let count = state.realtime.unsubscribe(&user.id, &cid);
    persist_user(&state, &user.id).await;
    state.realtime.to_user(
        &user.id,
        "conversation_unsubscribed",
        json!({ "conversationId": cid, "subscriptionCount": count }),
    );
    Ok(envelope::ok(
        json!({ "conversationId": cid, "subscriptionCount": count }),
    ))
}

/// Update presence (heartbeat) — POST /presence (CRD 3743-3748).
pub async fn presence(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    _body: Option<Json<Value>>,
) -> Result {
    let user = authenticate(&state, &headers).await?;
    hydrate(&state, &user.id).await;
    let (online, last_seen) = state.realtime.heartbeat(&user.id);
    persist_user(&state, &user.id).await;
    Ok(envelope::ok(
        json!({ "online": online, "lastSeen": last_seen }),
    ))
}

/// Read notification preferences — GET /preferences (CRD 3750-3753).
pub async fn get_preferences(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    let user = authenticate(&state, &headers).await?;
    hydrate(&state, &user.id).await;
    Ok(envelope::ok(state.realtime.preferences(&user.id)))
}

/// Replace/merge notification preferences — PUT /preferences (CRD 3755-3760).
pub async fn put_preferences(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    let user = authenticate(&state, &headers).await?;
    let patch = body.map(|Json(v)| v).unwrap_or(json!({}));
    hydrate(&state, &user.id).await;
    let merged = state.realtime.merge_preferences(&user.id, &patch);
    persist_user(&state, &user.id).await;
    Ok(envelope::ok(merged))
}

/// Any other method on /preferences -> 405 "Method not allowed" (CRD 3760).
pub async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({
            "success": false,
            "error": "Method not allowed",
            "code": "METHOD_NOT_ALLOWED",
            "timestamp": crate::db::now_iso(),
        })),
    )
        .into_response()
}

/// Get connection status snapshot — GET /status (CRD 3762-3765).
pub async fn status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    let user = authenticate(&state, &headers).await?;
    hydrate(&state, &user.id).await;
    let snap = state.realtime.user_state_snapshot(&user.id);
    Ok(envelope::ok(json!({
        "userId": user.id,
        "online": snap["online"],
        "lastSeen": snap["lastSeen"],
        "sessionCount": snap["sessionCount"],
        "stats": snap["stats"],
        "subscriptionCount": snap["subscriptions"].as_array().map(Vec::len).unwrap_or(0),
    })))
}

/// Get metrics snapshot — GET /metrics (CRD 3767-3770).
pub async fn metrics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    let user = authenticate(&state, &headers).await?;
    hydrate(&state, &user.id).await;
    let snap = state.realtime.user_state_snapshot(&user.id);
    Ok(envelope::ok(json!({
        "userId": user.id,
        "online": snap["online"],
        "lastSeen": snap["lastSeen"],
        "sessionCount": snap["sessionCount"],
        "stats": snap["stats"],
        "uptimeSeconds": state.realtime.uptime_secs(),
        "subscriptionCount": snap["subscriptions"].as_array().map(Vec::len).unwrap_or(0),
    })))
}

/// Resolve the target user for the trusted delivery operations: the caller
/// itself, or — administrators only — an explicit `userId` in the body.
fn delivery_target(user: &AuthUser, body: &Value) -> Result<String> {
    match body
        .get("userId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(target) if target != user.id => {
            if !user.is_admin() {
                return Err(AppError::Forbidden("Administrator role required".into()));
            }
            Ok(target.to_string())
        }
        _ => Ok(user.id.clone()),
    }
}

/// Push a message to all of a user's sessions — POST /broadcast
/// (CRD 3772-3777). Non-open sessions are skipped silently.
pub async fn broadcast(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    let user = authenticate(&state, &headers).await?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let message = body
        .get("message")
        .cloned()
        .filter(|m| m.is_object())
        .ok_or_else(|| AppError::BadRequest("message is required".into()))?;
    let target = delivery_target(&user, &body)?;
    let event_type = message
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("event")
        .to_string();
    let payload = message.get("payload").cloned().unwrap_or(message.clone());
    let delivered = state.realtime.to_user(&target, &event_type, payload);
    Ok(envelope::ok(json!({ "delivered": delivered })))
}

/// Deliver batched events to a user — POST /batch-events (CRD 3779-3786):
/// global or cross-conversation events fanned out to every live session.
pub async fn batch_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    let user = authenticate(&state, &headers).await?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let Some(events) = body.get("events").and_then(Value::as_array).cloned() else {
        return Err(AppError::BadRequest("Invalid events format".into()));
    };
    let target = delivery_target(&user, &body)?;
    let mut delivered = 0u64;
    for event in &events {
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("event")
            .to_string();
        let mut payload = event.get("data").cloned().unwrap_or_else(|| event.clone());
        // The conversation association and timestamp are preserved; the
        // timestamp defaults to now (CRD 3782).
        if let Some(cid) = event.get("conversationId") {
            payload["conversationId"] = cid.clone();
        }
        payload["eventTimestamp"] = event
            .get("timestamp")
            .cloned()
            .unwrap_or_else(|| json!(crate::db::now_iso()));
        state.realtime.to_user(&target, &event_type, payload);
        delivered += 1;
    }
    state.realtime.note_messages_received(&target, delivered);
    persist_user(&state, &target).await;
    Ok(envelope::ok(json!({
        "eventsProcessed": delivered,
        "userId": target,
        "activeSessions": state.realtime.user_session_count(&target),
    })))
}

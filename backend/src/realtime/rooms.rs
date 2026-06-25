//! Conversation room surface (CRD §5.2 lines 3469-3577): the per-conversation
//! live WebSocket endpoint with its three authentication methods (bearer
//! token, single-use challenge + signature, simplified triple), the challenge
//! issuer, and the trusted room HTTP operations (connect status, forced
//! disconnect, event injection, participants, metrics).
//!
//! Mounted under `/api/realtime/rooms/{conversationId}`. A room's mode (full
//! vs simplified, CRD 3479) is fixed at creation: the first request that
//! touches a room may pass `mode=simplified`; rooms default to full-featured.
//!
//! TODO(scale-out): a single conversation served by multiple instances
//! (CRD 3542, 3667) — single-process delivery is the observable equivalent:
//! every participant is reached exactly once and per-room order is preserved.

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;

use crate::domain::auth::tokens;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::authenticate;
use crate::state::AppState;

use super::hub::{ConnIdentity, RegisterError};
use super::socket::run_socket;
use super::user_sessions;

/// Ceiling applied to scheduled close timers so a missing/absurd expiry never
/// overflows the timer wheel (~30 days).
const MAX_SCHEDULED_CLOSE_SECS: i64 = 30 * 24 * 60 * 60;

/// Keyed signature of the challenge identifier and the bound credential
/// (CRD 3268: HMAC over `<challengeId>.<token>` with the service secret).
pub fn challenge_signature(secret: &str, challenge_id: &str, token: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(challenge_id.as_bytes());
    mac.update(b".");
    mac.update(token.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Deserialize)]
pub struct RoomConnectQuery {
    pub token: Option<String>,
    #[serde(rename = "challengeId")]
    pub challenge_id: Option<String>,
    pub signature: Option<String>,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    pub role: Option<String>,
    #[serde(rename = "tokenExpiry")]
    pub token_expiry: Option<i64>,
    /// Sets the room's mode when this request creates the room (CRD 3479).
    pub mode: Option<String>,
    #[serde(rename = "deviceId")]
    pub device_id: Option<String>,
}

/// Establish a live connection to a conversation room (CRD 3478-3509).
/// GET /api/realtime/rooms/{conversationId}/websocket
pub async fn room_connect(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    Query(q): Query<RoomConnectQuery>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    // The room's mode is fixed at creation time (CRD 3479).
    let mode = state.realtime.ensure_room(&conversation_id, q.mode.as_deref());

    // 1. Authenticate per the mode; failures reject before any socket exists
    //    (CRD 3493, 3502-3506).
    let (identity, exp) = if mode == "simplified" {
        // Simplified mode: user identifier, token and role all required; the
        // role is taken at face value, the token is not verified (CRD 3485).
        let (Some(user_id), Some(_token), Some(role)) = (
            q.user_id.clone().filter(|s| !s.is_empty()),
            q.token.clone().filter(|s| !s.is_empty()),
            q.role.clone().filter(|s| !s.is_empty()),
        ) else {
            return AppError::BadRequest("userId, token and role are required".into())
                .into_response();
        };
        let identity = ConnIdentity {
            user_id: user_id.clone(),
            email: String::new(),
            display_name: user_id,
            role,
            team_ids: Vec::new(),
        };
        (identity, chrono::Utc::now().timestamp() + MAX_SCHEDULED_CLOSE_SECS)
    } else if let Some(token) = q.token.clone().filter(|s| !s.is_empty()) {
        // Full mode, bearer token: identity and role come from the verified
        // credential (CRD 3488).
        let claims = match tokens::verify(&token, &state.config.jwt_secret) {
            Ok(c) => c,
            Err(_) => return AppError::Unauthorized("Invalid token".into()).into_response(),
        };
        let identity = ConnIdentity {
            user_id: claims.sub.clone(),
            email: claims.email.clone().unwrap_or_default(),
            display_name: claims.name.clone().unwrap_or_else(|| claims.sub.clone()),
            role: claims.role.clone(),
            team_ids: Vec::new(),
        };
        (identity, claims.exp)
    } else if let (Some(challenge_id), Some(signature)) =
        (q.challenge_id.clone(), q.signature.clone())
    {
        // Full mode, challenge-response: single-use, unexpired, signature must
        // match the keyed signature for that challenge (CRD 3489, 3505).
        let Some(challenge) = state.realtime.consume_challenge(&conversation_id, &challenge_id)
        else {
            return AppError::Unauthorized("Invalid challenge response".into()).into_response();
        };
        let expected =
            challenge_signature(&state.config.jwt_secret, &challenge_id, &challenge.token);
        if signature != expected {
            return AppError::Unauthorized("Invalid challenge response".into()).into_response();
        }
        let identity = ConnIdentity {
            user_id: challenge.user_id.clone(),
            email: String::new(),
            display_name: challenge.display_name.clone(),
            role: challenge.role.clone(),
            team_ids: Vec::new(),
        };
        (identity, challenge.token_exp)
    } else {
        return AppError::BadRequest(
            "Either a token or challengeId and signature are required".into(),
        )
        .into_response();
    };

    // Must be a protocol-upgrade request (CRD 3479).
    let ws = match ws {
        Ok(ws) => ws,
        Err(_) => {
            return AppError::BadRequest("WebSocket upgrade required".into()).into_response()
        }
    };

    // 2. Capacity is enforced strictly before acceptance (CRD 3491, 3507).
    let registration = match state.realtime.register(
        identity.clone(),
        Some(conversation_id.clone()),
        q.device_id.clone(),
    ) {
        Ok(r) => r,
        Err(RegisterError::CeilingReached(_)) => {
            return AppError::TooManyRequests {
                message: "Connection limit reached".into(),
                retry_after: 30,
            }
            .into_response()
        }
    };

    // Optional explicit token expiry schedules the forced close (CRD 3486,
    // 3499); otherwise the credential's own expiry applies.
    let now = chrono::Utc::now().timestamp();
    let exp = match q.token_expiry {
        Some(t) if t > 0 => t,
        _ => exp,
    }
    .min(now + MAX_SCHEDULED_CLOSE_SECS);

    ws.on_upgrade(move |socket| {
        run_socket(state, socket, registration, identity, Some(conversation_id), exp)
    })
}

/// Generate an authentication challenge — POST .../challenge (CRD 3511-3518).
/// Full-feature rooms only; in simplified mode the route does not exist.
pub async fn challenge(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if state.realtime.ensure_room(&conversation_id, None) == "simplified" {
        return Err(AppError::NotFound(
            "Challenge endpoint is not available in simplified mode".into(),
        ));
    }
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::to_string)
        .ok_or_else(|| AppError::Unauthorized("Authentication required".into()))?;
    let claims = tokens::verify(&token, &state.config.jwt_secret)
        .map_err(|_| AppError::Unauthorized("Invalid or expired token".into()))?;
    if claims.sub.trim().is_empty() {
        return Err(AppError::Unauthorized("Invalid or expired token".into()));
    }
    let display_name = claims.name.clone().unwrap_or_else(|| claims.sub.clone());
    let (id, expires_at) = state.realtime.create_challenge(
        &conversation_id,
        &claims.sub,
        &claims.role,
        &display_name,
        &token,
        claims.exp,
    );
    Ok(envelope::ok(json!({
        "challengeId": id,
        "expiresAt": expires_at,
        "ttlMs": super::hub::CHALLENGE_TTL.as_millis() as u64,
    })))
}

/// Connection acknowledgement / status — POST .../connect (CRD 3520-3522).
pub async fn connect_status(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    authenticate(&state, &headers).await?;
    let info = state.realtime.room_info(&conversation_id);
    Ok(envelope::ok(json!({
        "conversationId": conversation_id,
        "activeConnections": info["activeConnections"],
        "mode": state.realtime.room_mode(&conversation_id),
    })))
}

/// Force-disconnect a connection — POST .../disconnect (CRD 3524-3528).
/// Always reports success; an unknown connection identifier is a no-op.
pub async fn force_disconnect(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result<Response, AppError> {
    let user = authenticate(&state, &headers).await?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let connection_id = body
        .get("connectionId")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("connectionId is required".into()))?;
    if let Some(snapshot) =
        state.realtime.remove_connection(connection_id, &user.id, user.is_admin())
    {
        user_sessions::persist_snapshot(&state.db, &snapshot).await;
    }
    Ok(envelope::ok(json!({
        "conversationId": conversation_id,
        "connectionId": connection_id,
        "disconnectedAt": crate::db::now_iso(),
    })))
}

/// Inject a broadcast event into the room — POST .../broadcast (CRD 3530-3534).
pub async fn broadcast(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result<Response, AppError> {
    let user = authenticate(&state, &headers).await?;
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let event = body
        .map(|Json(v)| v)
        .filter(|v| v.is_object())
        .ok_or_else(|| AppError::BadRequest("event body is required".into()))?;
    let delivered = state.realtime.room_broadcast_raw(&conversation_id, event.clone());
    super::broadcaster::publish_remote_room_broadcast(&state, &conversation_id, &event).await;
    Ok(envelope::ok(json!({ "delivered": delivered })))
}

/// List participants — POST .../participants (CRD 3536-3537).
pub async fn participants(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    authenticate(&state, &headers).await?;
    Ok(envelope::ok(state.realtime.room_info(&conversation_id)))
}

/// Room metrics — POST .../metrics (CRD 3539-3540).
pub async fn room_metrics(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    authenticate(&state, &headers).await?;
    Ok(envelope::ok(state.realtime.room_metrics_snapshot(&conversation_id)))
}

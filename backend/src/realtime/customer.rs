//! Customer-Side Real-time Channels (CRD §5.4, lines 3847-3974).
//!
//! Per-conversation live channels for the customer-support side: the channel
//! WebSocket (with pre-validated fast path and session-store fallback), the
//! notify-message / notify-message-updated fan-out triggers, the header-driven
//! message list/create endpoints, and the file-asset upload. Channel frames
//! are raw top-level JSON events (`new_message`, `message_updated`,
//! `USER_CONNECTED`, `USER_DISCONNECTED`) — not hub-framed — matching the
//! section's documented payload shapes (CRD 3962-3972).
//!
//! Mounted under `/api/customer-channel`; the §2.3 `/api/customer-ws` upgrade
//! target registers into the same channel registry.

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Multipart, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::mpsc;

use crate::domain::conversations::channels::{OutboundGateway, OutboundItem, BATCH_CAP};
use crate::state::AppState;

const REMOTE_FANOUT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
const REMOTE_FANOUT_BATCH: i64 = 100;
const REMOTE_FANOUT_RETENTION: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);
const REMOTE_FANOUT_CLEANUP_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(60 * 60);

// ------------------------------------------------------------ channel registry

struct CustConn {
    user_id: String,
    tx: mpsc::UnboundedSender<String>,
}

/// Per-conversation live-connection registry (CRD 3949): connections are
/// tracked individually (one user may hold several), exist only while the
/// socket is open, and are pruned when a send fails.
#[derive(Default)]
pub struct CustomerChannels {
    inner: Mutex<HashMap<String, HashMap<String, CustConn>>>,
}

impl CustomerChannels {
    /// Register a connection under a freshly generated unique identifier and
    /// broadcast a presence "connected" event to the channel's *other*
    /// connections (CRD 3864, 3966).
    pub fn connect(&self, conversation_id: &str, user_id: &str) -> (String, mpsc::UnboundedReceiver<String>) {
        let connection_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::unbounded_channel();
        let presence = json!({
            "type": "USER_CONNECTED",
            "userId": user_id,
            "timestamp": crate::db::now_iso(),
        })
        .to_string();
        let mut inner = self.inner.lock().expect("customer channels lock");
        let channel = inner.entry(conversation_id.to_string()).or_default();
        for conn in channel.values() {
            let _ = conn.tx.send(presence.clone());
        }
        channel.insert(connection_id.clone(), CustConn { user_id: user_id.to_string(), tx });
        (connection_id, rx)
    }

    /// Remove a connection. A presence "disconnected" event fires only when
    /// the user holds no other remaining connections on the channel
    /// (CRD 3959, 3967).
    pub fn disconnect(&self, conversation_id: &str, connection_id: &str) {
        let mut inner = self.inner.lock().expect("customer channels lock");
        let Some(channel) = inner.get_mut(conversation_id) else { return };
        let Some(removed) = channel.remove(connection_id) else { return };
        let user_still_connected = channel.values().any(|c| c.user_id == removed.user_id);
        if !user_still_connected {
            let presence = json!({
                "type": "USER_DISCONNECTED",
                "userId": removed.user_id,
                "timestamp": crate::db::now_iso(),
            })
            .to_string();
            for conn in channel.values() {
                let _ = conn.tx.send(presence.clone());
            }
        }
        if channel.is_empty() {
            inner.remove(conversation_id);
        }
    }

    /// Push one raw event frame to every open connection; connections whose
    /// send fails are pruned (CRD 3876). Returns the delivery count.
    pub fn broadcast(&self, conversation_id: &str, event: &Value) -> usize {
        let frame = event.to_string();
        let mut inner = self.inner.lock().expect("customer channels lock");
        let Some(channel) = inner.get_mut(conversation_id) else { return 0 };
        let mut dead: Vec<String> = Vec::new();
        let mut delivered = 0usize;
        for (id, conn) in channel.iter() {
            if conn.tx.send(frame.clone()).is_ok() {
                delivered += 1;
            } else {
                dead.push(id.clone());
            }
        }
        for id in dead {
            channel.remove(&id);
        }
        delivered
    }

    /// (total connection count, distinct connected user ids) for the
    /// diagnostic block of the notify endpoint (CRD 3877).
    pub fn snapshot(&self, conversation_id: &str) -> (usize, Vec<String>) {
        let inner = self.inner.lock().expect("customer channels lock");
        let Some(channel) = inner.get(conversation_id) else { return (0, Vec::new()) };
        let mut users: Vec<String> = Vec::new();
        for conn in channel.values() {
            if !users.contains(&conn.user_id) {
                users.push(conn.user_id.clone());
            }
        }
        users.sort();
        (channel.len(), users)
    }
}

/// Publish a raw customer-channel event locally and to the cross-instance
/// relay. The database write is best-effort so a fan-out storage hiccup cannot
/// block the request path that already delivered to this process.
async fn publish_customer_event(state: &AppState, conversation_id: &str, event: &Value) {
    let event_id = uuid::Uuid::new_v4().to_string();
    if let Err(err) = sqlx::query(
        "INSERT INTO realtime_customer_fanout_events
         (id, source_instance, conversation_id, event, created_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(event_id)
    .bind(state.realtime.instance_id())
    .bind(conversation_id)
    .bind(event.to_string())
    .bind(crate::db::now_iso())
    .execute(&state.db)
    .await
    {
        tracing::warn!(error = %err, conversation_id, "customer-channel fanout publish failed");
    }
}

pub(crate) async fn broadcast_customer_event(
    state: &AppState,
    conversation_id: &str,
    event: &Value,
) -> usize {
    let delivered = state.realtime.customers.broadcast(conversation_id, event);
    publish_customer_event(state, conversation_id, event).await;
    delivered
}

#[derive(sqlx::FromRow)]
struct RemoteCustomerEvent {
    id: String,
    conversation_id: String,
    event: String,
}

/// Deliver peer-instance customer-channel events into this process's in-memory
/// registry and ack them once processed. Invalid persisted payloads are acked
/// after logging so a bad row cannot permanently block a receiver.
pub async fn process_remote_customer_events(
    state: &AppState,
    limit: i64,
) -> Result<usize, sqlx::Error> {
    let rows = sqlx::query_as::<_, RemoteCustomerEvent>(
        "SELECT e.id, e.conversation_id, e.event
         FROM realtime_customer_fanout_events e
         WHERE e.source_instance <> $1
           AND NOT EXISTS (
             SELECT 1 FROM realtime_customer_fanout_acks a
             WHERE a.event_id = e.id AND a.instance_id = $1
           )
         ORDER BY e.created_at ASC
         LIMIT $2",
    )
    .bind(state.realtime.instance_id())
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let mut processed = 0usize;
    for row in rows {
        match serde_json::from_str::<Value>(&row.event) {
            Ok(event) => {
                state.realtime.customers.broadcast(&row.conversation_id, &event);
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    event_id = %row.id,
                    "customer-channel fanout payload was not valid JSON"
                );
            }
        }
        sqlx::query(
            "INSERT INTO realtime_customer_fanout_acks (event_id, instance_id, acked_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (event_id, instance_id) DO NOTHING",
        )
        .bind(&row.id)
        .bind(state.realtime.instance_id())
        .bind(crate::db::now_iso())
        .execute(&state.db)
        .await?;
        processed += 1;
    }
    Ok(processed)
}

pub async fn cleanup_remote_customer_events(
    state: &AppState,
    older_than: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM realtime_customer_fanout_events WHERE created_at < $1")
        .bind(older_than)
        .execute(&state.db)
        .await?;
    Ok(result.rows_affected())
}

fn remote_fanout_cutoff(retention: std::time::Duration) -> String {
    let retention =
        chrono::Duration::from_std(retention).unwrap_or_else(|_| chrono::Duration::hours(24));
    (chrono::Utc::now() - retention).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn spawn_remote_fanout_loop(state: Arc<AppState>) {
    let state: Weak<AppState> = Arc::downgrade(&state);
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(REMOTE_FANOUT_INTERVAL);
        loop {
            ticker.tick().await;
            let Some(state) = state.upgrade() else {
                break;
            };
            if let Err(err) = process_remote_customer_events(&state, REMOTE_FANOUT_BATCH).await {
                tracing::warn!(error = %err, "customer-channel remote fanout loop failed");
            }
        }
    });
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(REMOTE_FANOUT_CLEANUP_INTERVAL);
        loop {
            ticker.tick().await;
            let Some(state) = cleanup_state.upgrade() else {
                break;
            };
            let cutoff = remote_fanout_cutoff(REMOTE_FANOUT_RETENTION);
            if let Err(err) = cleanup_remote_customer_events(&state, &cutoff).await {
                tracing::warn!(error = %err, "customer-channel remote fanout cleanup failed");
            }
        }
    });
}

/// Drive one accepted customer-channel socket: forward channel broadcasts;
/// inbound client frames are accepted but produce no observable effect
/// (CRD 3871, 3972); deregister on close or error.
pub async fn run_customer_socket(
    state: Arc<AppState>,
    socket: WebSocket,
    conversation_id: String,
    connection_id: String,
    mut rx: mpsc::UnboundedReceiver<String>,
) {
    let (mut sink, mut stream) = socket.split();
    loop {
        tokio::select! {
            out = rx.recv() => {
                match out {
                    Some(text) => {
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            inbound = stream.next() => {
                match inbound {
                    // Reserved for future use (typing indicators / read
                    // receipts): received, never acted upon (CRD 3871).
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
    state.realtime.customers.disconnect(&conversation_id, &connection_id);
}

// ------------------------------------------------------------------- helpers

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "success": false, "error": message }))).into_response()
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

fn header<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|v| v.to_str().ok()).filter(|v| !v.is_empty())
}

/// Conversation-identifier header for the message API (CRD 3892).
const CONVERSATION_HEADER: &str = "x-conversation-id";
/// Session/credential header for the message API (CRD 3907, 3930).
const SESSION_HEADER: &str = "x-session-token";

// -------------------------------------- channel websocket (CRD 3854-3871)

#[derive(Deserialize)]
pub struct ChannelWsQuery {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "preValidated")]
    pub pre_validated: Option<String>,
    #[serde(rename = "validatedUserId")]
    pub validated_user_id: Option<String>,
    #[serde(rename = "validatedRole")]
    pub validated_role: Option<String>,
    #[serde(rename = "validatedUsername")]
    pub validated_username: Option<String>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

/// GET /api/customer-channel/ws — open the per-conversation channel
/// (CRD 3854-3871). Fast path: `preValidated=true` plus a validated user id
/// trusts the supplied identity; fallback: the session token is resolved
/// against the session store.
pub async fn channel_ws(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ChannelWsQuery>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    // Upgrade header absent -> plain 400 (CRD 3868).
    let ws = match ws {
        Ok(ws) => ws,
        Err(_) => return plain(StatusCode::BAD_REQUEST, "Expected WebSocket upgrade request"),
    };
    let Some(conversation_id) = q.conversation_id.filter(|c| !c.is_empty()) else {
        return plain(StatusCode::BAD_REQUEST, "Conversation ID is required");
    };

    let fast_path = q.pre_validated.as_deref() == Some("true")
        && q.validated_user_id.as_deref().is_some_and(|u| !u.is_empty());
    let user_id = if fast_path {
        // Identity accepted as-is; role defaults to agent, label to a generic
        // one (CRD 3859, 3862) — only the user id is observable in events.
        let _ = (q.validated_role.as_deref().unwrap_or("agent"), q.validated_username.as_deref().unwrap_or("User"));
        q.validated_user_id.clone().unwrap_or_default()
    } else {
        // Fallback path: session-store lookup (CRD 3863, 3869-3870).
        let Some(session_id) = q.session_id.filter(|s| !s.is_empty()) else {
            return plain(StatusCode::BAD_REQUEST, "Session ID is required");
        };
        type SessionRow = (String, Option<String>, String);
        let row: Option<SessionRow> = match sqlx::query_as(
            "SELECT agent_id, data, expires_at FROM auth_sessions WHERE id = $1",
        )
        .bind(&session_id)
        .fetch_optional(&state.db)
        .await
        {
            Ok(v) => v,
            Err(_) => return fail(StatusCode::UNAUTHORIZED, "Session lookup failed"),
        };
        let Some((agent_id, data, expires_at)) = row else {
            return fail(StatusCode::UNAUTHORIZED, "Session not found");
        };
        if expires_at <= crate::db::now_iso() {
            return fail(StatusCode::UNAUTHORIZED, "Session expired");
        }
        // A stored session in an unreadable form also rejects (CRD 3863).
        let parsed = match data {
            None => Some(Value::Null),
            Some(raw) => serde_json::from_str::<Value>(&raw).ok(),
        };
        let Some(profile) = parsed else {
            return fail(StatusCode::UNAUTHORIZED, "Session data unreadable");
        };
        profile["userId"].as_str().map(str::to_string).unwrap_or(agent_id)
    };

    ws.on_upgrade(move |socket| async move {
        let (connection_id, rx) = state.realtime.customers.connect(&conversation_id, &user_id);
        run_customer_socket(state, socket, conversation_id, connection_id, rx).await;
    })
}

// ------------------------------------- notify endpoints (CRD 3873-3887)

/// POST /api/customer-channel/notify-message — fan a created message out to
/// connected viewers (CRD 3873-3879).
pub async fn notify_message(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let Some(conversation_id) = body["conversationId"].as_str().map(str::to_string).or_else(|| {
        body["conversationId"].as_i64().map(|n| n.to_string())
    }) else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "conversationId is missing");
    };
    let message = body.get("message").cloned().unwrap_or(json!({}));
    let now = crate::db::now_iso();
    // Event shape per CRD 3968: lowercase type marker, top-level data object,
    // original message copy for backward compatibility, platform defaulting
    // to LINE.
    let event = json!({
        "type": "new_message",
        "conversationId": conversation_id,
        "data": {
            "conversationId": conversation_id,
            "content": message.get("content").cloned().unwrap_or(Value::Null),
            "messageType": message.get("messageType").cloned().unwrap_or(Value::Null),
            "senderType": message.get("senderType").cloned().unwrap_or(Value::Null),
            "senderId": message.get("senderId").cloned().unwrap_or(Value::Null),
            "platform": message.get("platform").cloned().unwrap_or(json!("line")),
            "timestamp": now,
        },
        "message": message,
        "timestamp": now,
    });
    broadcast_customer_event(&state, &conversation_id, &event).await;
    let (total, users) = state.realtime.customers.snapshot(&conversation_id);
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "debug": {
                "totalConnections": total,
                "connectedUsers": users,
                "conversationId": conversation_id,
            },
        })),
    )
        .into_response()
}

/// POST /api/customer-channel/notify-message-updated — deferred-media
/// completion fan-out (CRD 3881-3887).
pub async fn notify_message_updated(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let conversation_id = body["conversationId"].as_str().map(str::to_string).or_else(|| {
        body["conversationId"].as_i64().map(|n| n.to_string())
    });
    let message_id = body["messageId"].as_str().map(str::to_string).or_else(|| {
        body["messageId"].as_i64().map(|n| n.to_string())
    });
    let (Some(conversation_id), Some(message_id)) = (conversation_id, message_id) else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "conversationId and messageId are missing");
    };
    let mut data = json!({ "conversationId": conversation_id, "messageId": message_id });
    if let Some(extra) = body.get("data").and_then(Value::as_object) {
        for (k, v) in extra {
            data[k.as_str()] = v.clone();
        }
    }
    let event = json!({
        "type": "message_updated",
        "conversationId": conversation_id,
        "data": data,
        "timestamp": crate::db::now_iso(),
    });
    broadcast_customer_event(&state, &conversation_id, &event).await;
    (StatusCode::OK, Json(json!({ "success": true }))).into_response()
}

// ---------------------------------------- message listing (CRD 3889-3901)

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<String>,
    pub before: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: String,
    conversation_id: String,
    sender_type: String,
    customer_id: Option<i64>,
    agent_id: Option<String>,
    content: Option<String>,
    content_type: String,
    is_sent: i64,
    delivery_status: String,
    sender_name: Option<String>,
    metadata: Option<String>,
    created_at: String,
}

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    id: String,
    message_id: Option<String>,
    file_name: Option<String>,
    content_type: Option<String>,
    file_size: Option<i64>,
    file_url: Option<String>,
    storage_key: Option<String>,
}

/// Inline link plus, when the stored binary exists, a force-download link;
/// minting failure degrades to inline-only (CRD 3896, 3952).
fn attachment_view(a: &AttachmentRow, upload_dir: &str) -> Value {
    let download_url = a
        .storage_key
        .as_deref()
        .filter(|key| std::path::Path::new(upload_dir).join(key).exists())
        .and(a.file_url.as_deref())
        .map(|u| format!("{u}?download=1"));
    json!({
        "id": a.id,
        "messageId": a.message_id,
        "fileName": a.file_name,
        "contentType": a.content_type,
        "fileSize": a.file_size,
        "url": a.file_url,
        "downloadUrl": download_url,
    })
}

fn message_view(m: &MessageRow, attachments: Vec<Value>) -> Value {
    // Unified sender identifier resolved from whichever sender slot applies
    // (CRD 3897, 3951).
    let sender_id: Value = match (&m.agent_id, m.customer_id) {
        (Some(aid), _) => json!(aid),
        (None, Some(cid)) => json!(cid.to_string()),
        _ => Value::Null,
    };
    json!({
        "id": m.id,
        "conversationId": m.conversation_id,
        "senderType": m.sender_type,
        "senderId": sender_id,
        "customerId": m.customer_id,
        "agentId": m.agent_id,
        "content": m.content,
        "messageType": m.content_type,
        "isSent": m.is_sent != 0,
        "deliveryStatus": m.delivery_status,
        "senderName": m.sender_name,
        "metadata": m.metadata.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
        "createdAt": m.created_at,
        "attachments": attachments,
    })
}

/// GET /api/customer-channel/messages — newest-first cursor pagination
/// (CRD 3889-3901). The conversation travels in a dedicated header.
pub async fn list_messages(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> Response {
    let headers = headers.clone();
    let Some(conversation_id) = header(&headers, CONVERSATION_HEADER).map(str::to_string) else {
        return fail(StatusCode::BAD_REQUEST, "Conversation ID header is required");
    };
    let limit = q.limit.as_deref().and_then(|v| v.parse::<i64>().ok()).filter(|v| *v > 0).unwrap_or(50);

    // "before" cursor: strictly older than the anchor message; an
    // unresolvable cursor degrades to the latest page (CRD 3896).
    let mut clause = String::from("conversation_id = ? AND deleted_at IS NULL");
    let mut binds: Vec<String> = vec![conversation_id.clone()];
    if let Some(before) = q.before.as_deref().filter(|s| !s.is_empty()) {
        let anchor: Option<String> = match sqlx::query_scalar(
            "SELECT created_at FROM messages WHERE id = $1 AND conversation_id = $2",
        )
        .bind(before)
        .bind(&conversation_id)
        .fetch_optional(&state.db)
        .await
        {
            Ok(v) => v,
            Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch messages"),
        };
        if let Some(at) = anchor.filter(|a| !a.is_empty()) {
            clause.push_str(" AND created_at < ?");
            binds.push(at);
        }
    }

    let sql = format!(
        "SELECT id, conversation_id, sender_type, customer_id, agent_id, content, content_type,
                is_sent, delivery_status, sender_name, metadata, created_at
         FROM messages WHERE {clause}
         ORDER BY created_at DESC, id DESC LIMIT $1"
    );
    let sql = crate::db::pg_params(&sql);
    let mut mq = sqlx::query_as::<_, MessageRow>(&sql);
    for b in &binds {
        mq = mq.bind(b);
    }
    let rows = match mq.bind(limit).fetch_all(&state.db).await {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch messages"),
    };

    let mut by_message: HashMap<String, Vec<Value>> = HashMap::new();
    if !rows.is_empty() {
        let placeholders = vec!["?"; rows.len()].join(", ");
        let sql = format!(
            "SELECT id, message_id, file_name, content_type, file_size, file_url, storage_key
             FROM attachments WHERE message_id IN ({placeholders})"
        );
        let sql = crate::db::pg_params(&sql);
        let mut aq = sqlx::query_as::<_, AttachmentRow>(&sql);
        for r in &rows {
            aq = aq.bind(&r.id);
        }
        match aq.fetch_all(&state.db).await {
            Ok(attachments) => {
                for a in attachments {
                    if let Some(mid) = a.message_id.clone() {
                        by_message
                            .entry(mid)
                            .or_default()
                            .push(attachment_view(&a, &state.config.upload_dir));
                    }
                }
            }
            Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch messages"),
        }
    }

    // "has more" is purely derived from a full page (CRD 3897, 3901).
    let has_more = rows.len() as i64 == limit;
    let messages: Vec<Value> = rows
        .iter()
        .map(|m| message_view(m, by_message.remove(&m.id).unwrap_or_default()))
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "success": true, "messages": messages, "hasMore": has_more })),
    )
        .into_response()
}

// -------------------------------------- message creation (CRD 3903-3924)

/// Resolve the caller identity from the credential header (CRD 3907): a
/// three-part signed token yields the user id and display label from its
/// decoded middle segment; anything else is treated as the user id itself.
fn identity_from_credential(token: &str) -> (String, Option<String>) {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() == 3 {
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1].as_bytes())
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if let Some(claims) = decoded {
            let sub = claims["sub"]
                .as_str()
                .or_else(|| claims["userId"].as_str())
                .map(str::to_string);
            let name = claims["name"]
                .as_str()
                .or_else(|| claims["username"].as_str())
                .or_else(|| claims["displayName"].as_str())
                .map(str::to_string);
            if let Some(sub) = sub {
                return (sub, name);
            }
        }
    }
    (token.to_string(), None)
}

#[derive(Deserialize, Default)]
pub struct CreateBody {
    pub content: Option<String>,
    pub assets: Option<Value>,
    #[serde(rename = "attachmentIds")]
    pub attachment_ids: Option<Vec<String>>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    pub platform: Option<String>,
    #[serde(rename = "correlationId")]
    pub correlation_id: Option<String>,
}

/// POST /api/customer-channel/messages — create an outbound agent message
/// (CRD 3903-3924): persist, link attachments, advance conversation recency,
/// relay over LINE when applicable, fan out to the channel and emit the
/// global conversation-list notification.
pub async fn create_message(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<CreateBody>>,
) -> Response {
    let headers = headers.clone();
    let Some(conversation_id) = header(&headers, CONVERSATION_HEADER).map(str::to_string) else {
        return fail(StatusCode::BAD_REQUEST, "Conversation ID header is required");
    };
    let Some(credential) = header(&headers, SESSION_HEADER).map(str::to_string) else {
        return fail(StatusCode::UNAUTHORIZED, "Session token header is required");
    };
    let body = body.map(|Json(b)| b).unwrap_or_default();

    let content = body.content.as_deref().unwrap_or("").trim().to_string();
    let attachment_ids = body.attachment_ids.unwrap_or_default();
    // At least one of content or attachments (CRD 3909, 3922).
    if content.is_empty() && attachment_ids.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "Content or attachments are required");
    }
    // Attachments force the file kind (CRD 3911).
    let message_type = if attachment_ids.is_empty() {
        body.message_type.unwrap_or_else(|| "text".to_string())
    } else {
        "file".to_string()
    };

    let (user_id, token_name) = identity_from_credential(&credential);
    // The agent reference column is only populated for a real agent record;
    // the display label is captured as a snapshot (CRD 3911).
    let agent: Option<(String, String)> = match sqlx::query_as(
        "SELECT id, display_name FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create message"),
    };
    let agent_id = agent.as_ref().map(|(id, _)| id.clone());
    let sender_name = token_name
        .or_else(|| agent.as_ref().map(|(_, n)| n.clone()))
        .unwrap_or_else(|| user_id.clone());

    let metadata = json!({
        "assets": body.assets,
        "attachmentIds": attachment_ids,
        "platform": body.platform.as_deref().unwrap_or("line"),
        "correlationId": body.correlation_id,
        "senderId": user_id,
    });

    // Persisted already sent and delivered (CRD 3911, 3960).
    let message_id = uuid::Uuid::new_v4().to_string();
    let now = crate::db::now_iso();
    let insert = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content,
                                   content_type, is_sent, sent_at, delivery_status, metadata,
                                   sender_name, created_at)
             VALUES ($1, $2, 'agent', $3, $4, $5, 1, $6, 'delivered', $7, $8, $9)",
        )
        .bind(&message_id)
        .bind(&conversation_id)
        .bind(&agent_id)
        .bind(if content.is_empty() { None } else { Some(content.clone()) })
        .bind(&message_type)
        .bind(&now)
        .bind(metadata.to_string())
        .bind(&sender_name)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        if !attachment_ids.is_empty() {
            let placeholders = vec!["?"; attachment_ids.len()].join(", ");
            let sql = format!(
                "UPDATE attachments SET message_id = $1
                 WHERE id IN ({placeholders}) AND message_id IS NULL"
            );
            let sql = crate::db::pg_params(&sql);
            let mut q = sqlx::query(&sql).bind(&message_id);
            for aid in &attachment_ids {
                q = q.bind(aid);
            }
            q.execute(&mut *tx).await?;
        }
        // Recency markers advance so the conversation re-sorts to the top
        // (CRD 3913).
        sqlx::query("UPDATE conversations SET last_message_at = $1, updated_at = $2 WHERE id = $3")
            .bind(&now)
            .bind(&now)
            .bind(&conversation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }
    .await;
    if insert.is_err() {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create message");
    }

    let attachments: Vec<Value> = if attachment_ids.is_empty() {
        Vec::new()
    } else {
        let placeholders = vec!["?"; attachment_ids.len()].join(", ");
        let sql = format!(
            "SELECT id, message_id, file_name, content_type, file_size, file_url, storage_key
             FROM attachments WHERE message_id = $1 AND id IN ({placeholders})"
        );
        let sql = crate::db::pg_params(&sql);
        let mut aq = sqlx::query_as::<_, AttachmentRow>(&sql).bind(&message_id);
        for aid in &attachment_ids {
            aq = aq.bind(aid);
        }
        aq.fetch_all(&state.db)
            .await
            .unwrap_or_default()
            .iter()
            .map(|a| attachment_view(a, &state.config.upload_dir))
            .collect()
    };

    // Outbound LINE delivery when the customer belongs to that platform
    // (CRD 3914): text plus one element per linked attachment, batched at
    // five per send; attachments without a usable URL are skipped; failure is
    // logged only.
    let customer: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT cu.platform, cu.platform_user_id
         FROM conversations c
         LEFT JOIN customers cu ON cu.id = c.customer_id AND cu.deleted_at IS NULL
         WHERE c.id = $1 AND c.deleted_at IS NULL",
    )
    .bind(&conversation_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    if let Some((Some(platform), Some(recipient))) = customer {
        if platform == "line" && !recipient.is_empty() {
            let mut items: Vec<OutboundItem> = Vec::new();
            if !content.is_empty() {
                items.push(OutboundItem::text(content.clone()));
            }
            for a in &attachments {
                if let Some(url) = a["url"].as_str().filter(|u| !u.is_empty()) {
                    items.push(OutboundItem::text(url.to_string()));
                }
            }
            if !items.is_empty() {
                let gateway = OutboundGateway::from_config(&state.config);
                tokio::spawn(async move {
                    for batch in items.chunks(BATCH_CAP) {
                        if let Err(e) = gateway.send_batch("line", &recipient, batch).await {
                            tracing::warn!(error = %e, "customer-channel LINE relay failed");
                        }
                    }
                });
            }
        }
    }

    // Channel fan-out with the accepted payload (CRD 3915, 3968) and the
    // global conversation-list notification (CRD 3916, 3970); both best-effort.
    let platform = body.platform.clone().unwrap_or_else(|| "line".to_string());
    let data = json!({
        "conversationId": conversation_id,
        "content": content,
        "messageType": message_type,
        "senderType": "agent",
        "senderId": user_id,
        "platform": platform,
        "timestamp": now,
    });
    let message = json!({
        "id": message_id,
        "conversationId": conversation_id,
        "content": content,
        "messageType": message_type,
        "senderType": "agent",
        "senderId": user_id,
        "senderName": sender_name,
        "attachments": attachments,
        "correlationId": body.correlation_id,
        "createdAt": now,
    });
    broadcast_customer_event(
        &state,
        &conversation_id,
        &json!({
            "type": "new_message",
            "conversationId": conversation_id,
            "data": data,
            "message": message,
            "timestamp": now,
        }),
    )
    .await;
    let mut global_data = data.clone();
    global_data["messageId"] = json!(message_id);
    state.realtime.global(
        "new_message",
        json!({
            "eventId": uuid::Uuid::new_v4().to_string(),
            "source": "customer-channel",
            "conversationId": conversation_id,
            "data": global_data,
            "priority": "normal",
            "timestamp": now,
        }),
    );
    // The conversation gained a new most-recent message: coalesced
    // latest-message cache refresh (CRD 4149-4153, 4170 eventual freshness).
    super::latest::schedule_refresh(state.clone(), conversation_id.clone());

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "message": {
                "id": message_id,
                "conversationId": conversation_id,
                "content": content,
                "messageType": message_type,
                "senderType": "agent",
                "senderId": user_id,
                "senderName": sender_name,
                "attachments": attachments,
                "correlationId": body.correlation_id,
                "metadata": metadata,
                "createdAt": now,
            },
        })),
    )
        .into_response()
}

// ------------------------------------------- file upload (CRD 3926-3941)

/// POST /api/customer-channel/upload — store one file asset (CRD 3926-3941).
/// Only stores the asset and returns its public URL; no message is created
/// and no linking happens here.
pub async fn upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let headers = headers.clone();
    let Some(conversation_id) = header(&headers, CONVERSATION_HEADER).map(str::to_string) else {
        return fail(StatusCode::BAD_REQUEST, "Conversation ID header is required");
    };
    let Some(session_token) = header(&headers, SESSION_HEADER).map(str::to_string) else {
        return fail(StatusCode::UNAUTHORIZED, "Session token header is required");
    };

    // The session token must resolve to a live, unexpired session record
    // (CRD 3932, 3938).
    let live: Result<Option<String>, _> = sqlx::query_scalar(
        "SELECT id FROM auth_sessions WHERE id = $1 AND expires_at > $2",
    )
    .bind(&session_token)
    .bind(crate::db::now_iso())
    .fetch_optional(&state.db)
    .await;
    match live {
        Ok(Some(_)) => {}
        Ok(None) => return fail(StatusCode::UNAUTHORIZED, "Session not found or expired"),
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to upload file"),
    }

    let mut file: Option<(String, String, Vec<u8>)> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("upload.bin").to_string();
            let mime = field.content_type().unwrap_or("application/octet-stream").to_string();
            match field.bytes().await {
                Ok(bytes) => {
                    file = Some((filename, mime, bytes.to_vec()));
                    break;
                }
                Err(_) => return fail(StatusCode::BAD_REQUEST, "No file provided"),
            }
        }
    }
    let Some((filename, mime, bytes)) = file.filter(|(_, _, b)| !b.is_empty()) else {
        return fail(StatusCode::BAD_REQUEST, "No file provided");
    };

    // Conversation-namespaced unique key preserving the extension (CRD 3933).
    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let safe: String = e.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            if safe.is_empty() { String::new() } else { format!(".{safe}") }
        })
        .unwrap_or_default();
    let safe_conv: String = conversation_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' })
        .collect();
    let storage_key = format!("conv_{safe_conv}_{}{extension}", uuid::Uuid::new_v4());
    let dir = std::path::Path::new(&state.config.upload_dir);
    let stored = async {
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(dir.join(&storage_key), &bytes).await
    }
    .await;
    if stored.is_err() {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to upload file");
    }

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "url": format!("/uploads/{storage_key}"),
            "fileName": filename,
            "size": bytes.len(),
            "contentType": mime,
        })),
    )
        .into_response()
}

/// Any other path under the channel surface -> 404 plain text (CRD 3944).
pub async fn not_found_plain() -> Response {
    plain(StatusCode::NOT_FOUND, "Not Found")
}

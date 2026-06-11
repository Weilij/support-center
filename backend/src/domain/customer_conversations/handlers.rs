//! Customer-Facing Conversations handlers (CRD §2.3, lines 1042-1170).
//!
//! Every operation is gated by one shared session check plus the four-way
//! conversation access rule (CRD 1045, 1130-1135). Response bodies follow the
//! section's own `{success, ...}` shapes rather than the global envelope.

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{Multipart, Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::auth::tokens;
use crate::domain::conversations::channels::{ChannelGateway, OutboundItem, StubGateway, BATCH_CAP};
use crate::state::AppState;

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "success": false, "error": message }))).into_response()
}

// ----------------------------------------------------- Session credential (CRD 1053, 1152)

/// Validated session identity: user identifier, role, display name.
pub struct CustomerSession {
    pub user_id: String,
    pub role: String,
    pub display_name: String,
}

/// The credential may arrive as an `X-Session-Id` header, an
/// `Authorization: Bearer <token>` header, or a `sessionId` query parameter
/// (CRD 1053, 1128).
fn extract_credential(headers: &HeaderMap, query_session: Option<&str>) -> Option<String> {
    if let Some(v) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")) {
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    query_session.filter(|s| !s.is_empty()).map(str::to_string)
}

/// Verify the signed session token (CRD 1053): a valid, non-expired credential
/// embedding a user identity, role, and display name.
#[allow(clippy::result_large_err)] // the Err is the ready-to-send denial response
fn validate_session(state: &AppState, token: &str) -> Result<CustomerSession, Response> {
    if state.config.jwt_secret.is_empty() {
        return Err(fail(StatusCode::INTERNAL_SERVER_ERROR, "Server configuration error"));
    }
    let claims = tokens::verify(token, &state.config.jwt_secret)
        .map_err(|_| fail(StatusCode::UNAUTHORIZED, "Invalid or expired session"))?;
    Ok(CustomerSession {
        user_id: claims.sub,
        role: claims.role,
        display_name: claims.name.unwrap_or_default(),
    })
}

// ----------------------------------------------- Shared four-way access rule (CRD 1130-1135)

struct ConvContext {
    cust_platform: Option<String>,
    cust_platform_user_id: Option<String>,
}

/// Loads the conversation and applies the shared access rule:
/// 1. an administrator is always admitted;
/// 2. the conversation's owner (its customer) is admitted;
/// 3. an unassigned conversation admits any valid session (open pool);
/// 4. otherwise only members of the assigned team are admitted.
async fn check_access(
    state: &AppState,
    session: &CustomerSession,
    conversation_id: &str,
) -> Result<ConvContext, Response> {
    type ConvRow = (i64, Option<i64>, Option<String>, Option<String>);
    let row: Option<ConvRow> = sqlx::query_as(
        "SELECT c.customer_id, c.team_id, cu.platform, cu.platform_user_id
         FROM conversations c
         LEFT JOIN customers cu ON cu.id = c.customer_id AND cu.deleted_at IS NULL
         WHERE c.id = ? AND c.deleted_at IS NULL",
    )
    .bind(conversation_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch conversation"))?;
    let Some((customer_id, team_id, cust_platform, cust_platform_user_id)) = row else {
        return Err(fail(StatusCode::NOT_FOUND, "Conversation not found"));
    };

    let admitted = session.role == "admin"
        || (session.role == "customer" && session.user_id == customer_id.to_string())
        || match team_id {
            None => true,
            Some(tid) => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM team_members WHERE agent_id = ? AND team_id = ?",
                )
                .bind(&session.user_id)
                .bind(tid)
                .fetch_one(&state.db)
                .await
                .map_err(|_| {
                    fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to verify access")
                })?
                    > 0
            }
        };
    if !admitted {
        return Err(fail(StatusCode::FORBIDDEN, "Access denied for this conversation"));
    }
    Ok(ConvContext { cust_platform, cust_platform_user_id })
}

/// Entry gate shared by all operations: credential extraction, session
/// validation, then the four-way access rule.
async fn gate(
    state: &AppState,
    headers: &HeaderMap,
    query_session: Option<&str>,
    conversation_id: &str,
) -> Result<(CustomerSession, ConvContext), Response> {
    if conversation_id.trim().is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "Conversation ID is required"));
    }
    let Some(token) = extract_credential(headers, query_session) else {
        return Err(fail(StatusCode::UNAUTHORIZED, "Authentication required"));
    };
    let session = validate_session(state, &token)?;
    let ctx = check_access(state, &session, conversation_id).await?;
    Ok((session, ctx))
}

// ------------------------------------------------ Message history (CRD 1049-1068)

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<String>,
    pub before: Option<String>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct HistoryRow {
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

/// Inline link plus, when the stored binary exists, a time-limited
/// force-download link; minting failure degrades to inline-only (CRD 1057).
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
        "filename": a.file_name,
        "mimeType": a.content_type,
        "size": a.file_size,
        "url": a.file_url,
        "downloadUrl": download_url,
    })
}

fn message_view(m: &HistoryRow, attachments: Vec<Value>) -> Value {
    // Unified sender identifier resolved from whichever sender reference is
    // present (CRD 1057, 1150).
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

pub async fn history(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    Query(q): Query<HistoryQuery>,
    headers: HeaderMap,
) -> Response {
    let headers = headers.clone();
    let (_session, _ctx) =
        match gate(&state, &headers, q.session_id.as_deref(), &conversation_id).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };

    let limit = q.limit.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(50).clamp(1, 200);

    // Timestamp-anchored cursor: entries strictly older than the referenced
    // message; an unresolvable cursor falls back to the most recent page
    // (CRD 1055).
    let mut clause = String::from("conversation_id = ? AND deleted_at IS NULL");
    let mut binds: Vec<String> = vec![conversation_id.clone()];
    if let Some(before) = q.before.as_deref().filter(|s| !s.is_empty()) {
        let anchor: Option<String> = match sqlx::query_scalar(
            "SELECT created_at FROM messages WHERE id = ? AND conversation_id = ?",
        )
        .bind(before)
        .bind(&conversation_id)
        .fetch_optional(&state.db)
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch messages")
            }
        };
        if let Some(at) = anchor {
            clause.push_str(" AND created_at < ?");
            binds.push(at);
        }
    }

    let sql = format!(
        "SELECT id, conversation_id, sender_type, customer_id, agent_id, content, content_type,
                is_sent, delivery_status, sender_name, metadata, created_at
         FROM messages WHERE {clause}
         ORDER BY created_at DESC, id DESC LIMIT ?"
    );
    let mut mq = sqlx::query_as::<_, HistoryRow>(&sql);
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

// ---------------------------------------------------- Send a reply (CRD 1070-1101)

#[derive(Deserialize, Default)]
pub struct ReplyBody {
    pub content: Option<String>,
    #[serde(rename = "attachmentIds")]
    pub attachment_ids: Option<Vec<String>>,
    pub assets: Option<Value>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    pub platform: Option<String>,
    #[serde(rename = "correlationId")]
    pub correlation_id: Option<String>,
}

#[derive(Deserialize)]
pub struct SessionOnlyQuery {
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

pub async fn send_reply(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    Query(q): Query<SessionOnlyQuery>,
    headers: HeaderMap,
    body: Option<Json<ReplyBody>>,
) -> Response {
    let headers = headers.clone();
    let (session, ctx) =
        match gate(&state, &headers, q.session_id.as_deref(), &conversation_id).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
    let body = body.map(|Json(b)| b).unwrap_or_default();

    let content = body.content.as_deref().unwrap_or("").trim().to_string();
    let attachment_ids = body.attachment_ids.unwrap_or_default();
    if content.is_empty() && attachment_ids.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "Content or attachments are required");
    }
    // Attachments force a file kind regardless of the supplied value (CRD 1079).
    let message_type = if attachment_ids.is_empty() {
        body.message_type.unwrap_or_else(|| "text".to_string())
    } else {
        "file".to_string()
    };

    // Sender identity from the session credential, with a display-name snapshot
    // (CRD 1084). The agent reference column is only populated when the
    // identity resolves to a real agent record.
    let agent: Option<(String, String)> =
        match sqlx::query_as("SELECT id, display_name FROM agents WHERE id = ? AND deleted_at IS NULL")
            .bind(&session.user_id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(v) => v,
            Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create message"),
        };
    let agent_id = agent.as_ref().map(|(id, _)| id.clone());
    let sender_name = if !session.display_name.is_empty() {
        session.display_name.clone()
    } else {
        agent.as_ref().map(|(_, n)| n.clone()).unwrap_or_else(|| "Unknown".to_string())
    };

    let metadata = json!({
        "assets": body.assets,
        "platform": body.platform.as_deref().unwrap_or("system"),
        "correlationId": body.correlation_id,
        "senderId": session.user_id,
    });

    // Recorded as agent-originated, already sent, and delivered (CRD 1084, 1158).
    let message_id = uuid::Uuid::new_v4().to_string();
    let now = crate::db::now_iso();
    let insert = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content,
                                   content_type, is_sent, sent_at, delivery_status, metadata,
                                   sender_name, created_at)
             VALUES (?, ?, 'agent', ?, ?, ?, 1, ?, 'delivered', ?, ?, ?)",
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
                "UPDATE attachments SET message_id = ?
                 WHERE id IN ({placeholders}) AND message_id IS NULL"
            );
            let mut q = sqlx::query(&sql).bind(&message_id);
            for aid in &attachment_ids {
                q = q.bind(aid);
            }
            q.execute(&mut *tx).await?;
        }
        // Recency markers advance so the conversation re-sorts to the top
        // (CRD 1086).
        sqlx::query("UPDATE conversations SET last_message_at = ?, updated_at = ? WHERE id = ?")
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
             FROM attachments WHERE message_id = ? AND id IN ({placeholders})"
        );
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

    // Best-effort outbound relay to the customer's LINE channel; images relay
    // as image content, other files as file content, chunked to the platform
    // cap. Relay failure never fails the request (CRD 1087).
    if ctx.cust_platform.as_deref() == Some("line") {
        let recipient = ctx.cust_platform_user_id.clone().unwrap_or_default();
        let mut items: Vec<OutboundItem> = Vec::new();
        if !content.is_empty() {
            items.push(OutboundItem { content: content.clone() });
        }
        for a in &attachments {
            if let Some(url) = a["url"].as_str() {
                items.push(OutboundItem { content: url.to_string() });
            }
        }
        if !items.is_empty() {
            tokio::spawn(async move {
                let gateway = StubGateway;
                for batch in items.chunks(BATCH_CAP) {
                    if let Err(e) = gateway.send_batch("line", &recipient, batch) {
                        tracing::warn!(error = %e, "customer-conversation LINE relay failed");
                    }
                }
            });
        }
    }

    // Realtime: `new_message` to all live subscribers of this conversation —
    // every connection of every user, including multiple tabs (CRD 1088, 1164,
    // 3968): conversationId, nested data object, the full message object
    // carrying attachments and the correlationId for client de-duplication.
    // The §5.4 customer-ws channel surface is wired separately (Phase 4).
    {
        let platform = body.platform.clone().unwrap_or_else(|| "line".to_string());
        let data = json!({
            "conversationId": conversation_id,
            "content": content,
            "messageType": message_type,
            "senderType": "agent",
            "senderId": session.user_id,
            "platform": platform,
            "timestamp": now,
        });
        let message = json!({
            "id": message_id,
            "conversationId": conversation_id,
            "content": content,
            "messageType": message_type,
            "senderType": "agent",
            "senderId": session.user_id,
            "senderName": sender_name,
            "attachments": attachments,
            "correlationId": body.correlation_id,
            "createdAt": now,
        });
        state.realtime.to_conversation_message(
            &conversation_id,
            "new_message",
            json!({
                "conversationId": conversation_id,
                "data": data,
                "message": message,
                "timestamp": now,
            }),
        );
        // Best-effort global conversation-list notification so list views
        // refresh their last-message preview (CRD 1089, 1169, 3970).
        state.realtime.global(
            "new_message",
            json!({
                "eventId": uuid::Uuid::new_v4().to_string(),
                "source": "api",
                "conversationId": conversation_id,
                "data": data,
                "priority": "normal",
                "timestamp": now,
            }),
        );
    }

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
                "senderId": session.user_id,
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

// ------------------------------------------------------- File upload (CRD 1103-1122)

pub async fn upload(
    State(state): State<Arc<AppState>>,
    Path(conversation_id): Path<String>,
    Query(q): Query<SessionOnlyQuery>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let headers = headers.clone();
    let (session, _ctx) =
        match gate(&state, &headers, q.session_id.as_deref(), &conversation_id).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };

    // Storage-layer re-validation against the live session store: a session
    // record must exist and be unexpired (CRD 1109, 1119).
    let live: Result<Option<String>, _> = sqlx::query_scalar(
        "SELECT id FROM auth_sessions WHERE agent_id = ? AND expires_at > ?",
    )
    .bind(&session.user_id)
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

    // Conversation-namespaced, unique storage key preserving the original
    // extension (CRD 1110, 1122).
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
    let attachment_id = uuid::Uuid::new_v4().to_string();
    let storage_key = format!("conv_{safe_conv}_{attachment_id}{extension}");
    let dir = std::path::Path::new(&state.config.upload_dir);
    let stored = async {
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(dir.join(&storage_key), &bytes).await
    }
    .await;
    if stored.is_err() {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to upload file");
    }

    // The upload creates no message; the attachment record (unlinked) lets the
    // returned reference be attached to a later reply via `attachmentIds`
    // (CRD 1110, 1151).
    let file_url = format!("/uploads/{storage_key}");
    let agent_exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM agents WHERE id = ? AND deleted_at IS NULL")
            .bind(&session.user_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let inserted = sqlx::query(
        "INSERT INTO attachments (id, message_id, conversation_id, file_name, content_type,
                                  file_size, file_url, storage_key, upload_status, uploaded_by,
                                  created_at)
         VALUES (?, NULL, ?, ?, ?, ?, ?, ?, 'completed', ?, ?)",
    )
    .bind(&attachment_id)
    .bind(&conversation_id)
    .bind(&filename)
    .bind(&mime)
    .bind(bytes.len() as i64)
    .bind(&file_url)
    .bind(&storage_key)
    .bind(&agent_exists)
    .bind(crate::db::now_iso())
    .execute(&state.db)
    .await;
    if inserted.is_err() {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "Failed to upload file");
    }

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "url": file_url,
            "filename": filename,
            "size": bytes.len(),
            "contentType": mime,
            "attachmentId": attachment_id,
        })),
    )
        .into_response()
}

// ------------------------------------------- WebSocket subscription (CRD 1124-1146)

#[derive(Deserialize)]
pub struct WsQuery {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

pub async fn subscribe_ws(
    State(state): State<Arc<AppState>>,
    Query(q): Query<WsQuery>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let headers = headers.clone();
    let (Some(conversation_id), Some(session_id)) = (
        q.conversation_id.as_deref().filter(|s| !s.is_empty()),
        q.session_id.as_deref().filter(|s| !s.is_empty()),
    ) else {
        return fail(StatusCode::BAD_REQUEST, "Missing required parameters");
    };
    let session = match validate_session(&state, session_id) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_access(&state, &session, conversation_id).await {
        return resp;
    }
    let _ = headers;

    match ws {
        Err(_) => fail(StatusCode::BAD_REQUEST, "Expected a WebSocket upgrade request"),
        Ok(ws) => ws.on_upgrade(move |_socket| async move {
            // TODO(realtime): register this connection (tracked per connection,
            // not per user) against the conversation's isolated channel using
            // the validated identity (user id, role, display name); broadcast a
            // USER_CONNECTED presence event to the other subscribers; deliver
            // server-pushed events (new_message / message_updated / presence);
            // ignore inbound client frames (reserved); on close, deregister and
            // broadcast USER_DISCONNECTED only when the user's last connection
            // ends; prune dead connections during broadcast (CRD 1136-1146,
            // 1162-1169). Full realtime lands in Phase 4.
        }),
    }
}

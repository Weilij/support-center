//! Conversations (Agent Side) handlers (CRD §2.1, lines 651-830).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Multipart, Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::channels::{self, OutboundItem};
use super::store::{self, ListFilters};

type Result<T = Response> = std::result::Result<T, AppError>;
type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b).map_err(|_| AppError::BadRequest("Invalid JSON".into()))
}

fn permission_denied() -> AppError {
    AppError::Forbidden("Permission denied".into())
}

fn epoch_ms(iso: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(iso).ok().map(|d| d.timestamp_millis())
}

// ------------------------------------------------------- List conversations (CRD 664-677)

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(rename = "tagIds")]
    pub tag_ids: Option<String>,
    pub search: Option<String>,
    #[serde(rename = "customerName")]
    pub customer_name: Option<String>,
    #[serde(rename = "updatedAfter")]
    pub updated_after: Option<String>,
    #[serde(rename = "updatedBefore")]
    pub updated_before: Option<String>,
}

pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    // Non-numeric tag-id entries are ignored (CRD 667).
    let tag_ids: Vec<i64> = q
        .tag_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect();
    let filters = ListFilters {
        tag_ids,
        search: q.search,
        customer_name: q.customer_name,
        updated_after: q.updated_after,
        updated_before: q.updated_before,
    };
    let rows = store::list_visible(&state.db, &user, &filters).await?;
    let items: Vec<Value> = rows.iter().map(|r| store::conversation_view(r, false)).collect();
    Ok(envelope::ok(items))
}

// --------------------------------------------------- Conversation detail (CRD 679-686)

pub async fn detail(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    if !store::can_act_on(&state.db, &user, &id).await? {
        return Err(permission_denied());
    }
    let row = store::find_full(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;
    Ok(envelope::ok(store::conversation_view(&row, true)))
}

// ----------------------------------------------------------- Mark as read (CRD 688-696)

pub async fn mark_read(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    if !store::can_act_on(&state.db, &user, &id).await? {
        return Err(permission_denied());
    }
    // The update is issued without an existence check: a missing conversation
    // still returns success (CRD 695).
    let now = crate::db::now_iso();
    sqlx::query("UPDATE conversations SET last_viewed_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&id)
        .execute(&state.db)
        .await?;
    // No real-time event is emitted for mark-as-read (CRD 696).
    Ok(envelope::ok(json!({ "lastReadAt": now })))
}

// ----------------------------------------------------- Assign to a team (CRD 698-706)

#[derive(Deserialize, Default)]
pub struct AssignBody {
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
    pub reason: Option<String>,
}

async fn reload_view(state: &AppState, id: &str) -> Result<Value> {
    let row = store::find_full(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to retrieve updated conversation".into()))?;
    Ok(store::conversation_view(&row, true))
}

pub async fn assign(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<AssignBody>,
) -> Result {
    // Spec ambiguity resolved: the "assign" permission check uses the same
    // per-conversation condition as view/send (CRD 584): admin always; agent only
    // when the conversation is unassigned or assigned to their primary team.
    if !store::can_act_on(&state.db, &user, &id).await? {
        return Err(permission_denied());
    }
    let body = parse_json(body)?;
    let team_id = body
        .team_id
        .ok_or_else(|| AppError::BadRequest("Team ID is required for assignment".into()))?;
    let (prior_team, prior_status) = store::find_bare(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;
    let team_name = store::team_name(&state.db, team_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Team not found".into()))?;

    // Routing history is only written when a reason is provided (CRD 706).
    let history = body
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| (prior_team, Some(team_id), r.to_string(), "assign"));
    let details = json!({
        "reversible": true,
        "old": { "teamId": prior_team, "status": prior_status },
        "new": { "teamId": team_id, "status": "assigned" },
        "teamName": team_name,
        "reason": body.reason,
    });
    store::apply_routing_change(
        &state.db, &user, &id, Some(team_id), "assigned", history,
        "conversation assign", details,
    )
    .await?;

    // TODO(realtime): broadcast `conversation_assigned` to the relevant team
    // audiences with { conversationId: id, assignedBy: { id, name }, teamId,
    // teamName, reason, timestamp }; failure is non-fatal (CRD 705, 820).
    let view = reload_view(&state, &id).await?;
    Ok(envelope::ok_msg(view, "Conversation assigned successfully"))
}

// ------------------------------------------------------------- Unassign (CRD 708-716)

#[derive(Deserialize, Default)]
pub struct ReasonBody {
    pub reason: Option<String>,
}

pub async fn unassign(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<ReasonBody>,
) -> Result {
    if !store::can_act_on(&state.db, &user, &id).await? {
        return Err(permission_denied());
    }
    // A missing or invalid body is tolerated (CRD 710).
    let body = parse_json(body).unwrap_or_default();
    let (prior_team, prior_status) = store::find_bare(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;
    let current_team =
        prior_team.ok_or_else(|| AppError::BadRequest("Conversation is not assigned".into()))?;

    // Reason-gated history; a blank reason defaults to a generic label (CRD 712).
    let history = body.reason.as_deref().map(|r| {
        let reason = if r.trim().is_empty() { "Unassigned".to_string() } else { r.to_string() };
        (Some(current_team), None, reason, "unassign")
    });
    let details = json!({
        "reversible": true,
        "old": { "teamId": current_team, "status": prior_status },
        "new": { "teamId": null, "status": "active" },
        "reason": body.reason,
    });
    store::apply_routing_change(
        &state.db, &user, &id, None, "active", history, "conversation unassign", details,
    )
    .await?;

    // TODO(realtime): broadcast `conversation_unassigned` (high priority) with
    // { conversationId: id, previousTeamId: current_team, previousTeamName,
    //   unassignedBy: { id, name }, reason, timestamp }; non-fatal (CRD 714, 821).
    let view = reload_view(&state, &id).await?;
    Ok(envelope::ok_msg(view, "Conversation unassigned successfully"))
}

// ------------------------------------------------- Transfer between teams (CRD 718-726)

#[derive(Deserialize)]
pub struct TransferBody {
    #[serde(rename = "fromTeamId")]
    pub from_team_id: Option<i64>,
    #[serde(rename = "toTeamId")]
    pub to_team_id: Option<i64>,
    pub reason: Option<String>,
}

pub async fn transfer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<TransferBody>,
) -> Result {
    // Admins bypass the per-conversation check (CRD 721).
    if !user.is_admin() && !store::can_act_on(&state.db, &user, &id).await? {
        return Err(permission_denied());
    }
    let body = parse_json(body)?;
    let to_team_id = body
        .to_team_id
        .ok_or_else(|| AppError::BadRequest("Target team ID is required for transfer".into()))?;
    let (prior_team, prior_status) = store::find_bare(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;
    let to_team_name = store::team_name(&state.db, to_team_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Team not found".into()))?;
    let from_team_name = match body.from_team_id {
        Some(fid) => store::team_name(&state.db, fid).await?,
        None => None,
    };

    // Routing history is always written for transfers (CRD 726); the recorded
    // source team is the caller-supplied one (may be empty, CRD 722).
    let history = Some((
        body.from_team_id,
        Some(to_team_id),
        body.reason.clone().unwrap_or_default(),
        "transfer",
    ));
    let details = json!({
        "reversible": true,
        "old": { "teamId": prior_team, "status": prior_status },
        "new": { "teamId": to_team_id, "status": "active" },
        "fromTeamId": body.from_team_id,
        "fromTeamName": from_team_name,
        "toTeamName": to_team_name,
        "reason": body.reason,
    });
    store::apply_routing_change(
        &state.db, &user, &id, Some(to_team_id), "active", history,
        "conversation transfer", details,
    )
    .await?;

    // TODO(realtime): broadcast the dual-team transfer notification so both the
    // losing and gaining teams are informed: conversation summary (id, customer id
    // and name, platform user id, avatar, platform, status, last-message time,
    // target team), fromTeamId/fromTeamName, toTeamId/toTeamName, transferredBy,
    // reason; failure is non-fatal (CRD 722, 822).

    // The full conversation object is not returned by this endpoint (CRD 723).
    Ok(envelope::message_only("Conversation transferred successfully"))
}

// ------------------------------------------------------- List messages (CRD 755-763)

#[derive(Deserialize)]
pub struct MessagesQuery {
    pub page: Option<String>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<String>,
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
    platform_message_id: Option<String>,
    is_recalled: i64,
    recall_deadline: Option<String>,
    recalled_at: Option<String>,
    is_sent: i64,
    sent_at: Option<String>,
    delivery_status: String,
    metadata: Option<String>,
    sender_name: Option<String>,
    created_at: String,
    customer_name: Option<String>,
    agent_name: Option<String>,
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

/// Inline + optional force-download URLs; signing is stubbed for local storage
/// (CRD 759, 763): the download variant exists only when the stored object does.
fn attachment_view(a: &AttachmentRow, upload_dir: &str) -> Value {
    let download_url = a
        .storage_key
        .as_deref()
        .filter(|key| std::path::Path::new(upload_dir).join(key).exists())
        .and(a.file_url.as_deref())
        .map(|u| format!("{u}?download=1"));
    json!({
        "id": a.id,
        "filename": a.file_name,
        "mimeType": a.content_type,
        "size": a.file_size,
        "url": a.file_url,
        "downloadUrl": download_url,
    })
}

fn message_view(
    m: &MessageRow,
    platform: Option<&str>,
    conv_customer_name: Option<&str>,
    attachments: Vec<Value>,
) -> Value {
    // Customer message senders are surfaced to clients as type "user" (CRD 760, 804).
    let sender_type = match m.sender_type.as_str() {
        "customer" => "user",
        other => other,
    };
    let sender_id: Value = match m.sender_type.as_str() {
        "customer" => json!(m.customer_id),
        _ => json!(m.agent_id),
    };
    let sender_name = match m.sender_type.as_str() {
        "customer" => m
            .customer_name
            .clone()
            .or_else(|| conv_customer_name.map(str::to_string))
            .or_else(|| m.sender_name.clone()),
        _ => m.agent_name.clone().or_else(|| m.sender_name.clone()),
    };
    json!({
        "id": m.id,
        "conversationId": m.conversation_id,
        "senderType": sender_type,
        "senderId": sender_id,
        "senderName": sender_name,
        "content": m.content,
        "mediaUrl": null,
        "mediaType": null,
        "messageType": m.content_type,
        "platform": platform,
        "createdAt": epoch_ms(&m.created_at),
        "platformMessageId": m.platform_message_id,
        "isSent": m.is_sent != 0,
        "deliveryStatus": m.delivery_status,
        "metadata": m.metadata.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
        "sentAt": m.sent_at,
        "isRecalled": m.is_recalled != 0,
        "recallDeadline": m.recall_deadline,
        "recalledAt": m.recalled_at,
        "attachments": attachments,
    })
}

pub async fn list_messages(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(q): Query<MessagesQuery>,
) -> Result {
    if !store::can_act_on(&state.db, &user, &id).await? {
        return Err(permission_denied());
    }
    let conv = store::find_full(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;
    let page = q.page.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(1).max(1);
    let page_size =
        q.page_size.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(30).clamp(1, 100);

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE conversation_id = ? AND deleted_at IS NULL",
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await?;

    let rows: Vec<MessageRow> = sqlx::query_as(
        "SELECT m.id, m.conversation_id, m.sender_type, m.customer_id, m.agent_id, m.content,
                m.content_type, m.platform_message_id, m.is_recalled, m.recall_deadline,
                m.recalled_at, m.is_sent, m.sent_at, m.delivery_status, m.metadata,
                m.sender_name, m.created_at,
                cu.display_name AS customer_name, a.display_name AS agent_name
         FROM messages m
         LEFT JOIN customers cu ON cu.id = m.customer_id
         LEFT JOIN agents a ON a.id = m.agent_id
         WHERE m.conversation_id = ? AND m.deleted_at IS NULL
         ORDER BY m.created_at DESC, m.id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(&id)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(&state.db)
    .await?;

    // One attachment fetch for the whole page; per-attachment URL failures degrade
    // rather than failing the page (CRD 763).
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
        for a in aq.fetch_all(&state.db).await? {
            if let Some(mid) = a.message_id.clone() {
                by_message
                    .entry(mid)
                    .or_default()
                    .push(attachment_view(&a, &state.config.upload_dir));
            }
        }
    }

    let platform = conv.cust_platform.as_deref();
    let conv_customer_name = conv.cust_name.as_deref();
    let items: Vec<Value> = rows
        .iter()
        .map(|m| {
            message_view(
                m,
                platform,
                conv_customer_name,
                by_message.remove(&m.id).unwrap_or_default(),
            )
        })
        .collect();

    let total_pages = if total == 0 { 0 } else { (total + page_size - 1) / page_size };
    Ok(envelope::ok(json!({
        "items": items,
        "page": page,
        "pageSize": page_size,
        "total": total,
        "totalPages": total_pages,
        "hasMore": page < total_pages,
    })))
}

// ------------------------------------------- Send a message (async delivery, CRD 765-773)

#[derive(Deserialize)]
pub struct SendBody {
    pub content: Option<String>,
    #[serde(rename = "senderId")]
    pub sender_id: Option<String>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    pub metadata: Option<Value>,
    #[serde(rename = "attachmentIds")]
    pub attachment_ids: Option<Vec<String>>,
}

pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<SendBody>,
) -> Result {
    let body = parse_json(body)?;
    let content = body.content.as_deref().unwrap_or("").trim().to_string();
    let attachment_ids = body.attachment_ids.unwrap_or_default();
    if content.is_empty() && attachment_ids.is_empty() {
        return Err(AppError::BadRequest("Message content or attachments are required".into()));
    }
    let sender_id = body
        .sender_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("Sender ID is required".into()))?
        .to_string();
    let message_type = match body.message_type.as_deref() {
        None => "text".to_string(),
        Some(t) if ["text", "image", "file", "quick_reply"].contains(&t) => t.to_string(),
        Some(_) => {
            return Err(AppError::BadRequest(
                "messageType must be one of: text, image, file, quick_reply".into(),
            ))
        }
    };

    // "Message send" permission (CRD 768): admins always; agents only when the
    // conversation is unassigned or assigned to their primary team, denied with a
    // role-specific explanation.
    if !store::can_act_on(&state.db, &user, &id).await? {
        return Err(AppError::Forbidden(
            "Agents can only send messages in unassigned conversations or conversations assigned to their team".into(),
        ));
    }
    let conv = store::find_full(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;
    // The linked customer must exist (CRD 768).
    if conv.cust_id.is_none() {
        return Err(AppError::NotFound("Customer not found".into()));
    }

    let sender_name: Option<String> =
        sqlx::query_scalar("SELECT display_name FROM agents WHERE id = ?")
            .bind(&sender_id)
            .fetch_optional(&state.db)
            .await?;
    let sender_name = sender_name.unwrap_or_else(|| user.display_name.clone());

    // Persist the outbound message in the pending delivery state, link attachments,
    // and advance the conversation's last-message/update times (CRD 769).
    let message_id = uuid::Uuid::new_v4().to_string();
    let now = crate::db::now_iso();
    let metadata_text = body.metadata.as_ref().map(|m| m.to_string());
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content, content_type,
                               is_sent, delivery_status, metadata, sender_name, created_at)
         VALUES (?, ?, 'agent', ?, ?, ?, 0, 'pending', ?, ?, ?)",
    )
    .bind(&message_id)
    .bind(&id)
    .bind(&sender_id)
    .bind(if content.is_empty() { None } else { Some(content.clone()) })
    .bind(&message_type)
    .bind(&metadata_text)
    .bind(&sender_name)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    if !attachment_ids.is_empty() {
        let placeholders = vec!["?"; attachment_ids.len()].join(", ");
        let sql = format!(
            "UPDATE attachments SET message_id = ? WHERE id IN ({placeholders}) AND conversation_id = ?"
        );
        let mut q = sqlx::query(&sql).bind(&message_id);
        for aid in &attachment_ids {
            q = q.bind(aid);
        }
        q.bind(&id).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE conversations SET last_message_at = ?, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&now)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    // TODO(realtime): broadcast `message_sent` (pending) and the unified
    // new-message event for list/detail previews, scoped to the conversation's
    // assigned team to prevent cross-team leakage; failures are non-fatal
    // (CRD 769, 825-826, 828).

    // Background delivery: returns before delivery is confirmed (CRD 769, 773).
    let mut items: Vec<OutboundItem> = Vec::new();
    if !content.is_empty() {
        items.push(OutboundItem { content: content.clone() });
    }
    if !attachment_ids.is_empty() {
        let placeholders = vec!["?"; attachment_ids.len()].join(", ");
        let sql = format!(
            "SELECT COALESCE(file_url, '') FROM attachments WHERE id IN ({placeholders}) AND message_id = ?"
        );
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for aid in &attachment_ids {
            q = q.bind(aid);
        }
        for url in q.bind(&message_id).fetch_all(&state.db).await? {
            items.push(OutboundItem { content: url });
        }
    }
    let platform = conv.cust_platform.clone().unwrap_or_default();
    let recipient = conv.cust_platform_user_id.clone().unwrap_or_default();
    tokio::spawn(channels::deliver_pending(
        state.db.clone(),
        message_id.clone(),
        platform.clone(),
        recipient,
        items,
    ));

    let created_ms = epoch_ms(&now);
    Ok(envelope::ok_msg(
        json!({
            "id": message_id,
            "conversationId": id,
            "senderType": "agent",
            "senderId": sender_id,
            "senderName": sender_name,
            "content": content,
            "mediaUrl": null,
            "mediaType": null,
            "messageType": message_type,
            "platform": platform,
            "createdAt": created_ms,
            "timestamp": created_ms,
            "deliveryStatus": "pending",
            "isSent": false,
            "platformMessageId": null,
            "metadata": body.metadata,
        }),
        "Message queued for delivery",
    ))
}

// ------------------------------------------------- Upload an attachment (CRD 775-783)

const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

fn failure(status: axum::http::StatusCode, message: &str) -> Response {
    use axum::response::IntoResponse;
    (
        status,
        Json(json!({
            "success": false,
            "error": message,
            "timestamp": crate::db::now_iso(),
        })),
    )
        .into_response()
}

pub async fn upload_attachment(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result {
    use axum::http::StatusCode;
    let conv = store::find_bare(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;

    // Team-scope gate (CRD 778): admins always; unassigned always; otherwise the
    // caller's allowed-team set must include the conversation's assigned team.
    if let (false, Some(team_id)) = (user.is_admin(), conv.0) {
        if !user.can_access_team(team_id) {
            return Ok(failure(
                StatusCode::FORBIDDEN,
                "You do not have access to this conversation",
            ));
        }
    }

    let mut file: Option<(String, String, Vec<u8>)> = None; // (filename, mime, bytes)
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("upload.bin").to_string();
            let mime = field.content_type().unwrap_or("application/octet-stream").to_string();
            match field.bytes().await {
                Ok(bytes) => {
                    file = Some((filename, mime, bytes.to_vec()));
                    break;
                }
                Err(_) => return Ok(failure(StatusCode::BAD_REQUEST, "No file provided")),
            }
        }
    }
    let Some((filename, mime, bytes)) = file.filter(|(_, _, b)| !b.is_empty()) else {
        return Ok(failure(StatusCode::BAD_REQUEST, "No file provided"));
    };
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Ok(failure(StatusCode::BAD_REQUEST, "File too large (max 10MB)"));
    }

    let safe_name: String = filename
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let attachment_id = uuid::Uuid::new_v4().to_string();
    let storage_key = format!("{attachment_id}_{safe_name}");
    let dir = std::path::Path::new(&state.config.upload_dir);
    let stored = async {
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(dir.join(&storage_key), &bytes).await
    }
    .await;
    if stored.is_err() {
        return Ok(failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to upload file to storage",
        ));
    }

    // The attachment exists independently of any message until a later send links
    // it (CRD 779, 783).
    let file_url = format!("/uploads/{storage_key}");
    let now = crate::db::now_iso();
    sqlx::query(
        "INSERT INTO attachments (id, message_id, conversation_id, file_name, content_type,
                                  file_size, file_url, storage_key, upload_status, uploaded_by, created_at)
         VALUES (?, NULL, ?, ?, ?, ?, ?, ?, 'completed', ?, ?)",
    )
    .bind(&attachment_id)
    .bind(&id)
    .bind(&filename)
    .bind(&mime)
    .bind(bytes.len() as i64)
    .bind(&file_url)
    .bind(&storage_key)
    .bind(&user.id)
    .bind(&now)
    .execute(&state.db)
    .await?;

    Ok(envelope::ok(json!({
        "attachmentId": attachment_id,
        "url": file_url,
        "filename": filename,
        "mimeType": mime,
        "size": bytes.len(),
    })))
}

// ----------------------------------------------------------- Bulk operations (CRD 785-798)

#[derive(Deserialize)]
pub struct BulkBody {
    pub operation: Option<String>,
    #[serde(rename = "conversationIds")]
    pub conversation_ids: Option<Value>,
    pub data: Option<Value>,
}

pub async fn bulk(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<BulkBody>,
) -> Result {
    let body = parse_json(body)?;
    let ids: Vec<String> = body
        .conversation_ids
        .as_ref()
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .ok_or_else(|| {
            AppError::BadRequest("conversationIds must be a non-empty array".into())
        })?;

    // A single unauthorized conversation blocks the entire batch (CRD 793, 798).
    let mut visible = 0usize;
    for chunk in store::chunks(&ids) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT id, team_id FROM conversations WHERE id IN ({placeholders}) AND deleted_at IS NULL"
        );
        let mut q = sqlx::query_as::<_, (String, Option<i64>)>(&sql);
        for cid in chunk {
            q = q.bind(cid);
        }
        for (_, team_id) in q.fetch_all(&state.db).await? {
            let ok = user.is_admin()
                || match team_id {
                    None => true,
                    Some(tid) => user.teams.iter().any(|t| t.team_id == tid),
                };
            if ok {
                visible += 1;
            }
        }
    }
    if visible < ids.len() {
        let unauthorized = ids.len() - visible;
        return Err(AppError::Forbidden(format!(
            "Access denied: {unauthorized} conversation(s) are not accessible"
        )));
    }

    let data = body.data.unwrap_or(Value::Null);
    let now = crate::db::now_iso();
    let op = body.operation.as_deref().unwrap_or("");
    match op {
        "assign" => {
            let team_id = data
                .get("teamId")
                .and_then(Value::as_i64)
                .ok_or_else(|| AppError::BadRequest("teamId is required for assign".into()))?;
            for chunk in store::chunks(&ids) {
                let placeholders = vec!["?"; chunk.len()].join(", ");
                let sql = format!(
                    "UPDATE conversations SET team_id = ?, status = 'assigned', updated_at = ?
                     WHERE id IN ({placeholders})"
                );
                let mut q = sqlx::query(&sql).bind(team_id).bind(&now);
                for cid in chunk {
                    q = q.bind(cid);
                }
                q.execute(&state.db).await?;
            }
        }
        "set_priority" => {
            let priority = data
                .get("priority")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| {
                    AppError::BadRequest("priority is required for set_priority".into())
                })?;
            for chunk in store::chunks(&ids) {
                let placeholders = vec!["?"; chunk.len()].join(", ");
                let sql = format!(
                    "UPDATE conversations SET priority = ?, updated_at = ? WHERE id IN ({placeholders})"
                );
                let mut q = sqlx::query(&sql).bind(priority).bind(&now);
                for cid in chunk {
                    q = q.bind(cid);
                }
                q.execute(&state.db).await?;
            }
        }
        "add_tags" | "remove_tags" => {
            let tag_ids: Vec<i64> = data
                .get("tagIds")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .filter(|v: &Vec<i64>| !v.is_empty())
                .ok_or_else(|| {
                    AppError::BadRequest(format!("tagIds are required for {op}"))
                })?;
            for cid in &ids {
                if op == "add_tags" {
                    for tag_id in &tag_ids {
                        // Bulk label addition is idempotent (CRD 790, 798).
                        sqlx::query(
                            "INSERT OR IGNORE INTO conversation_tags (conversation_id, tag_id, assigned_by, created_at)
                             SELECT ?, id, ?, ? FROM tags WHERE id = ?",
                        )
                        .bind(cid)
                        .bind(&user.id)
                        .bind(&now)
                        .bind(tag_id)
                        .execute(&state.db)
                        .await?;
                    }
                } else {
                    let placeholders = vec!["?"; tag_ids.len()].join(", ");
                    let sql = format!(
                        "DELETE FROM conversation_tags WHERE conversation_id = ? AND tag_id IN ({placeholders})"
                    );
                    let mut q = sqlx::query(&sql).bind(cid);
                    for tag_id in &tag_ids {
                        q = q.bind(tag_id);
                    }
                    q.execute(&state.db).await?;
                }
            }
            // TODO(realtime): emit one `conversation_status_changed` event per
            // affected conversation (change type "tags_updated") carrying the
            // label operation, affected tag ids, the updating actor, and a
            // timestamp; failures are non-fatal (CRD 794, 796, 824).
        }
        "close" | "reopen" => {
            // Explicitly rejected as no-longer-supported (CRD 792, 816).
            return Err(AppError::BadRequest(format!(
                "Operation '{op}' is no longer supported"
            )));
        }
        _ => {
            return Err(AppError::BadRequest(
                "Invalid operation. Valid operations are: assign, set_priority, add_tags, remove_tags"
                    .into(),
            ))
        }
    }

    let mut result = Map::new();
    result.insert("operation".into(), json!(op));
    result.insert("affectedCount".into(), json!(ids.len()));
    result.insert("conversationIds".into(), json!(ids));
    Ok(envelope::ok_msg(
        Value::Object(result),
        &format!("Bulk {op} completed successfully"),
    ))
}

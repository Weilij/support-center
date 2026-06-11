//! Messaging HTTP handlers (CRD §2.2, lines 830-1042), mounted at `/api/messages`.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::store::{self, FullMessage, RECALL_PLACEHOLDER};

type Result<T = Response> = std::result::Result<T, AppError>;
type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b).map_err(|_| AppError::BadRequest("Invalid JSON".into()))
}

fn message_not_found() -> AppError {
    AppError::NotFound("Message not found".into())
}

const BULK_CAP: usize = 100;
const FORWARD_CAP: usize = 20;
const TAG_CAP: usize = 10;
const EXPORT_FILTER_CAP: i64 = 100;
const EXPORT_MAX: i64 = 1000;
const EXPORT_DEFAULT: i64 = 100;
const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

/// Allowed upload MIME types: common image, video, audio, PDF, plain text, and
/// Word documents (CRD 957).
const ALLOWED_MIME: &[&str] = &[
    "image/jpeg",
    "image/jpg",
    "image/png",
    "image/gif",
    "image/webp",
    "video/mp4",
    "video/quicktime",
    "video/webm",
    "audio/mpeg",
    "audio/mp3",
    "audio/wav",
    "audio/ogg",
    "application/pdf",
    "text/plain",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
];

// ----------------------------------------------------------- Health & info (CRD 839-847)

pub async fn health() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "status": "healthy",
            "module": "messages",
            "version": env!("CARGO_PKG_VERSION"),
            "timestamp": crate::db::now_iso(),
        })),
    )
        .into_response()
}

pub async fn info() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "data": {
                "name": "messages",
                "version": env!("CARGO_PKG_VERSION"),
                "status": "active",
                "features": [
                    "message CRUD", "recall", "bulk operations", "attachments",
                    "forwarding", "tags", "search", "export", "delayed send",
                ],
                "endpoints": [
                    "GET /api/messages/health",
                    "GET /api/messages/info",
                    "POST /api/messages",
                    "GET /api/messages/:id",
                    "PUT /api/messages/:id",
                    "DELETE /api/messages/:id",
                    "GET /api/messages/conversation/:conversationId",
                    "GET /api/messages/search",
                    "GET /api/messages/stats",
                    "GET /api/messages/tags",
                    "GET /api/messages/export",
                    "GET /api/messages/export/count",
                    "GET /api/messages/export/customers",
                    "GET /api/messages/export/agents",
                    "POST /api/messages/bulk-create",
                    "POST /api/messages/bulk-delete",
                    "GET /api/messages/:id/attachments",
                    "POST /api/messages/:id/attachments",
                    "POST /api/messages/:id/forward",
                    "PUT /api/messages/:id/tags",
                    "DELETE /api/messages/:id/tags",
                ],
            },
            "timestamp": crate::db::now_iso(),
        })),
    )
        .into_response()
}

// ------------------------------------------------------------ Create message (CRD 849-856)

#[derive(Deserialize)]
pub struct CreateBody {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    pub content: Option<String>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    #[serde(rename = "replyToMessageId")]
    pub reply_to_message_id: Option<String>,
    pub metadata: Option<Value>,
    #[serde(rename = "attachmentIds")]
    pub attachment_ids: Option<Vec<String>>,
}

/// @-mention tokens in the content (CRD 853, 1032).
fn extract_mentions(content: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    content
        .split_whitespace()
        .filter_map(|w| w.strip_prefix('@'))
        .map(|s| s.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-'))
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert(s.to_string()))
        .map(str::to_string)
        .collect()
}

pub async fn create_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<CreateBody>,
) -> Result {
    let body = parse_json(body)?;
    let conversation_id = body
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("conversationId is required".into()))?
        .to_string();
    let content = body
        .content
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("content is required".into()))?
        .to_string();
    let message_type = body.message_type.as_deref().unwrap_or("text").to_string();

    let (team_id, _customer_id) = store::conversation_bare(&state.db, &conversation_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;
    if !store::team_scope_ok(&user, team_id) {
        return Err(AppError::Forbidden(
            "You do not have access to this conversation".into(),
        ));
    }

    // A reply target must reference an existing non-deleted message in the same
    // conversation (CRD 851, 853).
    let reply_to = match body.reply_to_message_id.as_deref().map(str::trim).filter(|s| !s.is_empty())
    {
        None => None,
        Some(rid) => {
            let found: Option<String> = sqlx::query_scalar(
                "SELECT id FROM messages
                 WHERE id = ? AND conversation_id = ? AND deleted_at IS NULL",
            )
            .bind(rid)
            .bind(&conversation_id)
            .fetch_optional(&state.db)
            .await?;
            Some(found.ok_or_else(|| AppError::BadRequest("Invalid reply target".into()))?)
        }
    };

    let message_id = store::new_message_id();
    let now = crate::db::now_iso();
    let metadata_text = body.metadata.as_ref().map(|m| m.to_string());
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content, content_type,
                               is_sent, sent_at, delivery_status, reply_to_id, metadata,
                               sender_name, created_at)
         VALUES (?, ?, 'agent', ?, ?, ?, 1, ?, 'sent', ?, ?, ?, ?)",
    )
    .bind(&message_id)
    .bind(&conversation_id)
    .bind(&user.id)
    .bind(&content)
    .bind(&message_type)
    .bind(&now)
    .bind(&reply_to)
    .bind(&metadata_text)
    .bind(&user.display_name)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE conversations SET last_message_at = ?, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&now)
        .bind(&conversation_id)
        .execute(&mut *tx)
        .await?;
    // Associate pre-uploaded attachments not yet linked to a message (CRD 853).
    let attachment_ids = body.attachment_ids.unwrap_or_default();
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
    tx.commit().await?;

    // Mention notifications: each mentioned user except the author; best-effort
    // and non-blocking for the create operation (CRD 853, 1032).
    let mut mentioned_ids: Vec<String> = Vec::new();
    let mention_names = extract_mentions(&content);
    if !mention_names.is_empty() {
        let placeholders = vec!["?"; mention_names.len()].join(", ");
        let sql = format!(
            "SELECT id FROM agents
             WHERE display_name IN ({placeholders}) AND deleted_at IS NULL AND is_active = 1"
        );
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for name in &mention_names {
            q = q.bind(name);
        }
        if let Ok(ids) = q.fetch_all(&state.db).await {
            mentioned_ids = ids.into_iter().filter(|id| *id != user.id).collect();
        }
        let preview: String = content.chars().take(100).collect();
        for target in &mentioned_ids {
            let _ = sqlx::query(
                "INSERT INTO notifications (id, agent_id, type, title, content, data, created_at)
                 VALUES (?, ?, 'mention', ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(target)
            .bind(format!("{} mentioned you", user.display_name))
            .bind(&preview)
            .bind(
                json!({
                    "mentionedBy": { "id": user.id, "name": user.display_name },
                    "conversationId": conversation_id,
                    "messageId": message_id,
                })
                .to_string(),
            )
            .bind(&now)
            .execute(&state.db)
            .await;
        }
    }

    // Audit activity entry (best-effort, CRD 855, 1034).
    crate::domain::auth::store::log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "message send",
        "message",
        Some(&message_id),
        Some(json!({ "conversationId": conversation_id, "messageType": message_type })),
        None,
        None,
    )
    .await;

    let attachments: Vec<Value> = store::attachments_for(&state.db, &message_id)
        .await?
        .iter()
        .map(store::attachment_view)
        .collect();
    let mut data = json!({
        "id": message_id,
        "conversationId": conversation_id,
        "content": content,
        "messageType": message_type,
        "senderType": "agent",
        "agentId": user.id,
        "sentAt": now,
        "createdAt": now,
        "attachments": attachments,
    });
    if !mentioned_ids.is_empty() {
        data["mentions"] = json!(mentioned_ids);
    }
    Ok(envelope::created(data))
}

// ----------------------------------------------------- Get message by id (CRD 858-864)

pub async fn get_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let m = store::find_message(&state.db, &id).await?.ok_or_else(message_not_found)?;
    // Scope violations are deliberately reported as "not found" (CRD 861).
    if !store::team_scope_ok(&user, m.conv_team_id) {
        return Err(message_not_found());
    }
    let mut view = store::list_view(&m);
    view["platformMessageId"] = json!(m.platform_message_id);
    view["recallDeadline"] = json!(m.recall_deadline);
    view["conversationInfo"] = json!({ "status": m.conv_status, "priority": m.conv_priority });
    Ok(envelope::ok(view))
}

// -------------------------------------------------------- Update message (CRD 866-872)

#[derive(Deserialize)]
pub struct UpdateBody {
    pub content: Option<String>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    pub metadata: Option<Value>,
}

/// Positive authorization for edit/recall (CRD 869, 877): the original agent
/// author or an administrator; customer- and system-origin messages never pass.
fn author_or_admin(user: &AuthUser, m: &FullMessage) -> bool {
    m.sender_type == "agent" && (user.is_admin() || m.agent_id.as_deref() == Some(user.id.as_str()))
}

pub async fn update_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<UpdateBody>,
) -> Result {
    let body = parse_json(body)?;
    if let Some(content) = &body.content {
        if content.trim().is_empty() {
            return Err(AppError::BadRequest("content cannot be empty".into()));
        }
    }
    let m = store::find_message(&state.db, &id).await?.ok_or_else(message_not_found)?;
    if !author_or_admin(&user, &m) {
        return Err(AppError::Forbidden("Only the author or an administrator can edit this message".into()));
    }
    if m.is_recalled != 0 {
        return Err(AppError::BadRequest("Message has already been recalled".into()));
    }

    let content = body.content.map(|c| c.trim().to_string()).or(m.content);
    let message_type = body.message_type.unwrap_or(m.content_type);
    let metadata_text = match &body.metadata {
        Some(v) => Some(v.to_string()),
        None => m.metadata,
    };
    let now = crate::db::now_iso();
    sqlx::query(
        "UPDATE messages SET content = ?, content_type = ?, metadata = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&content)
    .bind(&message_type)
    .bind(&metadata_text)
    .bind(&now)
    .bind(&id)
    .execute(&state.db)
    .await?;

    Ok(envelope::ok_msg(
        json!({
            "id": id,
            "conversationId": m.conversation_id,
            "content": content,
            "messageType": message_type,
            "metadata": metadata_text.as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok()),
            "createdAt": m.created_at,
        }),
        "Message updated successfully",
    ))
}

// --------------------------------------------- Recall (soft-delete) message (CRD 874-881)

pub async fn recall_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let m = store::find_message(&state.db, &id).await?.ok_or_else(message_not_found)?;
    if !author_or_admin(&user, &m) {
        return Err(AppError::Forbidden("Only the author or an administrator can recall this message".into()));
    }
    if m.is_recalled != 0 {
        return Err(AppError::BadRequest("Message has already been recalled".into()));
    }
    let now = crate::db::now_iso();
    if let Some(deadline) = &m.recall_deadline {
        if now.as_str() > deadline.as_str() {
            return Err(AppError::BadRequest("Recall deadline has passed".into()));
        }
    }

    sqlx::query(
        "UPDATE messages
            SET is_recalled = 1, recalled_at = ?, content = ?, delivery_status = 'recalled',
                updated_at = ?
          WHERE id = ?",
    )
    .bind(&now)
    .bind(RECALL_PLACEHOLDER)
    .bind(&now)
    .bind(&id)
    .execute(&state.db)
    .await?;

    crate::domain::auth::store::log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "message recall",
        "message",
        Some(&id),
        Some(json!({ "conversationId": m.conversation_id })),
        None,
        None,
    )
    .await;

    Ok(envelope::ok(json!({
        "id": id,
        "conversationId": m.conversation_id,
        "isRecalled": true,
        "recalledAt": now,
        "recalledBy": { "id": user.id, "name": user.display_name },
    })))
}

// ------------------------------------------- List conversation messages (CRD 883-889)

#[derive(Deserialize)]
pub struct ConversationQuery {
    pub page: Option<String>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<String>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    #[serde(rename = "senderType")]
    pub sender_type: Option<String>,
    #[serde(rename = "includeRecalled")]
    pub include_recalled: Option<String>,
}

pub async fn conversation_messages(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(conversation_id): Path<String>,
    Query(q): Query<ConversationQuery>,
) -> Result {
    let exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM conversations WHERE id = ? AND deleted_at IS NULL")
            .bind(&conversation_id)
            .fetch_optional(&state.db)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound("Conversation not found".into()));
    }

    let page = q.page.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(1).max(1);
    let page_size =
        q.page_size.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(20).clamp(1, 100);
    let include_recalled = q.include_recalled.as_deref() == Some("true");

    let mut clause = String::from("m.conversation_id = ? AND m.deleted_at IS NULL");
    let mut binds: Vec<String> = vec![conversation_id.clone()];
    if let Some(t) = q.message_type.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.content_type = ?");
        binds.push(t.to_string());
    }
    if let Some(s) = q.sender_type.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.sender_type = ?");
        binds.push(s.to_string());
    }
    if !include_recalled {
        clause.push_str(" AND m.is_recalled = 0");
    }

    let count_sql = format!("SELECT COUNT(*) FROM messages m WHERE {clause}");
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        cq = cq.bind(b);
    }
    let total = cq.fetch_one(&state.db).await?;

    let sql = format!(
        "{} WHERE {clause} ORDER BY m.created_at DESC, m.id DESC LIMIT ? OFFSET ?",
        store::MESSAGE_SELECT
    );
    let mut mq = sqlx::query_as::<_, FullMessage>(&sql);
    for b in &binds {
        mq = mq.bind(b);
    }
    let rows = mq.bind(page_size).bind((page - 1) * page_size).fetch_all(&state.db).await?;

    let total_pages = if total == 0 { 0 } else { (total + page_size - 1) / page_size };
    Ok(envelope::ok(json!({
        "messages": rows.iter().map(store::list_view).collect::<Vec<_>>(),
        "pagination": {
            "page": page,
            "pageSize": page_size,
            "total": total,
            "totalPages": total_pages,
            "hasMore": page < total_pages,
        },
        "filters": {
            "messageType": q.message_type,
            "senderType": q.sender_type,
            "includeRecalled": include_recalled,
        },
    })))
}

// ------------------------------------------------------------- Search (CRD 891-896)

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    #[serde(rename = "senderType")]
    pub sender_type: Option<String>,
    #[serde(rename = "dateFrom")]
    pub date_from: Option<String>,
    #[serde(rename = "dateTo")]
    pub date_to: Option<String>,
    #[serde(rename = "isRecalled")]
    pub is_recalled: Option<String>,
    pub limit: Option<String>,
    pub offset: Option<String>,
}

pub async fn search_messages(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<SearchQuery>,
) -> Result {
    let limit = q.limit.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(50).clamp(1, 200);
    let offset = q.offset.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).max(0);

    let mut clause = String::from("m.deleted_at IS NULL");
    let mut binds: Vec<String> = Vec::new();
    if let Some(term) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.content LIKE ? ESCAPE '\\'");
        binds.push(format!("%{}%", store::like_escape(term)));
    }
    if let Some(cid) = q.conversation_id.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.conversation_id = ?");
        binds.push(cid.to_string());
    }
    if let Some(t) = q.message_type.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.content_type = ?");
        binds.push(t.to_string());
    }
    if let Some(s) = q.sender_type.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.sender_type = ?");
        binds.push(s.to_string());
    }
    if let Some(f) = q.date_from.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.created_at >= ?");
        binds.push(f.to_string());
    }
    if let Some(t) = q.date_to.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.created_at <= ?");
        binds.push(t.to_string());
    }
    match q.is_recalled.as_deref() {
        Some("true") => clause.push_str(" AND m.is_recalled = 1"),
        Some("false") => clause.push_str(" AND m.is_recalled = 0"),
        _ => {}
    }

    let count_sql = format!("SELECT COUNT(*) FROM messages m WHERE {clause}");
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        cq = cq.bind(b);
    }
    let total = cq.fetch_one(&state.db).await?;

    let sql = format!(
        "{} WHERE {clause} ORDER BY m.created_at DESC, m.id DESC LIMIT ? OFFSET ?",
        store::MESSAGE_SELECT
    );
    let mut mq = sqlx::query_as::<_, FullMessage>(&sql);
    for b in &binds {
        mq = mq.bind(b);
    }
    let rows = mq.bind(limit).bind(offset).fetch_all(&state.db).await?;

    // One attachment fetch for the whole result set.
    let mut by_message: HashMap<String, Vec<Value>> = HashMap::new();
    if !rows.is_empty() {
        let placeholders = vec!["?"; rows.len()].join(", ");
        let sql = format!(
            "SELECT id, message_id, file_name, content_type, file_size, file_url, storage_key,
                    created_at
             FROM attachments WHERE message_id IN ({placeholders})"
        );
        let mut aq = sqlx::query_as::<_, store::AttachmentRow>(&sql);
        for r in &rows {
            aq = aq.bind(&r.id);
        }
        for a in aq.fetch_all(&state.db).await? {
            if let Some(mid) = a.message_id.clone() {
                by_message.entry(mid).or_default().push(store::attachment_view(&a));
            }
        }
    }

    let messages: Vec<Value> = rows
        .iter()
        .map(|m| {
            let metadata = store::parse_metadata(&m.metadata);
            json!({
                "id": m.id,
                "conversationId": m.conversation_id,
                "senderType": m.sender_type,
                "senderName": store::resolved_sender_name(m),
                "senderAvatar": if m.sender_type == "customer" { json!(m.cust_avatar) } else { Value::Null },
                "content": m.content,
                "messageType": m.content_type,
                "isRecalled": m.is_recalled != 0,
                "createdAt": m.created_at,
                "attachments": by_message.get(&m.id).cloned().unwrap_or_default(),
                "reactions": metadata.get("reactions").cloned().unwrap_or_else(|| json!([])),
                "readBy": m.read_by.as_deref()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or_else(|| json!([])),
                "metadata": metadata,
            })
        })
        .collect();

    Ok(envelope::ok(json!({
        "messages": messages,
        "total": total,
        "pagination": { "limit": limit, "offset": offset, "hasMore": offset + limit < total },
        "query": {
            "q": q.q,
            "conversationId": q.conversation_id,
            "messageType": q.message_type,
            "senderType": q.sender_type,
            "dateFrom": q.date_from,
            "dateTo": q.date_to,
            "isRecalled": q.is_recalled,
        },
    })))
}

// ------------------------------------------------------------- Stats (CRD 898-902)

pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL")
            .fetch_one(&state.db)
            .await?;
    // Breakdown fields are reported as zero within the current behavioral
    // boundary; the per-day figure is derived from the total (CRD 900).
    let average_per_day = (total as f64 / 30.0 * 100.0).round() / 100.0;
    Ok(envelope::ok(json!({
        "overview": {
            "totalMessages": total,
            "todayMessages": 0,
            "activeConversations": 0,
            "averagePerDay": average_per_day,
            "recalledMessages": 0,
        },
        "breakdown": { "byType": {}, "bySender": {}, "byStatus": {} },
        "scope": "global",
        "note": "Breakdown statistics are not computed in the current behavioral boundary",
        "generatedAt": crate::db::now_iso(),
    })))
}

// ----------------------------------------------- List available message tags (CRD 904-908)

pub async fn list_tags(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT metadata FROM messages
         WHERE deleted_at IS NULL AND is_recalled = 0 AND metadata IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for (raw,) in &rows {
        if let Some(tags) = store::parse_metadata(raw).get("tags").and_then(Value::as_array) {
            for tag in tags.iter().filter_map(Value::as_str) {
                *counts.entry(tag.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut tags: Vec<(String, i64)> = counts.into_iter().collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total = tags.len();
    let tags: Vec<Value> =
        tags.into_iter().map(|(name, count)| json!({ "name": name, "count": count })).collect();
    Ok(envelope::ok(json!({ "tags": tags, "total": total })))
}

// ----------------------------------------------- Export filter options (CRD 910-918)

pub async fn export_customers(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let rows: Vec<(i64, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, display_name, platform, platform_user_id FROM customers
         WHERE deleted_at IS NULL ORDER BY display_name LIMIT ?",
    )
    .bind(EXPORT_FILTER_CAP)
    .fetch_all(&state.db)
    .await?;
    let data: Vec<Value> = rows
        .iter()
        .map(|(id, name, platform, puid)| {
            json!({ "id": id, "displayName": name, "platform": platform, "platformUserId": puid })
        })
        .collect();
    Ok(envelope::ok(data))
}

pub async fn export_agents(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, display_name, role FROM agents
         WHERE deleted_at IS NULL AND is_active = 1 ORDER BY display_name LIMIT ?",
    )
    .bind(EXPORT_FILTER_CAP)
    .fetch_all(&state.db)
    .await?;
    let data: Vec<Value> = rows
        .iter()
        .map(|(id, name, role)| json!({ "id": id, "displayName": name, "role": role }))
        .collect();
    Ok(envelope::ok(data))
}

// ------------------------------------------------------ Export pre-count (CRD 920-924)

#[derive(Deserialize)]
pub struct ExportQuery {
    pub format: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "dateFrom")]
    pub date_from: Option<String>,
    #[serde(rename = "dateTo")]
    pub date_to: Option<String>,
    #[serde(rename = "customerId")]
    pub customer_id: Option<String>,
    #[serde(rename = "agentId")]
    pub agent_id: Option<String>,
    pub limit: Option<String>,
}

/// Recalled messages are always excluded from exports (CRD 922, 928).
fn export_clause(q: &ExportQuery) -> (String, Vec<String>) {
    let mut clause = String::from("m.deleted_at IS NULL AND m.is_recalled = 0");
    let mut binds = Vec::new();
    if let Some(cid) = q.conversation_id.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.conversation_id = ?");
        binds.push(cid.to_string());
    }
    if let Some(f) = q.date_from.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.created_at >= ?");
        binds.push(f.to_string());
    }
    if let Some(t) = q.date_to.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.created_at <= ?");
        binds.push(t.to_string());
    }
    if let Some(c) = q.customer_id.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.customer_id = ?");
        binds.push(c.to_string());
    }
    if let Some(a) = q.agent_id.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.agent_id = ?");
        binds.push(a.to_string());
    }
    (clause, binds)
}

pub async fn export_count(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ExportQuery>,
) -> Result {
    let (clause, binds) = export_clause(&q);
    let sql = format!("SELECT COUNT(*) FROM messages m WHERE {clause}");
    let mut cq = sqlx::query_scalar::<_, i64>(&sql);
    for b in &binds {
        cq = cq.bind(b);
    }
    let count = cq.fetch_one(&state.db).await?;
    Ok(envelope::ok(json!({
        "count": count,
        "limit": EXPORT_MAX,
        "willBeTruncated": count > EXPORT_MAX,
    })))
}

// --------------------------------------------------------- Export messages (CRD 926-931)

fn csv_escape(field: &str) -> String {
    if field.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Localized-time rendering for the TXT transcript (CRD 929).
fn localized_time(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

pub async fn export_messages(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ExportQuery>,
) -> Result {
    let format = q.format.as_deref().unwrap_or("json");
    if !["json", "csv", "txt"].contains(&format) {
        return Err(AppError::BadRequest(
            "Invalid format. Valid formats are: json, csv, txt".into(),
        ));
    }
    let limit = q
        .limit
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(EXPORT_DEFAULT)
        .clamp(1, EXPORT_MAX);

    let (clause, binds) = export_clause(&q);
    let sql = format!(
        "{} WHERE {clause} ORDER BY m.created_at DESC, m.id DESC LIMIT ?",
        store::MESSAGE_SELECT
    );
    let mut mq = sqlx::query_as::<_, FullMessage>(&sql);
    for b in &binds {
        mq = mq.bind(b);
    }
    let rows = mq.bind(limit).fetch_all(&state.db).await?;
    let now = crate::db::now_iso();

    match format {
        "json" => {
            let messages: Vec<Value> = rows
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "conversationId": m.conversation_id,
                        "senderType": m.sender_type,
                        "senderName": store::resolved_sender_name(m),
                        "content": m.content,
                        "messageType": m.content_type,
                        "createdAt": m.created_at,
                    })
                })
                .collect();
            Ok(envelope::ok(json!({
                "messages": messages,
                "exportInfo": {
                    "format": "json",
                    "totalRecords": rows.len(),
                    "exportedAt": now,
                    "exportedBy": user.id,
                    "filters": {
                        "conversationId": q.conversation_id,
                        "dateFrom": q.date_from,
                        "dateTo": q.date_to,
                        "customerId": q.customer_id,
                        "agentId": q.agent_id,
                    },
                },
            })))
        }
        "csv" => {
            let mut out =
                String::from("id,conversationId,senderType,senderName,content,messageType,createdAt\n");
            for m in &rows {
                out.push_str(&format!(
                    "{},{},{},{},{},{},{}\n",
                    csv_escape(&m.id),
                    csv_escape(&m.conversation_id),
                    csv_escape(&m.sender_type),
                    csv_escape(&store::resolved_sender_name(m).unwrap_or_default()),
                    csv_escape(m.content.as_deref().unwrap_or_default()),
                    csv_escape(&m.content_type),
                    csv_escape(&m.created_at),
                ));
            }
            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"messages_export_{}.csv\"", chrono::Utc::now().timestamp()),
                    ),
                ],
                out,
            )
                .into_response())
        }
        _ => {
            // Human-readable transcript grouped by conversation, each group
            // oldest-first (CRD 929).
            let mut groups: BTreeMap<String, Vec<&FullMessage>> = BTreeMap::new();
            for m in &rows {
                groups.entry(m.conversation_id.clone()).or_default().push(m);
            }
            let mut out = String::new();
            for (conversation_id, mut group) in groups {
                group.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
                out.push_str(&format!("Conversation: {conversation_id}\n"));
                for m in group {
                    out.push_str(&format!(
                        "[{}] {}: {}\n",
                        localized_time(&m.created_at),
                        store::resolved_sender_name(m).unwrap_or_else(|| "Unknown".into()),
                        m.content.as_deref().unwrap_or_default(),
                    ));
                }
                out.push('\n');
            }
            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "text/plain; charset=utf-8".to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"messages_export_{}.txt\"", chrono::Utc::now().timestamp()),
                    ),
                ],
                out,
            )
                .into_response())
        }
    }
}

// ---------------------------------------------------- Bulk create messages (CRD 933-938)

#[derive(Deserialize)]
pub struct BulkCreateBody {
    pub messages: Option<Value>,
}

pub async fn bulk_create(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<BulkCreateBody>,
) -> Result {
    let body = parse_json(body)?;
    let entries = body
        .messages
        .as_ref()
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest("messages must be a non-empty array".into()))?
        .clone();
    if entries.len() > BULK_CAP {
        return Err(AppError::BadRequest(format!(
            "Cannot create more than {BULK_CAP} messages per batch"
        )));
    }

    // One existence probe for every referenced conversation (CRD 936).
    let referenced: HashSet<String> = entries
        .iter()
        .filter_map(|e| e.get("conversationId").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let mut existing: HashSet<String> = HashSet::new();
    if !referenced.is_empty() {
        let ids: Vec<&String> = referenced.iter().collect();
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!(
            "SELECT id FROM conversations WHERE id IN ({placeholders}) AND deleted_at IS NULL"
        );
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for id in &ids {
            q = q.bind(id.as_str());
        }
        existing = q.fetch_all(&state.db).await?.into_iter().collect();
    }

    let now = crate::db::now_iso();
    let mut results: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let mut touched: HashSet<String> = HashSet::new();
    let mut tx = state.db.begin().await?;
    for (index, entry) in entries.iter().enumerate() {
        let conversation_id = entry
            .get("conversationId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let content = entry
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let (Some(conversation_id), Some(content)) = (conversation_id, content) else {
            errors.push(json!({ "index": index, "error": "conversationId and content are required" }));
            continue;
        };
        if !existing.contains(conversation_id) {
            errors.push(json!({ "index": index, "error": "Conversation not found" }));
            continue;
        }
        let message_type = entry.get("messageType").and_then(Value::as_str).unwrap_or("text");
        let metadata = entry.get("metadata").map(|m| m.to_string());
        let message_id = store::new_message_id();
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content,
                                   content_type, is_sent, sent_at, delivery_status, metadata,
                                   sender_name, created_at)
             VALUES (?, ?, 'agent', ?, ?, ?, 1, ?, 'sent', ?, ?, ?)",
        )
        .bind(&message_id)
        .bind(conversation_id)
        .bind(&user.id)
        .bind(content)
        .bind(message_type)
        .bind(&now)
        .bind(&metadata)
        .bind(&user.display_name)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        touched.insert(conversation_id.to_string());
        results.push(json!({
            "index": index,
            "messageId": message_id,
            "conversationId": conversation_id,
            "status": "created",
        }));
    }
    for conversation_id in &touched {
        sqlx::query("UPDATE conversations SET last_message_at = ?, updated_at = ? WHERE id = ?")
            .bind(&now)
            .bind(&now)
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let mut data = json!({
        "totalRequested": entries.len(),
        "successCount": results.len(),
        "failureCount": errors.len(),
        "results": results,
    });
    if !errors.is_empty() {
        data["errors"] = json!(errors);
    }
    Ok(envelope::created(data))
}

// ---------------------------------------------------- Bulk recall messages (CRD 940-945)

#[derive(Deserialize)]
pub struct BulkDeleteBody {
    #[serde(rename = "messageIds")]
    pub message_ids: Option<Value>,
}

pub async fn bulk_delete(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<BulkDeleteBody>,
) -> Result {
    let body = parse_json(body)?;
    let ids: Vec<String> = body
        .message_ids
        .as_ref()
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .filter(|v: &Vec<String>| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("messageIds must be a non-empty array".into()))?;
    if ids.len() > BULK_CAP {
        return Err(AppError::BadRequest(format!(
            "Cannot recall more than {BULK_CAP} messages per batch"
        )));
    }

    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT id, conversation_id, sender_type, agent_id, is_recalled, recall_deadline
         FROM messages WHERE id IN ({placeholders}) AND deleted_at IS NULL"
    );
    // (conversationId, senderType, agentId, isRecalled, recallDeadline)
    type RecallRow = (String, String, Option<String>, i64, Option<String>);
    let mut q = sqlx::query_as::<_, (String, String, String, Option<String>, i64, Option<String>)>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    let found: HashMap<String, RecallRow> = q
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|(id, cid, st, aid, rec, dl)| (id, (cid, st, aid, rec, dl)))
        .collect();

    let now = crate::db::now_iso();
    let mut eligible: Vec<(String, String)> = Vec::new(); // (id, conversationId)
    let mut errors: Vec<Value> = Vec::new();
    for id in &ids {
        match found.get(id) {
            None => errors.push(json!({ "messageId": id, "error": "Message not found" })),
            Some((cid, sender_type, agent_id, is_recalled, deadline)) => {
                let permitted = sender_type == "agent"
                    && (user.is_admin() || agent_id.as_deref() == Some(user.id.as_str()));
                if !permitted {
                    errors.push(json!({ "messageId": id, "error": "Permission denied" }));
                } else if *is_recalled != 0 {
                    errors.push(json!({ "messageId": id, "error": "Message has already been recalled" }));
                } else if deadline.as_deref().is_some_and(|d| now.as_str() > d) {
                    errors.push(json!({ "messageId": id, "error": "Recall deadline has passed" }));
                } else {
                    eligible.push((id.clone(), cid.clone()));
                }
            }
        }
    }

    // Recall all eligible messages in one batch (CRD 943).
    if !eligible.is_empty() {
        let placeholders = vec!["?"; eligible.len()].join(", ");
        let sql = format!(
            "UPDATE messages
                SET is_recalled = 1, recalled_at = ?, content = ?, delivery_status = 'recalled',
                    updated_at = ?
              WHERE id IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql).bind(&now).bind(RECALL_PLACEHOLDER).bind(&now);
        for (id, _) in &eligible {
            q = q.bind(id);
        }
        q.execute(&state.db).await?;
    }

    let results: Vec<Value> = eligible
        .iter()
        .map(|(id, cid)| {
            json!({
                "messageId": id,
                "conversationId": cid,
                "recalledAt": now,
                "status": "recalled",
            })
        })
        .collect();
    let mut data = json!({
        "totalRequested": ids.len(),
        "successCount": results.len(),
        "failureCount": errors.len(),
        "results": results,
    });
    if !errors.is_empty() {
        data["errors"] = json!(errors);
    }
    Ok(envelope::ok(data))
}

// ----------------------------------------------- List message attachments (CRD 947-952)

pub async fn list_attachments(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let m = store::find_message(&state.db, &id).await?.ok_or_else(message_not_found)?;
    let attachments: Vec<Value> =
        store::attachments_for(&state.db, &id).await?.iter().map(store::attachment_view).collect();
    Ok(envelope::ok(json!({
        "messageId": id,
        "conversationId": m.conversation_id,
        "attachments": attachments,
        "count": attachments.len(),
    })))
}

// --------------------------------------------- Upload message attachment (CRD 954-960)

pub async fn upload_attachment(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result {
    let m = store::find_message(&state.db, &id).await?.ok_or_else(message_not_found)?;
    // For agent-origin messages only the author or an administrator may add
    // attachments (CRD 957).
    if m.sender_type == "agent" && !author_or_admin(&user, &m) {
        return Err(AppError::Forbidden(
            "Only the author or an administrator can add attachments".into(),
        ));
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
                Err(_) => return Err(AppError::BadRequest("No file provided".into())),
            }
        }
    }
    let Some((filename, mime, bytes)) = file.filter(|(_, _, b)| !b.is_empty()) else {
        return Err(AppError::BadRequest("No file provided".into()));
    };
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(AppError::BadRequest("File too large (max 10MB)".into()));
    }
    if !ALLOWED_MIME.contains(&mime.as_str()) {
        return Err(AppError::BadRequest(format!("File type '{mime}' is not allowed")));
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
        return Err(AppError::Internal("Failed to upload file to storage".into()));
    }

    let file_url = format!("/uploads/{storage_key}");
    let now = crate::db::now_iso();
    sqlx::query(
        "INSERT INTO attachments (id, message_id, conversation_id, file_name, content_type,
                                  file_size, file_url, storage_key, upload_status, uploaded_by,
                                  created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'completed', ?, ?)",
    )
    .bind(&attachment_id)
    .bind(&id)
    .bind(&m.conversation_id)
    .bind(&filename)
    .bind(&mime)
    .bind(bytes.len() as i64)
    .bind(&file_url)
    .bind(&storage_key)
    .bind(&user.id)
    .bind(&now)
    .execute(&state.db)
    .await?;

    Ok(envelope::created(json!({
        "id": attachment_id,
        "messageId": id,
        "filename": filename,
        "mimeType": mime,
        "fileSize": bytes.len(),
        "url": file_url,
        "createdAt": now,
    })))
}

// ------------------------------------------------------- Forward message (CRD 962-967)

#[derive(Deserialize)]
pub struct ForwardBody {
    #[serde(rename = "targetConversationIds")]
    pub target_conversation_ids: Option<Value>,
    pub comment: Option<String>,
}

pub async fn forward_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<ForwardBody>,
) -> Result {
    let body = parse_json(body)?;
    let targets: Vec<String> = body
        .target_conversation_ids
        .as_ref()
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .filter(|v: &Vec<String>| !v.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest("targetConversationIds must be a non-empty array".into())
        })?;
    if targets.len() > FORWARD_CAP {
        return Err(AppError::BadRequest(format!(
            "Cannot forward to more than {FORWARD_CAP} conversations"
        )));
    }
    let m = store::find_message(&state.db, &id).await?.ok_or_else(message_not_found)?;

    let placeholders = vec!["?"; targets.len()].join(", ");
    let sql = format!(
        "SELECT id FROM conversations WHERE id IN ({placeholders}) AND deleted_at IS NULL"
    );
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for t in &targets {
        q = q.bind(t);
    }
    let existing: HashSet<String> = q.fetch_all(&state.db).await?.into_iter().collect();

    // Forwarded marker prefix plus the optional appended comment (CRD 965).
    let mut content = format!("[Forwarded] {}", m.content.as_deref().unwrap_or_default());
    if let Some(comment) = body.comment.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        content.push_str(&format!("\n\n{comment}"));
    }
    let metadata = json!({
        "forwardedFrom": {
            "messageId": m.id,
            "conversationId": m.conversation_id,
            "senderType": m.sender_type,
        },
        "forwardedBy": user.id,
        "forwardedAt": crate::db::now_iso(),
    })
    .to_string();

    let now = crate::db::now_iso();
    let mut results: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let mut tx = state.db.begin().await?;
    for target in &targets {
        if !existing.contains(target) {
            errors.push(json!({ "conversationId": target, "error": "Conversation not found" }));
            continue;
        }
        let message_id = store::new_message_id();
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content,
                                   content_type, is_sent, sent_at, delivery_status, metadata,
                                   sender_name, created_at)
             VALUES (?, ?, 'agent', ?, ?, ?, 1, ?, 'sent', ?, ?, ?)",
        )
        .bind(&message_id)
        .bind(target)
        .bind(&user.id)
        .bind(&content)
        .bind(&m.content_type)
        .bind(&now)
        .bind(&metadata)
        .bind(&user.display_name)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        results.push(json!({
            "conversationId": target,
            "messageId": message_id,
            "status": "forwarded",
        }));
    }
    // Affected conversations' timestamps are bumped in one batch (CRD 965).
    let bumped: Vec<&str> = results
        .iter()
        .filter_map(|r| r["conversationId"].as_str())
        .collect();
    if !bumped.is_empty() {
        let placeholders = vec!["?"; bumped.len()].join(", ");
        let sql = format!(
            "UPDATE conversations SET last_message_at = ?, updated_at = ?
             WHERE id IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql).bind(&now).bind(&now);
        for cid in &bumped {
            q = q.bind(*cid);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;

    crate::domain::auth::store::log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "message forward",
        "message",
        Some(&id),
        Some(json!({ "targetCount": targets.len() })),
        None,
        None,
    )
    .await;

    let mut data = json!({
        "originalMessageId": id,
        "totalTargets": targets.len(),
        "successCount": results.len(),
        "failureCount": errors.len(),
        "results": results,
    });
    if !errors.is_empty() {
        data["errors"] = json!(errors);
    }
    Ok(envelope::created(data))
}

// --------------------------------------------- Set / replace message tags (CRD 969-974)

#[derive(Deserialize)]
pub struct TagsBody {
    pub tags: Option<Value>,
}

pub async fn set_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<TagsBody>,
) -> Result {
    let body = parse_json(body)?;
    let raw = body
        .tags
        .as_ref()
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("tags must be an array".into()))?;
    if raw.len() > TAG_CAP {
        return Err(AppError::BadRequest(format!("Cannot set more than {TAG_CAP} tags")));
    }
    let mut tags: Vec<String> = Vec::new();
    for entry in raw {
        let tag = entry
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::BadRequest("Every tag must be a non-empty string".into()))?;
        tags.push(tag.to_string());
    }

    let m = store::find_message(&state.db, &id).await?.ok_or_else(message_not_found)?;
    let mut metadata = store::metadata_map(&m.metadata);
    let previous = metadata.get("tags").cloned().unwrap_or_else(|| json!([]));
    let now = crate::db::now_iso();
    metadata.insert("tags".into(), json!(tags));
    metadata.insert("tagsUpdatedAt".into(), json!(now));
    metadata.insert("tagsUpdatedBy".into(), json!(user.id));
    sqlx::query("UPDATE messages SET metadata = ?, updated_at = ? WHERE id = ?")
        .bind(Value::Object(metadata).to_string())
        .bind(&now)
        .bind(&id)
        .execute(&state.db)
        .await?;

    Ok(envelope::ok(json!({
        "messageId": id,
        "conversationId": m.conversation_id,
        "tags": tags,
        "previousTags": previous,
        "updatedAt": now,
        "updatedBy": user.id,
    })))
}

// --------------------------------------------- Remove all message tags (CRD 976-981)

pub async fn remove_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let m = store::find_message(&state.db, &id).await?.ok_or_else(message_not_found)?;
    let mut metadata = store::metadata_map(&m.metadata);
    let removed = metadata.remove("tags").unwrap_or_else(|| json!([]));
    let now = crate::db::now_iso();
    metadata.insert("tagsRemovedAt".into(), json!(now));
    metadata.insert("tagsRemovedBy".into(), json!(user.id));
    sqlx::query("UPDATE messages SET metadata = ?, updated_at = ? WHERE id = ?")
        .bind(Value::Object(metadata).to_string())
        .bind(&now)
        .bind(&id)
        .execute(&state.db)
        .await?;

    Ok(envelope::ok(json!({
        "messageId": id,
        "conversationId": m.conversation_id,
        "removedTags": removed,
        "removedAt": now,
    })))
}

//! Messaging HTTP handlers (CRD §2.2, lines 830-1042), mounted at `/api/messages`.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::store::{self, FullMessage, RECALL_PLACEHOLDER};

mod attachments;
mod bulk;
mod exports;
mod forward;
mod tags;
pub use attachments::*;
pub use bulk::*;
pub use exports::*;
pub use forward::*;
pub use tags::*;

pub(super) type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

pub(super) fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b)
        .map_err(|_| AppError::BadRequest("Invalid JSON".into()))
}

pub(super) fn message_not_found() -> AppError {
    AppError::NotFound("Message not found".into())
}

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
    let reply_to = match body
        .reply_to_message_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(rid) => {
            let found: Option<String> = sqlx::query_scalar(
                "SELECT id FROM messages
                 WHERE id = $1 AND conversation_id = $2 AND deleted_at IS NULL",
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
         VALUES ($1, $2, 'agent', $3, $4, $5, 1, $6, 'sent', $7, $8, $9, $10)",
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
    sqlx::query("UPDATE conversations SET last_message_at = $1, updated_at = $2 WHERE id = $3")
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
        let sql = crate::db::pg_params(&sql);
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for name in &mention_names {
            q = q.bind(name);
        }
        if let Ok(ids) = q.fetch_all(&state.db).await {
            mentioned_ids = ids.into_iter().filter(|id| *id != user.id).collect();
        }
        let preview: String = content.chars().take(100).collect();
        for target in &mentioned_ids {
            if let Err(error) = sqlx::query(
                "INSERT INTO notifications (id, agent_id, type, title, content, data, created_at)
                 VALUES ($1, $2, 'mention', $3, $4, $5, $6)",
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
            .await
            {
                tracing::warn!(
                    error = %error,
                    agent_id = %target,
                    conversation_id,
                    message_id,
                    "mention notification insert failed"
                );
            }
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
    let m = store::find_message(&state.db, &id)
        .await?
        .ok_or_else(message_not_found)?;
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
pub(super) fn author_or_admin(user: &AuthUser, m: &FullMessage) -> bool {
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
    let m = store::find_message(&state.db, &id)
        .await?
        .ok_or_else(message_not_found)?;
    if !author_or_admin(&user, &m) {
        return Err(AppError::Forbidden(
            "Only the author or an administrator can edit this message".into(),
        ));
    }
    if m.is_recalled != 0 {
        return Err(AppError::BadRequest(
            "Message has already been recalled".into(),
        ));
    }

    let content = body.content.map(|c| c.trim().to_string()).or(m.content);
    let message_type = body.message_type.unwrap_or(m.content_type);
    let metadata_text = match &body.metadata {
        Some(v) => Some(v.to_string()),
        None => m.metadata,
    };
    let now = crate::db::now_iso();
    sqlx::query(
        "UPDATE messages SET content = $1, content_type = $2, metadata = $3, updated_at = $4
         WHERE id = $5",
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
    let m = store::find_message(&state.db, &id)
        .await?
        .ok_or_else(message_not_found)?;
    if !author_or_admin(&user, &m) {
        return Err(AppError::Forbidden(
            "Only the author or an administrator can recall this message".into(),
        ));
    }
    if m.is_recalled != 0 {
        return Err(AppError::BadRequest(
            "Message has already been recalled".into(),
        ));
    }
    let now = crate::db::now_iso();
    if let Some(deadline) = &m.recall_deadline {
        if now.as_str() > deadline.as_str() {
            return Err(AppError::BadRequest("Recall deadline has passed".into()));
        }
    }

    sqlx::query(
        "UPDATE messages
            SET is_recalled = 1, recalled_at = $1, content = $2, delivery_status = 'recalled',
                updated_at = $3
          WHERE id = $4",
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
        sqlx::query_scalar("SELECT id FROM conversations WHERE id = $1 AND deleted_at IS NULL")
            .bind(&conversation_id)
            .fetch_optional(&state.db)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound("Conversation not found".into()));
    }

    let page = q
        .page
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    let page_size = q
        .page_size
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(20)
        .clamp(1, 100);
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
    let count_sql = crate::db::pg_params(&count_sql);
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        cq = cq.bind(b);
    }
    let total = cq.fetch_one(&state.db).await?;

    let sql = format!(
        "{} WHERE {clause} ORDER BY m.created_at DESC, m.id DESC LIMIT $1 OFFSET $2",
        store::MESSAGE_SELECT
    );
    let sql = crate::db::pg_params(&sql);
    let mut mq = sqlx::query_as::<_, FullMessage>(&sql);
    for b in &binds {
        mq = mq.bind(b);
    }
    let rows = mq
        .bind(page_size)
        .bind((page - 1) * page_size)
        .fetch_all(&state.db)
        .await?;

    let total_pages = if total == 0 {
        0
    } else {
        (total + page_size - 1) / page_size
    };
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
    let limit = q
        .limit
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    let offset = q
        .offset
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);

    let mut clause = String::from("m.deleted_at IS NULL");
    let mut binds: Vec<String> = Vec::new();
    if let Some(term) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.content ILIKE ? ESCAPE '\\'");
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
    let count_sql = crate::db::pg_params(&count_sql);
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        cq = cq.bind(b);
    }
    let total = cq.fetch_one(&state.db).await?;

    let sql = format!(
        "{} WHERE {clause} ORDER BY m.created_at DESC, m.id DESC LIMIT $1 OFFSET $2",
        store::MESSAGE_SELECT
    );
    let sql = crate::db::pg_params(&sql);
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
        let sql = crate::db::pg_params(&sql);
        let mut aq = sqlx::query_as::<_, store::AttachmentRow>(&sql);
        for r in &rows {
            aq = aq.bind(&r.id);
        }
        for a in aq.fetch_all(&state.db).await? {
            if let Some(mid) = a.message_id.clone() {
                by_message
                    .entry(mid)
                    .or_default()
                    .push(store::attachment_view(&a));
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
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL")
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

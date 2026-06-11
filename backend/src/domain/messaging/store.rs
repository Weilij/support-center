//! Message row access and shared view assembly (CRD §2.2, lines 830-1042).

use serde_json::{json, Map, Value};
use sqlx::SqlitePool;

use crate::error::AppError;
use crate::middleware::auth::AuthUser;

/// Content written over a recalled message (CRD 878, 1024): recall never
/// hard-deletes; it flags the record and replaces the text with this marker.
pub const RECALL_PLACEHOLDER: &str = "[Message recalled]";

/// New-message identifier in the documented prefixed shape
/// `msg_<digits>_<alphanumeric>` (CRD 835).
pub fn new_message_id() -> String {
    let suffix: String = uuid::Uuid::new_v4().simple().to_string().chars().take(9).collect();
    format!("msg_{}_{}", chrono::Utc::now().timestamp_millis(), suffix)
}

/// One message joined with its conversation, agent author, and customer context
/// (CRD 862).
#[derive(sqlx::FromRow, Clone)]
pub struct FullMessage {
    pub id: String,
    pub conversation_id: String,
    pub sender_type: String,
    pub customer_id: Option<i64>,
    pub agent_id: Option<String>,
    pub content: Option<String>,
    pub content_type: String,
    pub platform_message_id: Option<String>,
    pub is_recalled: i64,
    pub recall_deadline: Option<String>,
    pub recalled_at: Option<String>,
    pub is_sent: i64,
    pub sent_at: Option<String>,
    pub delivery_status: String,
    pub reply_to_id: Option<String>,
    pub thread_id: Option<String>,
    pub session_id: Option<String>,
    pub session_seq: Option<i64>,
    pub metadata: Option<String>,
    pub sender_name: Option<String>,
    pub read_by: Option<String>,
    pub created_at: String,
    pub conv_team_id: Option<i64>,
    pub conv_status: Option<String>,
    pub conv_priority: Option<String>,
    pub agent_name: Option<String>,
    pub agent_role: Option<String>,
    pub cust_name: Option<String>,
    pub cust_platform: Option<String>,
    pub cust_platform_user_id: Option<String>,
    pub cust_avatar: Option<String>,
}

pub const MESSAGE_SELECT: &str = "
    SELECT m.id, m.conversation_id, m.sender_type, m.customer_id, m.agent_id, m.content,
           m.content_type, m.platform_message_id, m.is_recalled, m.recall_deadline,
           m.recalled_at, m.is_sent, m.sent_at, m.delivery_status, m.reply_to_id,
           m.thread_id, m.session_id, m.session_seq, m.metadata, m.sender_name,
           m.read_by, m.created_at,
           c.team_id AS conv_team_id, c.status AS conv_status, c.priority AS conv_priority,
           a.display_name AS agent_name, a.role AS agent_role,
           cu.display_name AS cust_name, cu.platform AS cust_platform,
           cu.platform_user_id AS cust_platform_user_id, cu.avatar_url AS cust_avatar
    FROM messages m
    LEFT JOIN conversations c ON c.id = m.conversation_id AND c.deleted_at IS NULL
    LEFT JOIN agents a ON a.id = m.agent_id
    LEFT JOIN customers cu ON cu.id = m.customer_id";

/// Fetch one non-deleted message with its joined context (CRD 861-862:
/// soft-deleted messages are treated as nonexistent).
pub async fn find_message(db: &SqlitePool, id: &str) -> Result<Option<FullMessage>, AppError> {
    let sql = format!("{MESSAGE_SELECT} WHERE m.id = ? AND m.deleted_at IS NULL");
    Ok(sqlx::query_as::<_, FullMessage>(&sql).bind(id).fetch_optional(db).await?)
}

/// Team-scoped read/write gate (CRD 852, 861): administrators always; any
/// caller for an unassigned (shared-pool) conversation; otherwise the assigned
/// team must be among the caller's teams.
pub fn team_scope_ok(user: &AuthUser, team_id: Option<i64>) -> bool {
    user.is_admin()
        || match team_id {
            None => true,
            Some(tid) => user.teams.iter().any(|t| t.team_id == tid),
        }
}

/// Bare conversation lookup: (team_id, customer_id). None when missing or
/// soft-deleted (CRD 852).
pub async fn conversation_bare(
    db: &SqlitePool,
    id: &str,
) -> Result<Option<(Option<i64>, i64)>, AppError> {
    Ok(sqlx::query_as(
        "SELECT team_id, customer_id FROM conversations WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(db)
    .await?)
}

pub fn parse_metadata(raw: &Option<String>) -> Value {
    raw.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null)
}

pub fn metadata_map(raw: &Option<String>) -> Map<String, Value> {
    match parse_metadata(raw) {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Resolved sender display name: persisted snapshot preferred, falling back to
/// the joined agent/customer name (CRD 888, 929).
pub fn resolved_sender_name(m: &FullMessage) -> Option<String> {
    m.sender_name.clone().filter(|s| !s.is_empty()).or_else(|| match m.sender_type.as_str() {
        "customer" => m.cust_name.clone(),
        _ => m.agent_name.clone(),
    })
}

/// Sender-info sub-object: agent / customer / null (CRD 863, 888).
pub fn sender_info(m: &FullMessage) -> Value {
    match m.sender_type.as_str() {
        "agent" => match &m.agent_id {
            Some(id) => json!({ "id": id, "name": m.agent_name, "role": m.agent_role }),
            None => Value::Null,
        },
        "customer" => match m.customer_id {
            Some(id) => json!({ "id": id, "name": m.cust_name, "platform": m.cust_platform }),
            None => Value::Null,
        },
        _ => Value::Null,
    }
}

/// The common per-message wire view used by the listing endpoint (CRD 888).
pub fn list_view(m: &FullMessage) -> Value {
    json!({
        "id": m.id,
        "conversationId": m.conversation_id,
        "senderType": m.sender_type,
        "senderName": resolved_sender_name(m),
        "senderInfo": sender_info(m),
        "content": m.content,
        "messageType": m.content_type,
        "isRecalled": m.is_recalled != 0,
        "recalledAt": m.recalled_at,
        "isSent": m.is_sent != 0,
        "sentAt": m.sent_at,
        "deliveryStatus": m.delivery_status,
        "replyToMessageId": m.reply_to_id,
        "threadId": m.thread_id,
        "sessionId": m.session_id,
        "sessionSeq": m.session_seq,
        "metadata": parse_metadata(&m.metadata),
        "createdAt": m.created_at,
    })
}

#[derive(sqlx::FromRow)]
pub struct AttachmentRow {
    pub id: String,
    pub message_id: Option<String>,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub file_url: Option<String>,
    pub storage_key: Option<String>,
    pub created_at: String,
}

pub async fn attachments_for(
    db: &SqlitePool,
    message_id: &str,
) -> Result<Vec<AttachmentRow>, AppError> {
    Ok(sqlx::query_as::<_, AttachmentRow>(
        "SELECT id, message_id, file_name, content_type, file_size, file_url, storage_key,
                created_at
         FROM attachments WHERE message_id = ? ORDER BY created_at, id",
    )
    .bind(message_id)
    .fetch_all(db)
    .await?)
}

pub fn attachment_view(a: &AttachmentRow) -> Value {
    json!({
        "id": a.id,
        "messageId": a.message_id,
        "filename": a.file_name,
        "mimeType": a.content_type,
        "fileSize": a.file_size,
        "fileUrl": a.file_url,
        "storageKey": a.storage_key,
        "createdAt": a.created_at,
    })
}

/// Bump a conversation's last-activity and updated markers (CRD 853, 936, 965).
pub async fn touch_conversation(
    db: &SqlitePool,
    conversation_id: &str,
    now: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE conversations SET last_message_at = ?, updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(now)
        .bind(conversation_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Escape `%`, `_`, and `\` so a user-supplied term is a safe substring match
/// (CRD 894).
pub fn like_escape(term: &str) -> String {
    term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

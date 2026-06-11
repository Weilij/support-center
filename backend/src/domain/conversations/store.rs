//! Conversation queries and view assembly (CRD §2.1, lines 651-830).

use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::error::AppError;
use crate::middleware::auth::AuthUser;

/// One conversation joined with its team, customer, latest-message preview, and
/// unread customer-message count (CRD 673-674, 683-684).
#[derive(sqlx::FromRow)]
pub struct ConvRow {
    pub id: String,
    pub team_id: Option<i64>,
    pub status: String,
    pub priority: String,
    pub customer_id: i64,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub last_message_at: Option<String>,
    pub last_viewed_at: Option<String>,
    pub team_name: Option<String>,
    pub team_description: Option<String>,
    pub cust_id: Option<i64>,
    pub cust_name: Option<String>,
    pub cust_platform: Option<String>,
    pub cust_platform_user_id: Option<String>,
    pub cust_avatar: Option<String>,
    pub cust_email: Option<String>,
    pub cust_phone: Option<String>,
    pub cust_source_team_id: Option<i64>,
    pub cust_metadata: Option<String>,
    pub cust_created_at: Option<String>,
    pub cust_updated_at: Option<String>,
    pub lm_id: Option<String>,
    pub lm_content: Option<String>,
    pub lm_created_at: Option<String>,
    pub lm_sender_type: Option<String>,
    pub lm_content_type: Option<String>,
    pub unread_count: i64,
}

/// "Unread" = customer-sent, non-deleted messages newer than the later of the most
/// recent agent/system reply and the conversation's last-read marker (CRD 677).
fn base_select(where_clause: &str) -> String {
    format!(
        "SELECT c.id, c.team_id, c.status, c.priority, c.customer_id,
                c.created_at, c.updated_at, c.last_message_at, c.last_viewed_at,
                t.name AS team_name, t.description AS team_description,
                cu.id AS cust_id, cu.display_name AS cust_name, cu.platform AS cust_platform,
                cu.platform_user_id AS cust_platform_user_id, cu.avatar_url AS cust_avatar,
                cu.email AS cust_email, cu.phone AS cust_phone,
                cu.source_team_id AS cust_source_team_id, cu.metadata AS cust_metadata,
                cu.created_at AS cust_created_at, cu.updated_at AS cust_updated_at,
                lm.id AS lm_id, lm.content AS lm_content, lm.created_at AS lm_created_at,
                lm.sender_type AS lm_sender_type, lm.content_type AS lm_content_type,
                (SELECT COUNT(*) FROM messages m
                  WHERE m.conversation_id = c.id
                    AND m.sender_type = 'customer'
                    AND m.deleted_at IS NULL
                    AND m.created_at > MAX(
                        COALESCE((SELECT MAX(r.created_at) FROM messages r
                                   WHERE r.conversation_id = c.id
                                     AND r.sender_type IN ('agent', 'system')
                                     AND r.deleted_at IS NULL), ''),
                        COALESCE(c.last_viewed_at, ''))) AS unread_count
         FROM conversations c
         LEFT JOIN teams t ON t.id = c.team_id
         LEFT JOIN customers cu ON cu.id = c.customer_id AND cu.deleted_at IS NULL
         LEFT JOIN (
             SELECT conversation_id, id, content, created_at, sender_type, content_type,
                    ROW_NUMBER() OVER (PARTITION BY conversation_id
                                       ORDER BY created_at DESC, id DESC) AS rn
             FROM messages WHERE deleted_at IS NULL
         ) lm ON lm.conversation_id = c.id AND lm.rn = 1
         WHERE c.deleted_at IS NULL {where_clause}
         ORDER BY COALESCE(c.updated_at, c.created_at) DESC, c.id"
    )
}

/// Visible-conversation clause (CRD 589-594, 672): admins see all; agents see the
/// unassigned shared pool plus conversations of every team they belong to.
fn visibility_clause(user: &AuthUser) -> String {
    if user.is_admin() {
        return String::new();
    }
    let team_ids: Vec<String> = user.teams.iter().map(|t| t.team_id.to_string()).collect();
    if team_ids.is_empty() {
        " AND c.team_id IS NULL".to_string()
    } else {
        format!(" AND (c.team_id IS NULL OR c.team_id IN ({}))", team_ids.join(", "))
    }
}

#[derive(Default)]
pub struct ListFilters {
    pub tag_ids: Vec<i64>,
    pub search: Option<String>,
    pub customer_name: Option<String>,
    pub updated_after: Option<String>,
    pub updated_before: Option<String>,
}

pub async fn list_visible(
    db: &SqlitePool,
    user: &AuthUser,
    f: &ListFilters,
) -> Result<Vec<ConvRow>, AppError> {
    let mut clause = visibility_clause(user);
    let mut binds: Vec<String> = Vec::new();

    // Labels match directly on the conversation or indirectly through the
    // conversation's customer (CRD 667). Ids are integers, safe to inline.
    if !f.tag_ids.is_empty() {
        let ids = f.tag_ids.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");
        clause.push_str(&format!(
            " AND (EXISTS (SELECT 1 FROM conversation_tags vt
                            WHERE vt.conversation_id = c.id AND vt.tag_id IN ({ids}))
               OR EXISTS (SELECT 1 FROM customer_tags ct
                            WHERE ct.customer_id = c.customer_id AND ct.tag_id IN ({ids})))"
        ));
    }
    for term in [&f.search, &f.customer_name].into_iter().flatten() {
        let term = term.trim();
        if !term.is_empty() {
            clause.push_str(" AND LOWER(COALESCE(cu.display_name, '')) LIKE ?");
            binds.push(format!("%{}%", term.to_lowercase()));
        }
    }
    if let Some(after) = f.updated_after.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        clause.push_str(" AND COALESCE(c.updated_at, c.created_at) >= ?");
        binds.push(after.to_string());
    }
    if let Some(before) = f.updated_before.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        clause.push_str(" AND COALESCE(c.updated_at, c.created_at) <= ?");
        binds.push(before.to_string());
    }

    let sql = base_select(&clause);
    let mut q = sqlx::query_as::<_, ConvRow>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    Ok(q.fetch_all(db).await?)
}

pub async fn find_full(db: &SqlitePool, id: &str) -> Result<Option<ConvRow>, AppError> {
    let sql = base_select(" AND c.id = ?");
    Ok(sqlx::query_as::<_, ConvRow>(&sql).bind(id).fetch_optional(db).await?)
}

/// Bare assignment state: (team_id, status). None when the conversation is missing.
pub async fn find_bare(
    db: &SqlitePool,
    id: &str,
) -> Result<Option<(Option<i64>, String)>, AppError> {
    Ok(sqlx::query_as(
        "SELECT team_id, status FROM conversations WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// Per-conversation capability condition (CRD 578-584, 682): a missing or
/// unassigned conversation is in the shared pool (granted); an assigned one is
/// granted only to admins and to agents whose primary team matches.
pub async fn can_act_on(db: &SqlitePool, user: &AuthUser, id: &str) -> Result<bool, AppError> {
    if user.is_admin() {
        return Ok(true);
    }
    match find_bare(db, id).await? {
        None => Ok(true),
        Some((None, _)) => Ok(true),
        Some((Some(team_id), _)) => Ok(user.primary_team_id == Some(team_id)),
    }
}

/// Customer name fallback chain: display name -> platform user id -> "Unknown
/// customer" (CRD 703).
fn customer_name(r: &ConvRow) -> String {
    r.cust_name
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| r.cust_platform_user_id.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "Unknown customer".to_string())
}

fn parse_json_text(raw: &Option<String>) -> Value {
    raw.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null)
}

/// Shared conversation view (CRD 674, 684): list items and detail/assignment
/// responses use the same shape; detail carries the extended customer object.
pub fn conversation_view(r: &ConvRow, detail: bool) -> Value {
    let customer = match r.cust_id {
        None => Value::Null,
        Some(cid) => {
            let mut c = json!({
                "id": cid,
                "name": customer_name(r),
                "displayName": customer_name(r),
                "platform": r.cust_platform,
                "platformUserId": r.cust_platform_user_id,
                "avatarUrl": r.cust_avatar,
                "createdAt": r.cust_created_at,
            });
            if detail {
                c["email"] = json!(r.cust_email);
                c["phone"] = json!(r.cust_phone);
                c["sourceTeamId"] = json!(r.cust_source_team_id);
                c["metadata"] = parse_json_text(&r.cust_metadata);
                c["updatedAt"] = json!(r.cust_updated_at);
            }
            c
        }
    };
    let assigned_team = match r.team_id {
        None => Value::Null,
        Some(tid) => json!({
            "id": tid,
            "name": r.team_name,
            "description": r.team_description,
        }),
    };
    let last_message = match &r.lm_id {
        None => Value::Null,
        Some(id) => json!({
            "id": id,
            "content": r.lm_content,
            "createdAt": r.lm_created_at,
            "senderType": r.lm_sender_type,
            "messageType": r.lm_content_type,
        }),
    };
    json!({
        "id": r.id,
        "teamId": r.team_id,
        "customerId": r.customer_id,
        "status": r.status,
        "priority": r.priority,
        "createdAt": r.created_at,
        "updatedAt": r.updated_at,
        "lastMessageAt": r.last_message_at,
        "lastReadAt": r.last_viewed_at,
        "customer": customer,
        "assignedTeam": assigned_team,
        // Flattened backward-compatible fields (CRD 674).
        "customerName": r.cust_id.map(|_| customer_name(r)),
        "platform": r.cust_platform,
        "platformUserId": r.cust_platform_user_id,
        "lastMessage": last_message,
        "lastMessageContent": r.lm_content,
        "lastMessageAtActual": r.lm_created_at,
        "lastMessageType": r.lm_content_type,
        "unreadCount": r.unread_count,
    })
}

pub async fn team_name(db: &SqlitePool, id: i64) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar("SELECT name FROM teams WHERE id = ? AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(db)
        .await?)
}

/// Routing change applied atomically with its routing-history record and the
/// reversible audit entry (CRD 706, 716, 726).
#[allow(clippy::too_many_arguments)]
pub async fn apply_routing_change(
    db: &SqlitePool,
    user: &AuthUser,
    conversation_id: &str,
    new_team: Option<i64>,
    new_status: &str,
    history: Option<(Option<i64>, Option<i64>, String, &str)>, // (from, to, reason, type)
    action: &str,
    details: Value,
) -> Result<(), AppError> {
    let now = crate::db::now_iso();
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE conversations SET team_id = ?, status = ?, updated_at = ? WHERE id = ?")
        .bind(new_team)
        .bind(new_status)
        .bind(&now)
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
    if let Some((from, to, reason, transfer_type)) = history {
        sqlx::query(
            "INSERT INTO conversation_transfers
                 (conversation_id, from_team_id, to_team_id, reason, transferred_by, transfer_type, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(conversation_id)
        .bind(from)
        .bind(to)
        .bind(reason)
        .bind(&user.id)
        .bind(transfer_type)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO activity_logs
             (agent_id, agent_name, agent_role, action, resource_type, resource_id, details, created_at)
         VALUES (?, ?, ?, ?, 'conversation', ?, ?, ?)",
    )
    .bind(&user.id)
    .bind(&user.display_name)
    .bind(&user.role)
    .bind(action)
    .bind(conversation_id)
    .bind(details.to_string())
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Chunk a large id list so a single statement never exceeds the datastore's
/// bound-parameter limit (CRD 677, 798).
pub const CHUNK: usize = 500;

pub fn chunks(ids: &[String]) -> impl Iterator<Item = &[String]> {
    ids.chunks(CHUNK)
}

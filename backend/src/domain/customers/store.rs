//! Persistence helpers for the customer directory (CRD §3.1).

use serde_json::{json, Value};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CustomerRow {
    pub id: i64,
    pub platform: String,
    pub platform_user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub source_team_id: Option<i64>,
    pub metadata: Option<String>,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
}

/// Raw customer record view (CRD 1660): identifiers, platform identity, contact
/// details, owning-team reference, metadata blob and timestamps.
pub fn customer_view(c: &CustomerRow) -> Value {
    let metadata = c
        .metadata
        .as_deref()
        .map(|m| serde_json::from_str::<Value>(m).unwrap_or_else(|_| json!(m)));
    json!({
        "id": c.id,
        "platform": c.platform,
        "platform_user_id": c.platform_user_id,
        "display_name": c.display_name,
        "avatar_url": c.avatar_url,
        "email": c.email,
        "phone": c.phone,
        "source_team_id": c.source_team_id,
        "metadata": metadata,
        "created_at": c.created_at,
        "updated_at": c.updated_at,
    })
}

pub async fn find_customer(pool: &PgPool, id: i64) -> sqlx::Result<Option<CustomerRow>> {
    sqlx::query_as::<_, CustomerRow>("SELECT * FROM customers WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn find_customer_by_platform(
    pool: &PgPool,
    platform: &str,
    platform_user_id: &str,
) -> sqlx::Result<Option<CustomerRow>> {
    sqlx::query_as::<_, CustomerRow>(
        "SELECT * FROM customers WHERE platform = $1 AND platform_user_id = $2 AND deleted_at IS NULL",
    )
    .bind(platform)
    .bind(platform_user_id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct ConversationRow {
    pub id: String,
    pub customer_id: i64,
    pub team_id: Option<i64>,
    pub status: String,
    pub priority: String,
    pub first_response_at: Option<String>,
    pub closed_at: Option<String>,
    pub last_message_at: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
}

pub fn conversation_view(c: &ConversationRow) -> Value {
    json!({
        "id": c.id,
        "customer_id": c.customer_id,
        "team_id": c.team_id,
        "status": c.status,
        "priority": c.priority,
        "first_response_at": c.first_response_at,
        "closed_at": c.closed_at,
        "last_message_at": c.last_message_at,
        "created_at": c.created_at,
        "updated_at": c.updated_at,
    })
}

pub async fn customer_conversations(
    pool: &PgPool,
    customer_id: i64,
) -> sqlx::Result<Vec<ConversationRow>> {
    sqlx::query_as::<_, ConversationRow>(
        "SELECT id, customer_id, team_id, status, priority, first_response_at, closed_at,
                last_message_at, created_at, updated_at
         FROM conversations WHERE customer_id = $1 AND deleted_at IS NULL
         ORDER BY created_at DESC",
    )
    .bind(customer_id)
    .fetch_all(pool)
    .await
}

/// Distinct ids among `ids` that reference an existing, active, non-deleted tag.
pub async fn active_tag_ids(pool: &PgPool, ids: &[i64]) -> sqlx::Result<Vec<i64>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT id FROM tags WHERE id IN ({placeholders}) AND is_active = 1 AND deleted_at IS NULL"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for id in ids {
        q = q.bind(id);
    }
    q.fetch_all(pool).await
}

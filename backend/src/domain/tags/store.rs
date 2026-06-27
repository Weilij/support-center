//! Persistence for labels and their derived usage counts (CRD §2.6).

use sqlx::PgPool;

/// Derived-count subselects shared by every tag listing (CRD 1479, 1555, 1709):
/// distinct non-deleted customers carrying the tag, and distinct non-deleted
/// conversations belonging to those customers (conversations are reached through
/// customer-label associations, not conversation-label ones — CRD 1627).
pub const TAG_COUNT_COLUMNS: &str = "\
 (SELECT COUNT(DISTINCT ct.customer_id) FROM customer_tags ct \
   JOIN customers c ON c.id = ct.customer_id AND c.deleted_at IS NULL \
   WHERE ct.tag_id = t.id) AS customer_count, \
 (SELECT COUNT(DISTINCT cv.id) FROM customer_tags ct2 \
   JOIN customers c2 ON c2.id = ct2.customer_id AND c2.deleted_at IS NULL \
   JOIN conversations cv ON cv.customer_id = ct2.customer_id AND cv.deleted_at IS NULL \
   WHERE ct2.tag_id = t.id) AS conversation_count";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TagRow {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub description: Option<String>,
    pub team_id: Option<i64>,
    pub is_active: i64,
    pub created_by: String,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct TagWithCounts {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub description: Option<String>,
    pub team_id: Option<i64>,
    pub is_active: i64,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub customer_count: i64,
    pub conversation_count: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct TagDetail {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub description: Option<String>,
    pub team_id: Option<i64>,
    pub team_name: Option<String>,
    pub is_active: i64,
    pub created_by: String,
    pub created_by_name: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub customer_count: i64,
    pub conversation_count: i64,
}

/// Escape LIKE wildcards so user-supplied search text matches literally
/// (CRD 1553: wildcard characters in the search are treated literally).
pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A live (non-soft-deleted) tag.
pub async fn find_live_tag(pool: &PgPool, id: i64) -> sqlx::Result<Option<TagRow>> {
    sqlx::query_as::<_, TagRow>("SELECT * FROM tags WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Name uniqueness among live tags (soft-deleting a tag frees its name — CRD 1487/1504).
pub async fn name_in_use(pool: &PgPool, name: &str, exclude_id: Option<i64>) -> sqlx::Result<bool> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM tags WHERE name = $1 AND deleted_at IS NULL AND id != $2 LIMIT 1",
    )
    .bind(name)
    .bind(exclude_id.unwrap_or(-1))
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

/// Tag plus derived counts; does NOT exclude soft-deleted rows (used by update re-read).
pub async fn tag_with_counts(pool: &PgPool, id: i64) -> sqlx::Result<Option<TagWithCounts>> {
    let sql = format!(
        "SELECT t.id, t.name, t.color, t.description, t.team_id, t.is_active, t.created_by, \
         t.created_at, t.updated_at, {TAG_COUNT_COLUMNS} FROM tags t WHERE t.id = $1"
    );
    sqlx::query_as::<_, TagWithCounts>(&crate::db::pg_params(&sql))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Detail view with team/creator display names; intentionally does not exclude
/// soft-deleted tags (CRD 1497).
pub async fn tag_detail(pool: &PgPool, id: i64) -> sqlx::Result<Option<TagDetail>> {
    let sql = format!(
        "SELECT t.id, t.name, t.color, t.description, t.team_id, tm.name AS team_name, \
         t.is_active, t.created_by, a.display_name AS created_by_name, \
         t.created_at, t.updated_at, {TAG_COUNT_COLUMNS} \
         FROM tags t \
         LEFT JOIN teams tm ON tm.id = t.team_id \
         LEFT JOIN agents a ON a.id = t.created_by \
         WHERE t.id = $1"
    );
    sqlx::query_as::<_, TagDetail>(&crate::db::pg_params(&sql))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// One page of live tags ordered by name, with derived counts (CRD 1475-1481).
pub async fn list_tags(
    pool: &PgPool,
    page: i64,
    page_size: i64,
    search: Option<&str>,
) -> sqlx::Result<(Vec<TagWithCounts>, i64)> {
    let pattern = search
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", escape_like(s)));
    let search_sql = if pattern.is_some() {
        " AND (t.name ILIKE ? ESCAPE '\\' OR t.description ILIKE ? ESCAPE '\\')"
    } else {
        ""
    };

    let count_sql = format!("SELECT COUNT(*) FROM tags t WHERE t.deleted_at IS NULL{search_sql}");
    let count_sql = crate::db::pg_params(&count_sql);
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    if let Some(p) = &pattern {
        count_q = count_q.bind(p.clone()).bind(p.clone());
    }
    let total = count_q.fetch_one(pool).await?;

    let rows_sql = format!(
        "SELECT t.id, t.name, t.color, t.description, t.team_id, t.is_active, t.created_by, \
         t.created_at, t.updated_at, {TAG_COUNT_COLUMNS} \
         FROM tags t WHERE t.deleted_at IS NULL{search_sql} \
         ORDER BY t.name ASC LIMIT $1 OFFSET $2"
    );
    let rows_sql = crate::db::pg_params(&rows_sql);
    let mut rows_q = sqlx::query_as::<_, TagWithCounts>(&rows_sql);
    if let Some(p) = &pattern {
        rows_q = rows_q.bind(p.clone()).bind(p.clone());
    }
    let rows = rows_q
        .bind(page_size)
        .bind((page - 1) * page_size)
        .fetch_all(pool)
        .await?;
    Ok((rows, total))
}

/// `ceil(total / size)`, 0 when empty — pagination metadata helper.
pub fn total_pages(total: i64, size: i64) -> i64 {
    if total == 0 {
        0
    } else {
        (total + size - 1) / size
    }
}

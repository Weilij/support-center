//! Persistence and aggregation over the append-only audit trail (CRD §3.5).

use serde_json::{json, Value};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActivityRow {
    pub id: i64,
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub agent_role: Option<String>,
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub details: Option<String>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
    pub restore_state: Option<String>,
    pub restored_by_log_id: Option<i64>,
    pub restored_at: Option<String>,
}

impl ActivityRow {
    pub fn details_json(&self) -> Value {
        self.details
            .as_deref()
            .and_then(|d| serde_json::from_str(d).ok())
            .unwrap_or(Value::Null)
    }
}

/// Wire shape of one audit entry (CRD 2470-2471). The restore-state indicator lives
/// inside the detail object per the reversible-action metadata concept (CRD 2586).
pub fn entry_view(row: &ActivityRow) -> Value {
    let mut details = row.details_json();
    if row.restore_state.is_some() {
        if !details.is_object() {
            details = json!({});
        }
        details["restoreState"] = json!(row.restore_state);
        details["restoredBy"] = json!(row.restored_by_log_id);
        details["restoredAt"] = json!(row.restored_at);
    }
    json!({
        "id": row.id,
        "userId": row.agent_id,
        "userName": row.agent_name,
        "userRole": row.agent_role,
        "action": row.action,
        "resourceType": row.resource_type,
        "resourceId": row.resource_id,
        "details": details,
        "ipAddress": row.ip_address,
        "userAgent": row.user_agent,
        "createdAt": row.created_at,
    })
}

/// Shared filter for listing and the statistics families. Timestamps are ISO strings
/// compared via `::timestamptz` so 'Z' and '+00:00' spellings collate together.
#[derive(Debug, Default, Clone)]
pub struct ListFilter {
    pub user_id: Option<String>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
}

fn where_clause(f: &ListFilter) -> (String, Vec<String>) {
    let mut conds: Vec<&str> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    if let Some(v) = &f.user_id {
        conds.push("agent_id = ?");
        binds.push(v.clone());
    }
    if let Some(v) = &f.action {
        conds.push("action = ?");
        binds.push(v.clone());
    }
    if let Some(v) = &f.resource_type {
        conds.push("resource_type = ?");
        binds.push(v.clone());
    }
    if let Some(v) = &f.start {
        conds.push("(created_at)::timestamptz >= (?)::timestamptz");
        binds.push(v.clone());
    }
    if let Some(v) = &f.end {
        conds.push("(created_at)::timestamptz <= (?)::timestamptz");
        binds.push(v.clone());
    }
    let w = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    (w, binds)
}

pub async fn find(pool: &PgPool, id: i64) -> sqlx::Result<Option<ActivityRow>> {
    sqlx::query_as::<_, ActivityRow>("SELECT * FROM activity_logs WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Newest-first page plus total count (CRD 2466-2472).
pub async fn list(
    pool: &PgPool,
    f: &ListFilter,
    page: i64,
    limit: i64,
) -> sqlx::Result<(Vec<ActivityRow>, i64)> {
    let (w, binds) = where_clause(f);
    let count_sql = format!("SELECT COUNT(*) FROM activity_logs{w}");
    let count_sql = crate::db::pg_params(&count_sql);
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        count_q = count_q.bind(b.clone());
    }
    let total = count_q.fetch_one(pool).await?;

    let sql = format!(
        "SELECT * FROM activity_logs{w} ORDER BY (created_at)::timestamptz DESC, id DESC LIMIT $1 OFFSET $2"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, ActivityRow>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    let rows = q.bind(limit).bind((page - 1) * limit).fetch_all(pool).await?;
    Ok((rows, total))
}

pub async fn count(pool: &PgPool, f: &ListFilter) -> sqlx::Result<i64> {
    let (w, binds) = where_clause(f);
    let sql = format!("SELECT COUNT(*) FROM activity_logs{w}");
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.fetch_one(pool).await
}

/// `(action, count)` ordered by frequency.
pub async fn action_breakdown(
    pool: &PgPool,
    f: &ListFilter,
) -> sqlx::Result<Vec<(String, i64)>> {
    let (w, binds) = where_clause(f);
    let sql = format!(
        "SELECT action, COUNT(*) AS c FROM activity_logs{w} GROUP BY action ORDER BY c DESC, action"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, (String, i64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.fetch_all(pool).await
}

/// Top contributors `(display name, role, count)` (CRD 2509).
pub async fn top_users(
    pool: &PgPool,
    f: &ListFilter,
    limit: i64,
) -> sqlx::Result<Vec<(String, String, i64)>> {
    let (w, binds) = where_clause(f);
    let sql = format!(
        "SELECT COALESCE(MAX(agent_name), agent_id), COALESCE(MAX(agent_role), 'agent'), COUNT(*) AS c
         FROM activity_logs{w} GROUP BY agent_id ORDER BY c DESC LIMIT $1"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, (String, String, i64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.bind(limit).fetch_all(pool).await
}

/// `(YYYY-MM-DD, count)` ascending by day.
pub async fn daily_counts(pool: &PgPool, f: &ListFilter) -> sqlx::Result<Vec<(String, i64)>> {
    let (w, binds) = where_clause(f);
    let sql = format!(
        "SELECT substr(created_at, 1, 10) AS d, COUNT(*) FROM activity_logs{w} GROUP BY d ORDER BY d"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, (String, i64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.fetch_all(pool).await
}

/// `(YYYY-MM-DD, action, count)` ascending by day (trends, CRD 2530-2532).
pub async fn daily_action_counts(
    pool: &PgPool,
    f: &ListFilter,
) -> sqlx::Result<Vec<(String, String, i64)>> {
    let (w, binds) = where_clause(f);
    let sql = format!(
        "SELECT substr(created_at, 1, 10) AS d, action, COUNT(*) FROM activity_logs{w}
         GROUP BY d, action ORDER BY d, action"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, (String, String, i64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.fetch_all(pool).await
}

/// `(YYYY-MM-DD, hour-of-day, count)` buckets (heatmap, CRD 2534-2536).
pub async fn heat_buckets(
    pool: &PgPool,
    f: &ListFilter,
) -> sqlx::Result<Vec<(String, i64, i64)>> {
    let (w, binds) = where_clause(f);
    let sql = format!(
        "SELECT substr(created_at, 1, 10) AS d, CAST(substr(created_at, 12, 2) AS BIGINT) AS h,
                COUNT(*) FROM activity_logs{w} GROUP BY d, h ORDER BY d, h"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, (String, i64, i64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.fetch_all(pool).await
}

/// `(hour-of-day, count)` across the window (metrics peak hour, CRD 2538-2540).
pub async fn hour_counts(pool: &PgPool, f: &ListFilter) -> sqlx::Result<Vec<(i64, i64)>> {
    let (w, binds) = where_clause(f);
    let sql = format!(
        "SELECT CAST(substr(created_at, 12, 2) AS BIGINT) AS h, COUNT(*) AS c
         FROM activity_logs{w} GROUP BY h ORDER BY c DESC, h"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, (i64, i64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.fetch_all(pool).await
}

/// `(resource type, count)` ordered by frequency (CRD 2518-2520).
pub async fn resource_counts(
    pool: &PgPool,
    f: &ListFilter,
) -> sqlx::Result<Vec<(String, i64)>> {
    let (w, binds) = where_clause(f);
    let sql = format!(
        "SELECT COALESCE(resource_type, 'unknown') AS r, COUNT(*) AS c
         FROM activity_logs{w} GROUP BY r ORDER BY c DESC, r"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, (String, i64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.fetch_all(pool).await
}

/// `(actor role, count)` ordered by frequency (CRD 2522-2524).
pub async fn role_counts(pool: &PgPool, f: &ListFilter) -> sqlx::Result<Vec<(String, i64)>> {
    let (w, binds) = where_clause(f);
    let sql = format!(
        "SELECT COALESCE(agent_role, 'unknown') AS r, COUNT(*) AS c
         FROM activity_logs{w} GROUP BY r ORDER BY c DESC, r"
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, (String, i64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    q.fetch_all(pool).await
}

/// Hard-deletes entries older than the cutoff; returns the removed count (CRD 2495-2502).
pub async fn purge_before(pool: &PgPool, cutoff_iso: &str) -> sqlx::Result<i64> {
    let res = sqlx::query("DELETE FROM activity_logs WHERE (created_at)::timestamptz < ($1)::timestamptz")
        .bind(cutoff_iso)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() as i64)
}

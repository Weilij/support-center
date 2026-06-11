//! Customer directory & customer-label association handlers
//! (CRD §3.1 lines 1644-1792 and §2.6 lines 1551-1592).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

use crate::domain::auth::store::log_activity;
use crate::domain::tags::handlers::{coerce_tag_ids, parse_json, validation};
use crate::domain::tags::store::{escape_like, total_pages, TagWithCounts, TAG_COUNT_COLUMNS};
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::store::{self, CustomerRow};

type Result<T = Response> = std::result::Result<T, AppError>;
type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

/// `customerId` path segments must be positive integers (CRD 1667, 1673).
fn parse_customer_id(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| AppError::BadRequest("Invalid customer ID".into()))
}

/// Team-scope rule (CRD 1659): admins see everything; other staff see customers
/// owned by their primary team or by no team (shared pool).
fn can_access(user: &AuthUser, customer: &CustomerRow) -> bool {
    user.is_admin()
        || match customer.source_team_id {
            None => true,
            Some(team) => user.primary_team_id == Some(team),
        }
}

async fn require_customer(state: &AppState, id: i64) -> Result<CustomerRow> {
    store::find_customer(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Customer not found".into()))
}

// -------------------------------------------------- List visible customers (CRD 1655-1663)

pub async fn list_customers(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    let rows: Vec<CustomerRow> = if user.is_admin() {
        sqlx::query_as(
            "SELECT * FROM customers WHERE deleted_at IS NULL ORDER BY created_at DESC",
        )
        .fetch_all(&state.db)
        .await?
    } else if let Some(team) = user.primary_team_id {
        sqlx::query_as(
            "SELECT * FROM customers
             WHERE deleted_at IS NULL AND (source_team_id IS NULL OR source_team_id = ?)
             ORDER BY created_at DESC",
        )
        .bind(team)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT * FROM customers WHERE deleted_at IS NULL AND source_team_id IS NULL
             ORDER BY created_at DESC",
        )
        .fetch_all(&state.db)
        .await?
    };

    let customers: Vec<Value> = rows.iter().map(store::customer_view).collect();
    Ok(envelope::ok(json!({
        "customers": customers,
        "count": customers.len(),
    })))
}

// ------------------------------------- Get one customer with conversations (CRD 1665-1677)

fn customer_with_conversations(
    customer: &CustomerRow,
    conversations: &[store::ConversationRow],
) -> Value {
    json!({
        "customer": store::customer_view(customer),
        "conversations": conversations.iter().map(store::conversation_view).collect::<Vec<_>>(),
        "conversationCount": conversations.len(),
    })
}

pub async fn get_customer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_customer_id(&raw_id)?;
    let customer = require_customer(&state, id).await?;
    // Out-of-scope callers receive the identical "not found" body so existence is
    // never leaked; a 403 is deliberately never returned here (CRD 1675-1677).
    if !can_access(&user, &customer) {
        return Err(AppError::NotFound("Customer not found".into()));
    }
    let conversations = store::customer_conversations(&state.db, id).await?;
    Ok(envelope::ok(customer_with_conversations(&customer, &conversations)))
}

// ----------------------------------------- Look up by platform identity (CRD 1679-1689)

pub async fn get_customer_by_platform(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((platform, platform_user_id)): Path<(String, String)>,
) -> Result {
    let customer = store::find_customer_by_platform(&state.db, &platform, &platform_user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Customer not found".into()))?;
    if !can_access(&user, &customer) {
        return Err(AppError::NotFound("Customer not found".into()));
    }
    let conversations = store::customer_conversations(&state.db, customer.id).await?;
    Ok(envelope::ok(customer_with_conversations(&customer, &conversations)))
}

// ------------------------------------------ Selectable tags catalogue (CRD 1701-1713)

#[derive(Deserialize)]
pub struct AvailableTagsQuery {
    pub page: Option<String>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<String>,
    pub search: Option<String>,
    #[serde(rename = "includeGlobal")]
    pub include_global: Option<String>,
}

pub async fn available_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<AvailableTagsQuery>,
) -> Result {
    let page = q.page.as_deref().and_then(|p| p.parse::<i64>().ok()).unwrap_or(1).max(1);
    let size = q
        .page_size
        .as_deref()
        .and_then(|p| p.parse::<i64>().ok())
        .unwrap_or(100)
        .max(1);
    let include_global = q.include_global.as_deref() != Some("false");

    // Team scoping (CRD 1554/1709): non-admins are restricted to their team's tags
    // plus (optionally) global team-less tags; admins see all unless includeGlobal=false
    // excludes the team-agnostic ones.
    let (scope_sql, team_bind): (&str, Option<i64>) = if user.is_admin() {
        if include_global {
            ("", None)
        } else {
            (" AND t.team_id IS NOT NULL", None)
        }
    } else if let Some(team) = user.primary_team_id {
        if include_global {
            (" AND (t.team_id = ? OR t.team_id IS NULL)", Some(team))
        } else {
            (" AND t.team_id = ?", Some(team))
        }
    } else if include_global {
        (" AND t.team_id IS NULL", None)
    } else {
        (" AND 1 = 0", None)
    };

    // Wildcards in the search text are matched literally (CRD 1706).
    let pattern = q
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", escape_like(s)));
    let search_sql = if pattern.is_some() {
        " AND (t.name LIKE ? ESCAPE '\\' OR t.description LIKE ? ESCAPE '\\')"
    } else {
        ""
    };

    let base = format!("t.is_active = 1 AND t.deleted_at IS NULL{scope_sql}{search_sql}");

    let count_sql = format!("SELECT COUNT(*) FROM tags t WHERE {base}");
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    if let Some(team) = team_bind {
        count_q = count_q.bind(team);
    }
    if let Some(p) = &pattern {
        count_q = count_q.bind(p.clone()).bind(p.clone());
    }
    let total = count_q.fetch_one(&state.db).await?;

    let rows_sql = format!(
        "SELECT t.id, t.name, t.color, t.description, t.team_id, t.is_active, t.created_by, \
         t.created_at, t.updated_at, {TAG_COUNT_COLUMNS} \
         FROM tags t WHERE {base} ORDER BY t.name ASC LIMIT ? OFFSET ?"
    );
    let mut rows_q = sqlx::query_as::<_, TagWithCounts>(&rows_sql);
    if let Some(team) = team_bind {
        rows_q = rows_q.bind(team);
    }
    if let Some(p) = &pattern {
        rows_q = rows_q.bind(p.clone()).bind(p.clone());
    }
    let rows = rows_q.bind(size).bind((page - 1) * size).fetch_all(&state.db).await?;

    let items: Vec<Value> = rows
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "color": t.color,
                "description": t.description,
                "teamId": t.team_id,
                "isActive": t.is_active != 0,
                "createdBy": t.created_by,
                "createdAt": t.created_at,
                "updatedAt": t.updated_at,
                "customerCount": t.customer_count,
                "conversationCount": t.conversation_count,
            })
        })
        .collect();

    // Top-level shape per CRD 1556/1710: data array + sibling pagination + message.
    let body = json!({
        "success": true,
        "data": items,
        "pagination": {
            "page": page,
            "limit": size,
            "total": total,
            "totalPages": total_pages(total, size),
        },
        "message": "Available tags retrieved successfully",
        "timestamp": crate::db::now_iso(),
        "requestId": envelope::request_id(),
    });
    Ok((StatusCode::OK, Json(body)).into_response())
}

// --------------------------------------------- Get a customer's labels (CRD 1691-1699)

pub async fn get_customer_tags(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_customer_id(&raw_id)?;
    require_customer(&state, id).await?;

    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        name: String,
        color: String,
        description: Option<String>,
        team_id: Option<i64>,
        assigned_at: String,
        assigned_by: Option<String>,
    }
    // Only tags currently marked active are returned (CRD 1699).
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT t.id, t.name, t.color, t.description, t.team_id,
                ct.created_at AS assigned_at, ct.assigned_by
         FROM customer_tags ct
         JOIN tags t ON t.id = ct.tag_id AND t.is_active = 1 AND t.deleted_at IS NULL
         WHERE ct.customer_id = ?
         ORDER BY ct.created_at DESC, ct.id DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    Ok(envelope::ok_msg(
        rows.iter()
            .map(|r| json!({
                "id": r.id,
                "name": r.name,
                "color": r.color,
                "description": r.description,
                "teamId": r.team_id,
                "assignedAt": r.assigned_at,
                "assignedBy": r.assigned_by,
            }))
            .collect::<Vec<_>>(),
        "Customer tags retrieved successfully",
    ))
}

// ---------------------------------------------- Add labels to a customer (CRD 1715-1728)

#[derive(Deserialize)]
pub struct TagIdsBody {
    #[serde(rename = "tagIds")]
    pub tag_ids: Option<Value>,
}

fn dedup(ids: &[i64]) -> Vec<i64> {
    let mut seen = HashSet::new();
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

/// Every supplied tag must exist and be active (CRD 1725, 1753).
async fn require_active_tags(state: &AppState, ids: &[i64]) -> Result<()> {
    let valid: HashSet<i64> = store::active_tag_ids(&state.db, ids).await?.into_iter().collect();
    if ids.iter().any(|id| !valid.contains(id)) {
        return Err(validation("tagIds", "Some tag IDs are invalid or inactive"));
    }
    Ok(())
}

/// The actor identity must be resolvable so the assigner can be recorded (CRD 1726).
fn require_actor(user: &AuthUser) -> Result<()> {
    if user.id.is_empty() {
        return Err(AppError::Unauthorized("Unauthorized: User ID not found in token".into()));
    }
    Ok(())
}

pub async fn add_customer_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<TagIdsBody>,
) -> Result {
    let id = parse_customer_id(&raw_id)?;
    let body = parse_json(body)?;
    let requested = coerce_tag_ids(body.tag_ids.as_ref())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| validation("tagIds", "Tag IDs must be a non-empty array"))?;
    let customer = require_customer(&state, id).await?;
    let ids = dedup(&requested);
    require_active_tags(&state, &ids).await?;

    let placeholders = vec!["?"; ids.len()].join(", ");
    let existing_sql = format!(
        "SELECT tag_id FROM customer_tags WHERE customer_id = ? AND tag_id IN ({placeholders})"
    );
    let mut existing_q = sqlx::query_scalar::<_, i64>(&existing_sql).bind(id);
    for tag_id in &ids {
        existing_q = existing_q.bind(tag_id);
    }
    let existing: HashSet<i64> = existing_q.fetch_all(&state.db).await?.into_iter().collect();
    let to_add: Vec<i64> = ids.iter().copied().filter(|t| !existing.contains(t)).collect();

    if !to_add.is_empty() {
        require_actor(&user)?;
        // New associations and their reversible audit entries persist atomically
        // (CRD 1573: one "tag assign" entry per added association).
        let now = crate::db::now_iso();
        let mut tx = state.db.begin().await?;
        for tag_id in &to_add {
            sqlx::query(
                "INSERT INTO customer_tags (customer_id, tag_id, assigned_by, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(tag_id)
            .bind(&user.id)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO activity_logs (agent_id, agent_name, agent_role, action, resource_type, resource_id, details, created_at)
                 VALUES (?, ?, ?, 'tag assign', 'customer_tag', ?, ?, ?)",
            )
            .bind(&user.id)
            .bind(&user.display_name)
            .bind(&user.role)
            .bind(tag_id.to_string())
            .bind(json!({ "reversible": true, "customerId": id, "tagId": tag_id }).to_string())
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        // Realtime: `customer_tags_updated` (operation "add") delivered to
        // administrators and agents (CRD 1573, 1637, 3461); emission is
        // non-fatal by construction.
        state.realtime.global(
            "customer_tags_updated",
            json!({
                "customerId": customer.id,
                "operation": "add",
                "tagIds": &to_add,
                "changedBy": { "id": user.id, "name": user.display_name },
                "timestamp": crate::db::now_iso(),
            }),
        );
    }

    let added = to_add.len();
    let already = ids.len() - added;
    Ok(envelope::ok_msg(
        json!({ "added": added, "alreadyExists": already }),
        &format!("Added {added} tag(s) to customer"),
    ))
}

// ------------------------------------------ Remove labels from a customer (CRD 1730-1741)

pub async fn remove_customer_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<TagIdsBody>,
) -> Result {
    let id = parse_customer_id(&raw_id)?;
    let body = parse_json(body)?;
    let requested = coerce_tag_ids(body.tag_ids.as_ref())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| validation("tagIds", "Tag IDs must be a non-empty array"))?;
    require_customer(&state, id).await?;
    let ids = dedup(&requested);

    // Capture existing associations before removal so each is reversible (CRD 1580).
    let placeholders = vec!["?"; ids.len()].join(", ");
    let existing_sql = format!(
        "SELECT tag_id, assigned_by, created_at FROM customer_tags
         WHERE customer_id = ? AND tag_id IN ({placeholders})"
    );
    let mut existing_q =
        sqlx::query_as::<_, (i64, Option<String>, String)>(&existing_sql).bind(id);
    for tag_id in &ids {
        existing_q = existing_q.bind(tag_id);
    }
    let existing = existing_q.fetch_all(&state.db).await?;

    let now = crate::db::now_iso();
    let mut tx = state.db.begin().await?;
    let delete_sql = format!(
        "DELETE FROM customer_tags WHERE customer_id = ? AND tag_id IN ({placeholders})"
    );
    let mut delete_q = sqlx::query(&delete_sql).bind(id);
    for tag_id in &ids {
        delete_q = delete_q.bind(tag_id);
    }
    delete_q.execute(&mut *tx).await?;
    // One reversible "tag unassign" audit entry per previously-existing association (CRD 1582).
    for (tag_id, assigned_by, assigned_at) in &existing {
        sqlx::query(
            "INSERT INTO activity_logs (agent_id, agent_name, agent_role, action, resource_type, resource_id, details, created_at)
             VALUES (?, ?, ?, 'tag unassign', 'customer_tag', ?, ?, ?)",
        )
        .bind(&user.id)
        .bind(&user.display_name)
        .bind(&user.role)
        .bind(tag_id.to_string())
        .bind(
            json!({
                "reversible": true,
                "customerId": id,
                "tagId": tag_id,
                "assignedBy": assigned_by,
                "assignedAt": assigned_at,
            })
            .to_string(),
        )
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // Realtime: `customer_tags_updated` (operation "remove") to administrators
    // and agents (CRD 1582, 1637, 3461); non-fatal by construction.
    state.realtime.global(
        "customer_tags_updated",
        json!({
            "customerId": id,
            "operation": "remove",
            "tagIds": &ids,
            "changedBy": { "id": user.id, "name": user.display_name },
            "timestamp": crate::db::now_iso(),
        }),
    );

    // Reported count equals the size of the requested list (CRD 1581).
    Ok(envelope::with_status(
        StatusCode::OK,
        Some(Value::Null),
        Some(&format!("Removed {} tag(s) from customer", requested.len())),
    ))
}

// ----------------------------------------- Replace a customer's labels (CRD 1743-1756)

pub async fn replace_customer_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<TagIdsBody>,
) -> Result {
    let id = parse_customer_id(&raw_id)?;
    let body = parse_json(body)?;
    // An empty array is valid here: it clears all labels (CRD 1587, 1751).
    let requested = coerce_tag_ids(body.tag_ids.as_ref())
        .ok_or_else(|| validation("tagIds", "Tag IDs must be an array"))?;
    let customer = require_customer(&state, id).await?;
    let ids = dedup(&requested);
    if !ids.is_empty() {
        require_active_tags(&state, &ids).await?;
        require_actor(&user)?;
    }

    let now = crate::db::now_iso();
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM customer_tags WHERE customer_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for tag_id in &ids {
        sqlx::query(
            "INSERT INTO customer_tags (customer_id, tag_id, assigned_by, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(tag_id)
        .bind(&user.id)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // Single non-reversible, best-effort summary entry naming the resulting set (CRD 1591).
    let tag_names: Vec<String> = if ids.is_empty() {
        vec![]
    } else {
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!("SELECT name FROM tags WHERE id IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for tag_id in &ids {
            q = q.bind(tag_id);
        }
        q.fetch_all(&state.db).await.unwrap_or_default()
    };
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "tag assign", "customer", Some(&id.to_string()),
        Some(json!({
            "reversible": false,
            "operation": "set",
            "customerName": customer.display_name,
            "tagIds": ids,
            "tagNames": tag_names,
        })),
        None, None,
    )
    .await;

    // Realtime: `customer_tags_updated` (operation "set") to administrators
    // and agents (CRD 1591, 1637, 3461); non-fatal by construction.
    state.realtime.global(
        "customer_tags_updated",
        json!({
            "customerId": id,
            "operation": "set",
            "tagIds": &ids,
            "changedBy": { "id": user.id, "name": user.display_name },
            "timestamp": crate::db::now_iso(),
        }),
    );

    Ok(envelope::ok_msg(
        json!({ "totalTags": ids.len() }),
        "Customer tags updated successfully",
    ))
}

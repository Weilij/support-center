//! Label-management and conversation-label handlers (CRD §2.6, lines 1453-1644).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::domain::auth::store::log_activity;
use crate::envelope;
use crate::error::{AppError, FieldProblem};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::store::{self, total_pages, TagWithCounts};

type Result<T = Response> = std::result::Result<T, AppError>;
type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

pub(crate) fn validation(field: &str, message: &str) -> AppError {
    AppError::Validation(
        message.to_string(),
        vec![FieldProblem { field: field.into(), message: message.into(), value: None }],
    )
}

/// Malformed JSON bodies are reported as 400 "Invalid JSON" (CRD 1490, 1525).
pub(crate) fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b).map_err(|_| AppError::BadRequest("Invalid JSON".into()))
}

/// Path id must be a positive integer (CRD 1507, 1516: 400 "Invalid tag id").
fn parse_tag_id(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| AppError::BadRequest("Invalid tag id".into()))
}

/// Validate a `#RGB` / `#RRGGBB` color and normalize to uppercase 6-digit HEX (CRD 1487).
fn normalize_color(raw: &str) -> Option<String> {
    let rest = raw.strip_prefix('#')?;
    if !rest.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match rest.len() {
        3 => Some(format!(
            "#{}",
            rest.chars().flat_map(|c| [c, c]).collect::<String>().to_uppercase()
        )),
        6 => Some(format!("#{}", rest.to_uppercase())),
        _ => None,
    }
}

const DEFAULT_COLOR: &str = "#3B82F6";
const INVALID_COLOR_MSG: &str = "Color must be a valid HEX color (e.g. #3B82F6)";

fn lenient_i64(raw: &Option<String>) -> Option<i64> {
    raw.as_deref().and_then(|v| v.trim().parse::<i64>().ok())
}

// ------------------------------------------------------------- Health probe (CRD 1467-1473)

pub async fn health() -> Response {
    envelope::ok_msg(
        json!({
            "status": "healthy",
            "handler": "tag-handler",
            "timestamp": crate::db::now_iso(),
        }),
        "Tag handler is operational",
    )
}

// -------------------------------------------------------------- List labels (CRD 1475-1481)

#[derive(Deserialize)]
pub struct ListTagsQuery {
    pub page: Option<String>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<String>,
    pub search: Option<String>,
}

/// Spec ambiguity resolved: the management listing excludes only soft-deleted labels
/// (CRD 1631 enumerates soft-delete as the sole list exclusion, and each item carries
/// an `isActive` flag); inactive labels remain visible so they can be re-activated.
pub async fn list_tags(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ListTagsQuery>,
) -> Result {
    let page = lenient_i64(&q.page).unwrap_or(1).max(1);
    let size = lenient_i64(&q.page_size).unwrap_or(50).clamp(1, 100);
    let (rows, total) = store::list_tags(&state.db, page, size, q.search.as_deref()).await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "color": t.color,
                "description": t.description,
                // Always null in this listing (CRD 1480).
                "teamId": null,
                "teamName": null,
                "isActive": t.is_active != 0,
                "createdBy": t.created_by,
                "createdByName": null,
                "customerCount": t.customer_count,
                "conversationCount": t.conversation_count,
                "createdAt": t.created_at,
                "updatedAt": t.updated_at,
            })
        })
        .collect();
    Ok(envelope::paginated(&items, page, size, total))
}

// ------------------------------------------------------------- Create label (CRD 1483-1490)

#[derive(Deserialize)]
pub struct CreateTagBody {
    pub name: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    // A `teamId` field is accepted but ignored (CRD 1485) — serde drops unknown fields.
}

pub async fn create_tag(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<CreateTagBody>,
) -> Result {
    let body = parse_json(body)?;
    let name = body.name.as_deref().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Tag name is required".into()));
    }
    let color = match body.color.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        None => DEFAULT_COLOR.to_string(),
        Some(c) => normalize_color(c).ok_or_else(|| validation("color", INVALID_COLOR_MSG))?,
    };
    if store::name_in_use(&state.db, &name, None).await? {
        return Err(AppError::Conflict("A tag with this name already exists".into()));
    }

    let now = crate::db::now_iso();
    let id = sqlx::query(
        "INSERT INTO tags (name, color, description, team_id, is_active, created_by, created_at, updated_at)
         VALUES (?, ?, ?, NULL, 1, ?, ?, ?)",
    )
    .bind(&name)
    .bind(&color)
    .bind(&body.description)
    .bind(&user.id)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?
    .last_insert_rowid();

    // Reversible audit entry capturing prior absence and the new state (CRD 1489).
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "tag create", "tag", Some(&id.to_string()),
        Some(json!({
            "reversible": true,
            "old": null,
            "new": { "name": name, "color": color, "description": body.description, "isActive": true },
        })),
        None, None,
    )
    .await;

    Ok(envelope::with_status(
        StatusCode::CREATED,
        Some(json!({
            "id": id,
            "name": name,
            "color": color,
            "description": body.description,
            "teamId": null,
            "isActive": true,
            "createdBy": user.id,
            "customerCount": 0,
            "conversationCount": 0,
            "createdAt": now,
            "updatedAt": now,
        })),
        Some("Tag created successfully"),
    ))
}

// --------------------------------------------------------- Get single label (CRD 1492-1498)

pub async fn get_tag(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_tag_id(&raw_id)?;
    // May return a soft-deleted label: this endpoint does not exclude deleted records (CRD 1497).
    let t = store::tag_detail(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Tag not found".into()))?;
    Ok(envelope::ok(json!({
        "id": t.id,
        "name": t.name,
        "color": t.color,
        "description": t.description,
        "teamId": t.team_id,
        "teamName": t.team_name,
        "isActive": t.is_active != 0,
        "createdBy": t.created_by,
        "createdByName": t.created_by_name,
        "customerCount": t.customer_count,
        "conversationCount": t.conversation_count,
        "createdAt": t.created_at,
        "updatedAt": t.updated_at,
    })))
}

// ------------------------------------------------------------- Update label (CRD 1500-1507)

#[derive(Deserialize)]
pub struct UpdateTagBody {
    pub name: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "isActive")]
    pub is_active: Option<bool>,
}

enum Bind {
    S(String),
    I(i64),
}

fn updated_tag_view(t: &TagWithCounts, customer_count: i64, conversation_count: i64) -> Value {
    json!({
        "id": t.id,
        "name": t.name,
        "color": t.color,
        "description": t.description,
        "teamId": t.team_id,
        "isActive": t.is_active != 0,
        "createdBy": t.created_by,
        "customerCount": customer_count,
        "conversationCount": conversation_count,
        "createdAt": t.created_at,
        "updatedAt": t.updated_at,
    })
}

pub async fn update_tag(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<UpdateTagBody>,
) -> Result {
    let id = parse_tag_id(&raw_id)?;
    let body = parse_json(body)?;
    let current = store::find_live_tag(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Tag not found".into()))?;

    let mut old = serde_json::Map::new();
    let mut new = serde_json::Map::new();
    let mut sets: Vec<(&str, Bind)> = Vec::new();

    if let Some(raw_name) = &body.name {
        let name = raw_name.trim().to_string();
        if name.is_empty() {
            return Err(validation("name", "Tag name cannot be empty"));
        }
        if name != current.name {
            if store::name_in_use(&state.db, &name, Some(id)).await? {
                return Err(validation("name", "A tag with this name already exists"));
            }
            old.insert("name".into(), json!(current.name));
            new.insert("name".into(), json!(name));
            sets.push(("name", Bind::S(name)));
        }
    }
    if let Some(raw_color) = &body.color {
        // Validated even when unchanged (CRD 1507: invalid color -> 422 on `color`).
        let color = normalize_color(raw_color.trim())
            .ok_or_else(|| validation("color", INVALID_COLOR_MSG))?;
        if color != current.color {
            old.insert("color".into(), json!(current.color));
            new.insert("color".into(), json!(color));
            sets.push(("color", Bind::S(color)));
        }
    }
    if let Some(description) = &body.description {
        if Some(description.as_str()) != current.description.as_deref() {
            old.insert("description".into(), json!(current.description));
            new.insert("description".into(), json!(description));
            sets.push(("description", Bind::S(description.clone())));
        }
    }
    if let Some(active) = body.is_active {
        if active != (current.is_active != 0) {
            old.insert("isActive".into(), json!(current.is_active != 0));
            new.insert("isActive".into(), json!(active));
            sets.push(("is_active", Bind::I(active as i64)));
        }
    }

    // No effective change: no write, no audit entry; counts reported as 0 (CRD 1504-1505).
    if sets.is_empty() {
        let view = TagWithCounts {
            id: current.id,
            name: current.name,
            color: current.color,
            description: current.description,
            team_id: current.team_id,
            is_active: current.is_active,
            created_by: current.created_by,
            created_at: current.created_at,
            updated_at: current.updated_at,
            customer_count: 0,
            conversation_count: 0,
        };
        return Ok(envelope::ok_msg(updated_tag_view(&view, 0, 0), "No changes made"));
    }

    let now = crate::db::now_iso();
    let assignments =
        sets.iter().map(|(col, _)| format!("{col} = ?")).collect::<Vec<_>>().join(", ");
    let sql = format!("UPDATE tags SET {assignments}, updated_at = ? WHERE id = ?");
    let mut q = sqlx::query(&sql);
    for (_, b) in &sets {
        q = match b {
            Bind::S(s) => q.bind(s.clone()),
            Bind::I(i) => q.bind(*i),
        };
    }
    q.bind(&now).bind(id).execute(&state.db).await?;

    // Reversible audit entry capturing only the changed fields (CRD 1506).
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "tag update", "tag", Some(&id.to_string()),
        Some(json!({ "reversible": true, "old": old, "new": new })),
        None, None,
    )
    .await;

    let updated = store::tag_with_counts(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload tag after update".into()))?;
    let (cc, vc) = (updated.customer_count, updated.conversation_count);
    Ok(envelope::ok_msg(updated_tag_view(&updated, cc, vc), "Tag updated successfully"))
}

// -------------------------------------------------------- Soft-delete label (CRD 1509-1516)

pub async fn delete_tag(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_tag_id(&raw_id)?;
    let current = store::find_live_tag(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Tag not found".into()))?;

    let now = crate::db::now_iso();
    sqlx::query("UPDATE tags SET is_active = 0, deleted_at = ?, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(&state.db)
        .await?;

    // Reversible audit entry capturing prior active/deleted state (CRD 1515).
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "tag delete", "tag", Some(&id.to_string()),
        Some(json!({
            "reversible": true,
            "old": { "isActive": current.is_active != 0, "deletedAt": null },
            "new": { "isActive": false, "deletedAt": now },
        })),
        None, None,
    )
    .await;

    Ok(envelope::with_status(
        StatusCode::OK,
        Some(Value::Null),
        Some("Tag deleted successfully"),
    ))
}

// ----------------------------------------------------- Bulk label operation (CRD 1518-1525)

#[derive(Deserialize)]
pub struct BulkBody {
    pub operation: Option<String>,
    #[serde(rename = "tagIds")]
    pub tag_ids: Option<Value>,
    pub data: Option<Value>,
}

pub async fn bulk_operation(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    body: JsonBody<BulkBody>,
) -> Result {
    let body = parse_json(body)?;
    let arr = body
        .tag_ids
        .as_ref()
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| validation("tagIds", "tagIds must be a non-empty array"))?;

    // Each element must be a number or an all-digits string (CRD 1520, 1525).
    let mut ids: Vec<i64> = Vec::with_capacity(arr.len());
    for v in arr {
        let id = match v {
            Value::Number(n) => n.as_i64(),
            Value::String(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) => {
                s.parse::<i64>().ok()
            }
            _ => None,
        };
        ids.push(id.ok_or_else(|| {
            AppError::BadRequest("Invalid tag ID format detected".into())
        })?);
    }

    let placeholders = vec!["?"; ids.len()].join(", ");
    let now = crate::db::now_iso();
    let op = body.operation.as_deref().unwrap_or("");
    match op {
        "activate" | "deactivate" => {
            let sql = format!(
                "UPDATE tags SET is_active = ?, updated_at = ? WHERE id IN ({placeholders})"
            );
            let mut q = sqlx::query(&sql).bind((op == "activate") as i64).bind(&now);
            for id in &ids {
                q = q.bind(id);
            }
            q.execute(&state.db).await?;
        }
        "update_color" => {
            // Bulk color values are stored as supplied — not HEX-validated/normalized (CRD 1522).
            let color = body
                .data
                .as_ref()
                .and_then(|d| d.get("color"))
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .ok_or_else(|| {
                    validation("data.color", "Color is required for update_color operation")
                })?
                .to_string();
            let sql =
                format!("UPDATE tags SET color = ?, updated_at = ? WHERE id IN ({placeholders})");
            let mut q = sqlx::query(&sql).bind(color).bind(&now);
            for id in &ids {
                q = q.bind(id);
            }
            q.execute(&state.db).await?;
        }
        _ => {
            return Err(validation(
                "operation",
                "operation must be one of: activate, deactivate, update_color",
            ))
        }
    }

    // No per-label reversible audit entries for bulk operations (CRD 1524).
    Ok(envelope::with_status(
        StatusCode::OK,
        Some(Value::Null),
        Some(&format!("Bulk operation '{op}' completed successfully")),
    ))
}

// ------------------------------------------------------- Usage statistics (CRD 1527-1533)

pub async fn tag_stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_tag_id(&raw_id)?;
    let tag: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, name, color FROM tags WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (tag_id, name, color) = tag.ok_or_else(|| AppError::NotFound("Tag not found".into()))?;

    let (cust_total, cust_line, cust_facebook): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(DISTINCT c.id),
                COUNT(DISTINCT CASE WHEN c.platform = 'line' THEN c.id END),
                COUNT(DISTINCT CASE WHEN c.platform = 'facebook' THEN c.id END)
         FROM customer_tags ct
         JOIN customers c ON c.id = ct.customer_id AND c.deleted_at IS NULL
         WHERE ct.tag_id = ?",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    let (conv_total, conv_active, conv_closed): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(DISTINCT cv.id),
                COUNT(DISTINCT CASE WHEN cv.status = 'active' THEN cv.id END),
                COUNT(DISTINCT CASE WHEN cv.status = 'closed' THEN cv.id END)
         FROM customer_tags ct
         JOIN customers c ON c.id = ct.customer_id AND c.deleted_at IS NULL
         JOIN conversations cv ON cv.customer_id = c.id AND cv.deleted_at IS NULL
         WHERE ct.tag_id = ?",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    let cutoff = (chrono::Utc::now() - chrono::Duration::days(30))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let trend: Vec<(String, i64)> = sqlx::query_as(
        "SELECT substr(created_at, 1, 10) AS day, COUNT(*) AS assignments
         FROM customer_tags WHERE tag_id = ? AND created_at >= ?
         GROUP BY day ORDER BY day DESC LIMIT 30",
    )
    .bind(id)
    .bind(&cutoff)
    .fetch_all(&state.db)
    .await?;

    let assigners: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(a.display_name, ct.assigned_by, 'Unknown') AS name, COUNT(*) AS assignments
         FROM customer_tags ct
         LEFT JOIN agents a ON a.id = ct.assigned_by
         WHERE ct.tag_id = ? AND ct.created_at >= ?
         GROUP BY name ORDER BY assignments DESC LIMIT 10",
    )
    .bind(id)
    .bind(&cutoff)
    .fetch_all(&state.db)
    .await?;

    Ok(envelope::ok(json!({
        "tagInfo": { "id": tag_id, "name": name, "color": color },
        "customers": {
            "total": cust_total,
            "byPlatform": { "line": cust_line, "facebook": cust_facebook },
        },
        "conversations": { "total": conv_total, "active": conv_active, "closed": conv_closed },
        "usageTrend": trend
            .iter()
            .map(|(date, n)| json!({ "date": date, "assignments": n }))
            .collect::<Vec<_>>(),
        "topAssigners": assigners
            .iter()
            .map(|(name, n)| json!({ "name": name, "assignments": n }))
            .collect::<Vec<_>>(),
    })))
}

// ------------------------------------------------- Label's customers list (CRD 1535-1541)

#[derive(Deserialize)]
pub struct PageLimitQuery {
    pub page: Option<String>,
    pub limit: Option<String>,
}

/// The label must be active and non-deleted to be found here (CRD 1538).
async fn require_active_tag(state: &AppState, id: i64) -> Result<()> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM tags WHERE id = ? AND is_active = 1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    found.map(|_| ()).ok_or_else(|| AppError::NotFound("Tag not found".into()))
}

pub async fn tag_customers(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    Query(q): Query<PageLimitQuery>,
) -> Result {
    let id = parse_tag_id(&raw_id)?;
    require_active_tag(&state, id).await?;
    let page = lenient_i64(&q.page).unwrap_or(1).max(1);
    let limit = lenient_i64(&q.limit).unwrap_or(50).clamp(1, 100);

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM customer_tags ct
         JOIN customers c ON c.id = ct.customer_id AND c.deleted_at IS NULL
         WHERE ct.tag_id = ?",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        platform: String,
        platform_user_id: String,
        display_name: Option<String>,
        avatar_url: Option<String>,
        email: Option<String>,
        phone: Option<String>,
        created_at: String,
        assigned_at: String,
        assigned_by: Option<String>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT c.id, c.platform, c.platform_user_id, c.display_name, c.avatar_url,
                c.email, c.phone, c.created_at, ct.created_at AS assigned_at, ct.assigned_by
         FROM customer_tags ct
         JOIN customers c ON c.id = ct.customer_id AND c.deleted_at IS NULL
         WHERE ct.tag_id = ?
         ORDER BY ct.created_at DESC, ct.id DESC LIMIT ? OFFSET ?",
    )
    .bind(id)
    .bind(limit)
    .bind((page - 1) * limit)
    .fetch_all(&state.db)
    .await?;

    Ok(envelope::ok(json!({
        "customers": rows
            .iter()
            .map(|r| json!({
                "id": r.id,
                "platform": r.platform,
                "platform_user_id": r.platform_user_id,
                "display_name": r.display_name,
                "avatar_url": r.avatar_url,
                "email": r.email,
                "phone": r.phone,
                "created_at": r.created_at,
                "assigned_at": r.assigned_at,
                "assigned_by": r.assigned_by,
            }))
            .collect::<Vec<_>>(),
        "pagination": {
            "page": page,
            "limit": limit,
            "total": total,
            "totalPages": total_pages(total, limit),
        },
    })))
}

// --------------------------------------------- Label's conversations list (CRD 1543-1549)

pub async fn tag_conversations(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    Query(q): Query<PageLimitQuery>,
) -> Result {
    let id = parse_tag_id(&raw_id)?;
    require_active_tag(&state, id).await?;
    let page = lenient_i64(&q.page).unwrap_or(1).max(1);
    let limit = lenient_i64(&q.limit).unwrap_or(20).clamp(1, 100);

    // Conversations are reached through the customers holding the label (CRD 1547).
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT cv.id)
         FROM customer_tags ct
         JOIN customers c ON c.id = ct.customer_id AND c.deleted_at IS NULL
         JOIN conversations cv ON cv.customer_id = c.id AND cv.deleted_at IS NULL
         WHERE ct.tag_id = ?",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        status: String,
        channel: String,
        created_at: String,
        updated_at: Option<String>,
        customer_name: Option<String>,
        customer_avatar: Option<String>,
        customer_platform: String,
        assigned_at: String,
        assigned_by: Option<String>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT DISTINCT cv.id, cv.status, c.platform AS channel, cv.created_at, cv.updated_at,
                c.display_name AS customer_name, c.avatar_url AS customer_avatar,
                c.platform AS customer_platform, ct.created_at AS assigned_at, ct.assigned_by
         FROM customer_tags ct
         JOIN customers c ON c.id = ct.customer_id AND c.deleted_at IS NULL
         JOIN conversations cv ON cv.customer_id = c.id AND cv.deleted_at IS NULL
         WHERE ct.tag_id = ?
         ORDER BY ct.created_at DESC, cv.id LIMIT ? OFFSET ?",
    )
    .bind(id)
    .bind(limit)
    .bind((page - 1) * limit)
    .fetch_all(&state.db)
    .await?;

    Ok(envelope::ok(json!({
        "conversations": rows
            .iter()
            .map(|r| json!({
                "id": r.id,
                "status": r.status,
                "channel": r.channel,
                "created_at": r.created_at,
                "updated_at": r.updated_at,
                "customer_name": r.customer_name,
                "customer_avatar": r.customer_avatar,
                "customer_platform": r.customer_platform,
                "assigned_at": r.assigned_at,
                "assigned_by": r.assigned_by,
            }))
            .collect::<Vec<_>>(),
        "pagination": {
            "page": page,
            "limit": limit,
            "total": total,
            "totalPages": total_pages(total, limit),
        },
    })))
}

// ------------------------------------------- Conversation-label endpoints (CRD 1594-1618)

async fn require_conversation(state: &AppState, id: &str) -> Result<()> {
    let found: Option<String> =
        sqlx::query_scalar("SELECT id FROM conversations WHERE id = ? AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    found.map(|_| ()).ok_or_else(|| AppError::NotFound("Conversation not found".into()))
}

#[derive(Deserialize)]
pub struct TagIdsBody {
    #[serde(rename = "tagIds")]
    pub tag_ids: Option<Value>,
}

/// Coerce a `tagIds` JSON array into integers (CRD 1604: values coerced to integers).
/// Returns None when the value is missing or not an array.
pub(crate) fn coerce_tag_ids(v: Option<&Value>) -> Option<Vec<i64>> {
    let arr = v?.as_array()?;
    Some(
        arr.iter()
            .map(|e| match e {
                Value::Number(n) => n.as_i64().unwrap_or(-1),
                Value::String(s) => s.trim().parse::<i64>().unwrap_or(-1),
                _ => -1,
            })
            .collect(),
    )
}

pub async fn conversation_tags(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    require_conversation(&state, &id).await?;
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        name: String,
        color: String,
        description: Option<String>,
        assigned_by: Option<String>,
        assigned_at: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT t.id, t.name, t.color, t.description, vt.assigned_by, vt.created_at AS assigned_at
         FROM conversation_tags vt
         JOIN tags t ON t.id = vt.tag_id AND t.is_active = 1 AND t.deleted_at IS NULL
         WHERE vt.conversation_id = ?
         ORDER BY vt.created_at DESC, vt.id DESC",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    Ok(envelope::ok_msg(
        rows.iter()
            .map(|r| json!({
                "id": r.id,
                "name": r.name,
                "color": r.color,
                "description": r.description,
                "assignedBy": r.assigned_by,
                "assignedAt": r.assigned_at,
            }))
            .collect::<Vec<_>>(),
        "Conversation tags retrieved successfully",
    ))
}

pub async fn add_conversation_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<TagIdsBody>,
) -> Result {
    let body = parse_json(body)?;
    let ids = coerce_tag_ids(body.tag_ids.as_ref())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| validation("tagIds", "Tag IDs must be a non-empty array"))?;
    require_conversation(&state, &id).await?;

    let now = crate::db::now_iso();
    for tag_id in &ids {
        // Pre-existing identical associations are ignored (CRD 1606); the SELECT guard
        // also skips identifiers that do not reference an existing tag.
        sqlx::query(
            "INSERT OR IGNORE INTO conversation_tags (conversation_id, tag_id, assigned_by, created_at)
             SELECT ?, id, ?, ? FROM tags WHERE id = ?",
        )
        .bind(&id)
        .bind(&user.id)
        .bind(&now)
        .bind(tag_id)
        .execute(&state.db)
        .await?;
    }

    // TODO(realtime): broadcast `conversation_tags_updated` (operation "add") with
    // { operation, tagIds, updatedBy: { id, name }, timestamp } to this conversation's
    // subscribers; emission failure must be non-fatal (CRD 1608, 1638).
    Ok(envelope::message_only("Tags added to conversation successfully"))
}

pub async fn remove_conversation_tags(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<TagIdsBody>,
) -> Result {
    let body = parse_json(body)?;
    let ids = coerce_tag_ids(body.tag_ids.as_ref())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| validation("tagIds", "Tag IDs must be a non-empty array"))?;
    require_conversation(&state, &id).await?;

    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "DELETE FROM conversation_tags WHERE conversation_id = ? AND tag_id IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql).bind(&id);
    for tag_id in &ids {
        q = q.bind(tag_id);
    }
    q.execute(&state.db).await?;

    // TODO(realtime): broadcast `conversation_tags_updated` (operation "remove") to this
    // conversation's subscribers; emission failure must be non-fatal (CRD 1617, 1638).
    Ok(envelope::message_only("Tags removed from conversation successfully"))
}

//! Conversation-Session Management handlers (CRD §1.2B, lines 329-483).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::store::{self, NewSession, SessionRow};
use super::topics;

type Result<T = Response> = std::result::Result<T, AppError>;
type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

const SESSION_TYPES: &[&str] = &["continuous", "scheduled", "support", "marketing"];
const SENDER_TYPES: &[&str] = &["customer", "agent", "system"];
const PRIORITIES: &[&str] = &["low", "medium", "high", "urgent"];
const SENTIMENTS: &[&str] = &["positive", "negative", "neutral"];

fn bad(msg: impl Into<String>) -> AppError {
    AppError::BadRequest(msg.into())
}

fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b).map_err(|_| bad("Invalid JSON"))
}

/// Session/conversation identifiers must be UUID v1-v5 (CRD 331).
fn require_uuid(raw: &str, label: &str) -> Result<String> {
    uuid::Uuid::parse_str(raw)
        .ok()
        .filter(|u| (1..=5).contains(&u.get_version_num()))
        .map(|_| raw.to_string())
        .ok_or_else(|| bad(format!("Invalid {label} format: must be a UUID")))
}

/// Input sanitization: trim and strip control characters (CRD 345).
fn sanitize(raw: &str) -> String {
    raw.trim().chars().filter(|c| !c.is_control()).collect()
}

fn require_enum(value: &str, allowed: &[&str], label: &str) -> Result<String> {
    if allowed.contains(&value) {
        Ok(value.to_string())
    } else {
        Err(bad(format!("{label} must be one of: {}", allowed.join(", "))))
    }
}

fn require_iso_date(value: &str, label: &str) -> Result<String> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|_| value.to_string())
        .map_err(|_| bad(format!("{label} must be a valid ISO-8601 date")))
}

/// `tags`: array, max 10 items, items sanitized, empties dropped (CRD 345, 373).
fn validate_tags(v: &Value) -> Result<Vec<String>> {
    let arr = v.as_array().ok_or_else(|| bad("tags must be an array"))?;
    if arr.len() > 10 {
        return Err(bad("tags can contain at most 10 items"));
    }
    Ok(arr
        .iter()
        .filter_map(Value::as_str)
        .map(sanitize)
        .filter(|t| !t.is_empty())
        .collect())
}

/// Success envelope carrying a `count` beside `data` (CRD 331, 361, 468).
fn ok_count(data: Value, count: usize, message: Option<&str>) -> Response {
    let mut body = json!({
        "success": true,
        "data": data,
        "count": count,
        "timestamp": crate::db::now_iso(),
    });
    if let Some(m) = message {
        body["message"] = json!(m);
    }
    (StatusCode::OK, Json(body)).into_response()
}

fn require_admin(user: &AuthUser, message: &str) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden(message.into()))
    }
}

/// Agent-level access to a session: the underlying conversation must be assigned
/// to one of the agent's teams (CRD 365, 372).
async fn has_team_access(
    state: &AppState,
    user: &AuthUser,
    session: &SessionRow,
) -> Result<bool> {
    if user.is_admin() {
        return Ok(true);
    }
    Ok(match store::conversation_team(&state.db, session).await? {
        Some(team_id) => user.teams.iter().any(|t| t.team_id == team_id),
        None => false,
    })
}

fn session_not_found() -> AppError {
    AppError::NotFound("Session not found".into())
}

// ------------------------------------------------------- Module health & info (CRD 335-341)

pub async fn health() -> Response {
    envelope::ok(json!({
        "status": "healthy",
        "module": "conversation-sessions",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub const ENDPOINTS: &[&str] = &[
    "GET /api/sessions/health",
    "GET /api/sessions/info",
    "POST /api/sessions",
    "GET /api/sessions",
    "GET /api/sessions/search",
    "GET /api/sessions/stats",
    "GET /api/sessions/stats/:conversationId",
    "GET /api/sessions/activity",
    "POST /api/sessions/batch",
    "POST /api/sessions/get-or-create",
    "POST /api/sessions/detect-boundary",
    "GET /api/sessions/topics/stats",
    "POST /api/sessions/topics/analyze",
    "POST /api/sessions/topics/suggest",
    "GET /api/sessions/:sessionId",
    "PUT /api/sessions/:sessionId",
    "DELETE /api/sessions/:sessionId",
    "POST /api/sessions/:sessionId/close",
    "POST /api/sessions/:sessionId/reopen",
    "GET /api/sessions/:sessionId/messages",
    "GET /api/sessions/:sessionId/health",
    "PUT /api/sessions/:sessionId/topic",
];

pub async fn info() -> Response {
    envelope::ok(json!({
        "module": "conversation-sessions",
        "version": env!("CARGO_PKG_VERSION"),
        "features": [
            "session CRUD", "search", "boundary detection", "topic analysis",
            "health checks", "statistics", "activity analytics", "batch operations",
        ],
        "endpoints": ENDPOINTS,
        "permissions": {
            "read": "admin, agent",
            "write": "admin, agent (team-scoped)",
            "delete": "admin",
            "statistics": "admin",
            "batch": "admin",
        },
    }))
}

/// Unknown paths under the module list the available endpoints (CRD 470-471).
pub async fn unmatched() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "success": false,
            "error": "Endpoint not found",
            "availableEndpoints": ENDPOINTS,
            "timestamp": crate::db::now_iso(),
        })),
    )
        .into_response()
}

// --------------------------------------------------------------- Create session (CRD 343-348)

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    let body = parse_json(body)?;
    let conversation_id = body
        .get("conversationId")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("conversationId is required"))?;
    let conversation_id = require_uuid(conversation_id, "conversationId")?;
    let sender_type = body
        .get("senderType")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("senderType is required"))?;
    require_enum(sender_type, SENDER_TYPES, "senderType")?;
    let session_type = match body.get("sessionType").and_then(Value::as_str) {
        Some(t) => require_enum(t, SESSION_TYPES, "sessionType")?,
        None => "continuous".to_string(),
    };
    let topic = match body.get("topic").and_then(Value::as_str) {
        Some(t) => {
            let t = sanitize(t);
            if t.chars().count() > 200 {
                return Err(bad("topic must be at most 200 characters"));
            }
            Some(t).filter(|t| !t.is_empty())
        }
        None => None,
    };
    let message_content = match body.get("messageContent").and_then(Value::as_str) {
        Some(c) => {
            let c = sanitize(c);
            if c.chars().count() > 2000 {
                return Err(bad("messageContent must be at most 2000 characters"));
            }
            Some(c)
        }
        None => None,
    };
    let priority = match body.get("priority").and_then(Value::as_str) {
        Some(p) => Some(require_enum(p, PRIORITIES, "priority")?),
        None => None,
    };
    let tags = match body.get("tags") {
        Some(v) => Some(validate_tags(v)?),
        None => None,
    };
    let metadata = match body.get("metadata") {
        Some(v) if v.is_object() => Some(v.clone()),
        Some(_) => return Err(bad("metadata must be an object")),
        None => None,
    };

    // A topic is auto-derived from the message content when none was given (CRD 345).
    let topic = topic.or_else(|| {
        message_content
            .as_deref()
            .filter(|c| !c.is_empty())
            .map(|c| topics::derive_topic(c).topic)
    });

    let session = store::create(
        &state.db,
        NewSession {
            conversation_id: &conversation_id,
            session_type: &session_type,
            topic,
            priority,
            tags,
            metadata,
        },
    )
    .await?;
    Ok(envelope::created(store::session_view(&session)))
}

// ----------------------------------------------------------------- List sessions (CRD 350-355)

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "isActive")]
    pub is_active: Option<String>,
    #[serde(rename = "sessionType")]
    pub session_type: Option<String>,
    pub priority: Option<String>,
    pub sentiment: Option<String>,
    #[serde(rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
    pub topic: Option<String>,
    pub tag: Option<String>,
    pub page: Option<String>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<String>,
}

/// Any invalid filter value yields 400 rather than being clamped (CRD 355).
fn parse_range(raw: Option<&str>, default: i64, min: i64, max: i64, label: &str) -> Result<i64> {
    match raw {
        None => Ok(default),
        Some(s) => s
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|v| (min..=max).contains(v))
            .ok_or_else(|| bad(format!("{label} must be between {min} and {max}"))),
    }
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    let mut clause = String::from("WHERE 1 = 1");
    let mut binds: Vec<String> = Vec::new();
    if let Some(cid) = q.conversation_id.as_deref() {
        let cid = require_uuid(cid, "conversationId")?;
        clause.push_str(" AND conversation_id = ?");
        binds.push(cid);
    }
    match q.is_active.as_deref() {
        None => {}
        Some("true") => clause.push_str(" AND is_active = 1"),
        Some("false") => clause.push_str(" AND is_active = 0"),
        Some(_) => return Err(bad("isActive must be true or false")),
    }
    if let Some(t) = q.session_type.as_deref() {
        clause.push_str(" AND session_type = ?");
        binds.push(require_enum(t, SESSION_TYPES, "sessionType")?);
    }
    if let Some(p) = q.priority.as_deref() {
        clause.push_str(" AND priority = ?");
        binds.push(require_enum(p, PRIORITIES, "priority")?);
    }
    if let Some(s) = q.sentiment.as_deref() {
        clause.push_str(" AND sentiment = ?");
        binds.push(require_enum(s, SENTIMENTS, "sentiment")?);
    }
    if let Some(d) = q.start_date.as_deref() {
        clause.push_str(" AND COALESCE(started_at, created_at) >= ?");
        binds.push(require_iso_date(d, "startDate")?);
    }
    if let Some(d) = q.end_date.as_deref() {
        clause.push_str(" AND COALESCE(started_at, created_at) <= ?");
        binds.push(require_iso_date(d, "endDate")?);
    }
    if let Some(t) = q.topic.as_deref().map(sanitize).filter(|t| !t.is_empty()) {
        clause.push_str(" AND topic LIKE ?");
        binds.push(format!("%{t}%"));
    }
    if let Some(t) = q.tag.as_deref().map(sanitize).filter(|t| !t.is_empty()) {
        clause.push_str(" AND COALESCE(tags, '') LIKE ?");
        binds.push(format!("%{t}%"));
    }
    let page = parse_range(q.page.as_deref(), 1, 1, 1000, "page")?;
    let page_size = parse_range(q.page_size.as_deref(), 20, 1, 100, "pageSize")?;

    let summary = store::summarize(&state.db, &clause, &binds).await?;
    let total = summary["total"].as_i64().unwrap_or(0);

    let sql = format!(
        "{} {clause} ORDER BY COALESCE(last_activity_at, created_at) DESC, created_at DESC
         LIMIT ? OFFSET ?",
        store::SELECT
    );
    let mut query = sqlx::query_as::<_, SessionRow>(&sql);
    for b in &binds {
        query = query.bind(b.clone());
    }
    let rows = query.bind(page_size).bind((page - 1) * page_size).fetch_all(&state.db).await?;

    let total_pages = if total == 0 { 0 } else { (total + page_size - 1) / page_size };
    Ok(envelope::ok(json!({
        "sessions": rows.iter().map(store::session_view).collect::<Vec<_>>(),
        "pagination": {
            "page": page,
            "pageSize": page_size,
            "total": total,
            "totalPages": total_pages,
            "hasNext": page < total_pages,
            "hasPrev": page > 1 && total_pages > 0,
        },
        "summary": summary,
    })))
}

// --------------------------------------------------------------- Search sessions (CRD 357-362)

#[derive(Deserialize)]
pub struct SearchQuery {
    pub query: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "sessionType")]
    pub session_type: Option<String>,
    pub limit: Option<String>,
}

pub async fn search_sessions(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<SearchQuery>,
) -> Result {
    let term = sanitize(q.query.as_deref().unwrap_or(""));
    if term.chars().count() < 2 {
        return Err(bad("query is required and must be at least 2 characters"));
    }
    let mut clause = String::from("WHERE (topic LIKE ? OR COALESCE(tags, '') LIKE ? OR id = ?)");
    let like = format!("%{term}%");
    let mut binds = vec![like.clone(), like, term];
    if let Some(cid) = q.conversation_id.as_deref() {
        clause.push_str(" AND conversation_id = ?");
        binds.push(require_uuid(cid, "conversationId")?);
    }
    if let Some(t) = q.session_type.as_deref() {
        clause.push_str(" AND session_type = ?");
        binds.push(require_enum(t, SESSION_TYPES, "sessionType")?);
    }
    let limit = parse_range(q.limit.as_deref(), 20, 1, 100, "limit")?;

    let sql = format!(
        "{} {clause} ORDER BY COALESCE(last_activity_at, created_at) DESC LIMIT ?",
        store::SELECT
    );
    let mut query = sqlx::query_as::<_, SessionRow>(&sql);
    for b in &binds {
        query = query.bind(b.clone());
    }
    let rows = query.bind(limit).fetch_all(&state.db).await?;
    let items: Vec<Value> = rows.iter().map(store::session_view).collect();
    let count = items.len();
    Ok(ok_count(json!(items), count, None))
}

// ------------------------------------------------------------- Session details (CRD 364-369)

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = require_uuid(&raw_id, "session ID")?;
    let session = store::find(&state.db, &id).await?.ok_or_else(session_not_found)?;
    // Not-found and access-denied are deliberately indistinguishable (CRD 369).
    if !has_team_access(&state, &user, &session).await? {
        return Err(session_not_found());
    }
    Ok(envelope::ok(store::session_view(&session)))
}

// ---------------------------------------------------------------- Update session (CRD 371-376)

enum Bind {
    Null,
    S(String),
    I(i64),
}

pub async fn update_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    let id = require_uuid(&raw_id, "session ID")?;
    let body = parse_json(body)?;
    let obj = body.as_object().cloned().unwrap_or_default();
    const UPDATABLE: &[&str] = &[
        "topic", "sessionType", "endTime", "isActive", "priority", "sentiment", "tags", "metadata",
    ];
    if !UPDATABLE.iter().any(|k| obj.contains_key(*k)) {
        return Err(bad("At least one updatable field must be provided"));
    }

    let session = store::find(&state.db, &id).await?.ok_or_else(session_not_found)?;
    if !has_team_access(&state, &user, &session).await? {
        return Err(AppError::Forbidden("You do not have access to this session".into()));
    }

    let mut sets: Vec<(&str, Bind)> = Vec::new();
    if let Some(v) = obj.get("topic") {
        match v {
            Value::Null => sets.push(("topic", Bind::Null)),
            Value::String(s) => {
                let t = sanitize(s);
                if t.chars().count() > 200 {
                    return Err(bad("topic must be at most 200 characters"));
                }
                sets.push(("topic", Bind::S(t)));
            }
            _ => return Err(bad("topic must be a string or null")),
        }
    }
    if let Some(v) = obj.get("sessionType") {
        let t = v.as_str().ok_or_else(|| bad("sessionType must be a string"))?;
        sets.push(("session_type", Bind::S(require_enum(t, SESSION_TYPES, "sessionType")?)));
    }
    if let Some(v) = obj.get("endTime") {
        match v {
            Value::Null => sets.push(("ended_at", Bind::Null)),
            Value::String(s) => {
                sets.push(("ended_at", Bind::S(require_iso_date(s, "endTime")?)))
            }
            _ => return Err(bad("endTime must be an ISO-8601 date or null")),
        }
    }
    if let Some(v) = obj.get("isActive") {
        let b = v.as_bool().ok_or_else(|| bad("isActive must be a boolean"))?;
        sets.push(("is_active", Bind::I(b as i64)));
    }
    if let Some(v) = obj.get("priority") {
        let p = v.as_str().ok_or_else(|| bad("priority must be a string"))?;
        sets.push(("priority", Bind::S(require_enum(p, PRIORITIES, "priority")?)));
    }
    if let Some(v) = obj.get("sentiment") {
        let s = v.as_str().ok_or_else(|| bad("sentiment must be a string"))?;
        sets.push(("sentiment", Bind::S(require_enum(s, SENTIMENTS, "sentiment")?)));
    }
    if let Some(v) = obj.get("tags") {
        sets.push(("tags", Bind::S(json!(validate_tags(v)?).to_string())));
    }
    if let Some(v) = obj.get("metadata") {
        if !v.is_object() {
            return Err(bad("metadata must be an object"));
        }
        sets.push(("metadata", Bind::S(v.to_string())));
    }

    let now = crate::db::now_iso();
    let assignments =
        sets.iter().map(|(col, _)| format!("{col} = ?")).collect::<Vec<_>>().join(", ");
    let sql = format!("UPDATE conversation_sessions SET {assignments}, updated_at = ? WHERE id = ?");
    let mut q = sqlx::query(&sql);
    for (_, b) in &sets {
        q = match b {
            Bind::Null => q.bind(Option::<String>::None),
            Bind::S(s) => q.bind(s.clone()),
            Bind::I(i) => q.bind(*i),
        };
    }
    q.bind(&now).bind(&id).execute(&state.db).await?;

    let updated = store::find(&state.db, &id).await?.ok_or_else(session_not_found)?;
    Ok(envelope::ok_msg(store::session_view(&updated), "Session updated successfully"))
}

// ---------------------------------------------------------------- Delete session (CRD 378-383)

pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = require_uuid(&raw_id, "session ID")?;
    require_admin(&user, "Only administrators can delete sessions")?;
    // Hard delete: the record is permanently removed (CRD 381, 474).
    let affected = sqlx::query("DELETE FROM conversation_sessions WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(session_not_found());
    }
    Ok(envelope::ok_msg(
        json!({ "deleted": true, "sessionId": id }),
        "Session deleted successfully",
    ))
}

// ----------------------------------------------------------- Close & reopen (CRD 385-397)

pub async fn close_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = require_uuid(&raw_id, "session ID")?;
    let session = store::find(&state.db, &id).await?.ok_or_else(session_not_found)?;
    if !has_team_access(&state, &user, &session).await? {
        return Err(AppError::Forbidden("You do not have access to this session".into()));
    }
    let now = crate::db::now_iso();
    let affected = sqlx::query(
        "UPDATE conversation_sessions SET is_active = 0, ended_at = ?, updated_at = ?
         WHERE id = ? AND is_active = 1",
    )
    .bind(&now)
    .bind(&now)
    .bind(&id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(AppError::NotFound("Session not found or already closed".into()));
    }
    Ok(envelope::ok_msg(json!({ "closed": true, "sessionId": id }), "Session closed successfully"))
}

pub async fn reopen_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = require_uuid(&raw_id, "session ID")?;
    let session = store::find(&state.db, &id).await?.ok_or_else(session_not_found)?;
    if !has_team_access(&state, &user, &session).await? {
        return Err(AppError::Forbidden("You do not have access to this session".into()));
    }
    let now = crate::db::now_iso();
    let affected = sqlx::query(
        "UPDATE conversation_sessions
            SET is_active = 1, ended_at = NULL, last_activity_at = ?, updated_at = ?
          WHERE id = ? AND is_active = 0",
    )
    .bind(&now)
    .bind(&now)
    .bind(&id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(AppError::NotFound("Session not found or not reopenable".into()));
    }
    Ok(envelope::ok_msg(
        json!({ "reopened": true, "sessionId": id }),
        "Session reopened successfully",
    ))
}

// --------------------------------------------------------------- Session messages (CRD 399-404)

#[derive(Deserialize)]
pub struct PageQuery {
    pub page: Option<String>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<String>,
}

pub async fn session_messages(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result {
    let id = require_uuid(&raw_id, "session ID")?;
    let page = q.page.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(1).max(1);
    let page_size =
        q.page_size.as_deref().and_then(|v| v.parse::<i64>().ok()).unwrap_or(20).clamp(1, 100);

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE session_id = ? AND deleted_at IS NULL",
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await?;

    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        conversation_id: String,
        session_id: Option<String>,
        sender_type: String,
        customer_id: Option<i64>,
        agent_id: Option<String>,
        content: Option<String>,
        content_type: String,
        session_seq: Option<i64>,
        platform_message_id: Option<String>,
        metadata: Option<String>,
        created_at: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, conversation_id, session_id, sender_type, customer_id, agent_id, content,
                content_type, session_seq, platform_message_id, metadata, created_at
         FROM messages WHERE session_id = ? AND deleted_at IS NULL
         ORDER BY COALESCE(session_seq, 0), created_at, id LIMIT ? OFFSET ?",
    )
    .bind(&id)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(&state.db)
    .await?;

    let messages: Vec<Value> = rows
        .iter()
        .map(|m| {
            let sender_id: Value = match m.sender_type.as_str() {
                "customer" => json!(m.customer_id),
                _ => json!(m.agent_id),
            };
            json!({
                "id": m.id,
                "sessionId": m.session_id,
                "conversationId": m.conversation_id,
                "senderType": m.sender_type,
                "senderId": sender_id,
                "content": m.content,
                "messageType": m.content_type,
                "sessionSeq": m.session_seq,
                "platformMessageId": m.platform_message_id,
                "metadata": m.metadata.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
                "createdAt": m.created_at,
            })
        })
        .collect();

    let total_pages = if total == 0 { 0 } else { (total + page_size - 1) / page_size };
    Ok(envelope::ok(json!({
        "sessionId": id,
        "messages": messages,
        "messageCount": total,
        "pagination": {
            "page": page,
            "pageSize": page_size,
            "total": total,
            "totalPages": total_pages,
            "hasNext": page < total_pages,
            "hasPrev": page > 1 && total_pages > 0,
        },
    })))
}

// ----------------------------------------------------------- Session health check (CRD 406-411)

/// Health thresholds (CRD 409): long-running ~48h, excessive messages ~100,
/// inactivity beyond the configured threshold (default 60 minutes).
const HEALTH_MAX_DURATION_HOURS: i64 = 48;
const HEALTH_MAX_MESSAGES: i64 = 100;
const HEALTH_INACTIVITY_MINUTES: i64 = 60;

pub async fn session_health(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = require_uuid(&raw_id, "session ID")?;
    let s = store::find(&state.db, &id).await?.ok_or_else(session_not_found)?;

    let now = chrono::Utc::now();
    let parse = |raw: &Option<String>| {
        raw.as_deref()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
    };
    let mut issues: Vec<String> = Vec::new();
    let mut suggestions: Vec<String> = Vec::new();
    if s.is_active != 0 {
        if let Some(started) = parse(&s.started_at) {
            if (now - started).num_hours() > HEALTH_MAX_DURATION_HOURS {
                issues.push(format!(
                    "Session has been active for more than {HEALTH_MAX_DURATION_HOURS} hours"
                ));
                suggestions.push("Consider closing this long-running session".into());
            }
        }
        if let Some(last) = parse(&s.last_activity_at) {
            if (now - last).num_minutes() > HEALTH_INACTIVITY_MINUTES {
                issues.push(format!(
                    "Session has been inactive for more than {HEALTH_INACTIVITY_MINUTES} minutes"
                ));
                suggestions.push("Close the session or follow up with the customer".into());
            }
        }
    }
    if s.message_count > HEALTH_MAX_MESSAGES {
        issues.push(format!("Session message count exceeds {HEALTH_MAX_MESSAGES}"));
        suggestions.push("Consider starting a new session segment".into());
    }

    Ok(envelope::ok(json!({
        "healthy": issues.is_empty(),
        "issues": issues,
        "suggestions": suggestions,
    })))
}

// ------------------------------------------------------------- Update session topic (CRD 413-418)

pub async fn update_topic(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    let id = require_uuid(&raw_id, "session ID")?;
    let body = parse_json(body)?;
    let topic = body
        .get("topic")
        .and_then(Value::as_str)
        .map(sanitize)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("topic is required"))?;
    if topic.chars().count() > 200 {
        return Err(bad("topic must be at most 200 characters"));
    }
    let session = store::find(&state.db, &id).await?.ok_or_else(session_not_found)?;
    if !has_team_access(&state, &user, &session).await? {
        return Err(AppError::Forbidden("You do not have access to this session".into()));
    }
    sqlx::query("UPDATE conversation_sessions SET topic = ?, updated_at = ? WHERE id = ?")
        .bind(&topic)
        .bind(crate::db::now_iso())
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(envelope::message_only("Session topic updated successfully"))
}

// ------------------------------------------------------------------- Statistics (CRD 420-431)

#[derive(Deserialize)]
pub struct StatsQuery {
    pub conversation_id: Option<String>,
}

pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<StatsQuery>,
) -> Result {
    require_admin(&user, "Administrator access required")?;
    let cid = match q.conversation_id.as_deref() {
        Some(c) => Some(require_uuid(c, "conversation_id")?),
        None => None,
    };
    let stats = store::statistics(&state.db, cid.as_deref()).await?;
    Ok(envelope::ok(stats))
}

pub async fn stats_for_conversation(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    require_admin(&user, "Administrator access required")?;
    let cid = require_uuid(&raw_id, "conversation_id")?;
    let mut stats = store::statistics(&state.db, Some(&cid)).await?;
    stats["conversationId"] = json!(cid);
    Ok(envelope::ok(stats))
}

// ------------------------------------------------------------ Activity statistics (CRD 433-438)

#[derive(Deserialize)]
pub struct ActivityQuery {
    pub conversation_id: Option<String>,
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
}

pub async fn activity_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ActivityQuery>,
) -> Result {
    require_admin(&user, "Administrator access required")?;
    let cid = match q.conversation_id.as_deref() {
        Some(c) => Some(require_uuid(c, "conversation_id")?),
        None => None,
    };
    let range = q.time_range.as_deref().unwrap_or("week");
    let (days, bucket_len) = match range {
        "day" => (1, 13),    // YYYY-MM-DDTHH
        "week" => (7, 10),   // YYYY-MM-DD
        "month" => (30, 10), // YYYY-MM-DD
        "year" => (365, 7),  // YYYY-MM
        _ => return Err(bad("timeRange must be one of: day, week, month, year")),
    };
    let since = (chrono::Utc::now() - chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    #[derive(Default, Clone)]
    struct Bucket {
        created: i64,
        ended: i64,
        messages: i64,
        active_minutes: f64,
    }
    let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
    let conv_clause = if cid.is_some() { " AND conversation_id = ?" } else { "" };

    let created_sql = format!(
        "SELECT substr(created_at, 1, {bucket_len}), COUNT(*),
                COALESCE(SUM((julianday(COALESCE(ended_at, last_activity_at, created_at))
                              - julianday(COALESCE(started_at, created_at))) * 1440.0), 0)
         FROM conversation_sessions WHERE created_at >= ?{conv_clause} GROUP BY 1"
    );
    let mut query = sqlx::query_as::<_, (String, i64, f64)>(&created_sql).bind(&since);
    if let Some(c) = &cid {
        query = query.bind(c.clone());
    }
    for (k, n, mins) in query.fetch_all(&state.db).await? {
        let b = buckets.entry(k).or_default();
        b.created = n;
        b.active_minutes = (mins * 100.0).round() / 100.0;
    }

    let ended_sql = format!(
        "SELECT substr(ended_at, 1, {bucket_len}), COUNT(*) FROM conversation_sessions
         WHERE ended_at IS NOT NULL AND ended_at >= ?{conv_clause} GROUP BY 1"
    );
    let mut query = sqlx::query_as::<_, (String, i64)>(&ended_sql).bind(&since);
    if let Some(c) = &cid {
        query = query.bind(c.clone());
    }
    for (k, n) in query.fetch_all(&state.db).await? {
        buckets.entry(k).or_default().ended = n;
    }

    let messages_sql = format!(
        "SELECT substr(created_at, 1, {bucket_len}), COUNT(*) FROM messages
         WHERE session_id IS NOT NULL AND deleted_at IS NULL AND created_at >= ?{conv_clause}
         GROUP BY 1"
    );
    let mut query = sqlx::query_as::<_, (String, i64)>(&messages_sql).bind(&since);
    if let Some(c) = &cid {
        query = query.bind(c.clone());
    }
    for (k, n) in query.fetch_all(&state.db).await? {
        buckets.entry(k).or_default().messages = n;
    }

    // Hour-of-day activity for peak / least computation.
    let hours_sql = format!(
        "SELECT substr(created_at, 12, 2), COUNT(*) FROM conversation_sessions
         WHERE created_at >= ?{conv_clause} GROUP BY 1"
    );
    let mut query = sqlx::query_as::<_, (String, i64)>(&hours_sql).bind(&since);
    if let Some(c) = &cid {
        query = query.bind(c.clone());
    }
    let hours = query.fetch_all(&state.db).await?;
    let peak = hours.iter().max_by_key(|(_, n)| *n).map(|(h, _)| h.clone());
    let least = hours.iter().min_by_key(|(_, n)| *n).map(|(h, _)| h.clone());

    let total_created: i64 = buckets.values().map(|b| b.created).sum();
    let total_ended: i64 = buckets.values().map(|b| b.ended).sum();
    let total_messages: i64 = buckets.values().map(|b| b.messages).sum();
    let bucket_count = buckets.len().max(1) as f64;
    let items: Vec<Value> = buckets
        .iter()
        .map(|(k, b)| {
            json!({
                "bucket": k,
                "sessionsCreated": b.created,
                "sessionsEnded": b.ended,
                "messagesSent": b.messages,
                "activeMinutes": b.active_minutes,
            })
        })
        .collect();

    Ok(envelope::ok(json!({
        "timeRange": range,
        "buckets": items,
        "summary": {
            "totalSessionsCreated": total_created,
            "totalSessionsEnded": total_ended,
            "totalMessages": total_messages,
            "avgSessionsPerBucket": ((total_created as f64 / bucket_count) * 100.0).round() / 100.0,
            "peakActivityHour": peak,
            "leastActivityHour": least,
        },
    })))
}

// --------------------------------------------------------------- Batch operation (CRD 440-445)

const BATCH_ACTIONS: &[&str] =
    &["close", "reopen", "update_priority", "add_tags", "remove_tags", "delete"];

pub async fn batch(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user, "Administrator access required")?;
    let body = parse_json(body)?;
    let ids = body
        .get("sessionIds")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| bad("sessionIds must be a non-empty array"))?;
    if ids.len() > 100 {
        return Err(bad("sessionIds can contain at most 100 items"));
    }
    let mut session_ids: Vec<String> = Vec::with_capacity(ids.len());
    for v in ids {
        let raw = v.as_str().ok_or_else(|| bad("Invalid session ID format: must be a UUID"))?;
        session_ids.push(require_uuid(raw, "session ID")?);
    }
    let action = body.get("action").and_then(Value::as_str).unwrap_or("");
    require_enum(action, BATCH_ACTIONS, "action")?;
    let data = body.get("data").cloned().unwrap_or(Value::Null);

    let priority = if action == "update_priority" {
        let p = data
            .get("priority")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("data.priority is required for update_priority"))?;
        Some(require_enum(p, PRIORITIES, "priority")?)
    } else {
        None
    };
    let tags = if action == "add_tags" || action == "remove_tags" {
        let v = data.get("tags").ok_or_else(|| bad(format!("data.tags is required for {action}")))?;
        Some(validate_tags(v)?)
    } else {
        None
    };
    let end_time = match data.get("endTime").and_then(Value::as_str) {
        Some(t) => Some(require_iso_date(t, "endTime")?),
        None => None,
    };

    let now = crate::db::now_iso();
    let mut results: Vec<Value> = Vec::with_capacity(session_ids.len());
    let mut succeeded = 0usize;
    for sid in &session_ids {
        let outcome: std::result::Result<bool, AppError> = match action {
            "close" => {
                let ended = end_time.clone().unwrap_or_else(|| now.clone());
                Ok(sqlx::query(
                    "UPDATE conversation_sessions SET is_active = 0, ended_at = ?, updated_at = ?
                     WHERE id = ? AND is_active = 1",
                )
                .bind(&ended)
                .bind(&now)
                .bind(sid)
                .execute(&state.db)
                .await?
                .rows_affected()
                    > 0)
            }
            "reopen" => Ok(sqlx::query(
                "UPDATE conversation_sessions
                    SET is_active = 1, ended_at = NULL, last_activity_at = ?, updated_at = ?
                  WHERE id = ? AND is_active = 0",
            )
            .bind(&now)
            .bind(&now)
            .bind(sid)
            .execute(&state.db)
            .await?
            .rows_affected()
                > 0),
            "update_priority" => Ok(sqlx::query(
                "UPDATE conversation_sessions SET priority = ?, updated_at = ? WHERE id = ?",
            )
            .bind(priority.as_deref())
            .bind(&now)
            .bind(sid)
            .execute(&state.db)
            .await?
            .rows_affected()
                > 0),
            "add_tags" | "remove_tags" => {
                match store::find(&state.db, sid).await? {
                    None => Ok(false),
                    Some(s) => {
                        let mut current: Vec<String> = s
                            .tags
                            .as_deref()
                            .and_then(|t| serde_json::from_str(t).ok())
                            .unwrap_or_default();
                        let change = tags.clone().unwrap_or_default();
                        if action == "add_tags" {
                            for t in change {
                                if !current.contains(&t) {
                                    current.push(t);
                                }
                            }
                        } else {
                            current.retain(|t| !change.contains(t));
                        }
                        sqlx::query(
                            "UPDATE conversation_sessions SET tags = ?, updated_at = ? WHERE id = ?",
                        )
                        .bind(json!(current).to_string())
                        .bind(&now)
                        .bind(sid)
                        .execute(&state.db)
                        .await?;
                        Ok(true)
                    }
                }
            }
            "delete" => Ok(sqlx::query("DELETE FROM conversation_sessions WHERE id = ?")
                .bind(sid)
                .execute(&state.db)
                .await?
                .rows_affected()
                > 0),
            _ => unreachable!("validated above"),
        };
        match outcome {
            Ok(true) => {
                succeeded += 1;
                results.push(json!({ "sessionId": sid, "success": true }));
            }
            Ok(false) => results.push(json!({
                "sessionId": sid,
                "success": false,
                "error": "Session not found or not applicable",
            })),
            Err(e) => results.push(json!({
                "sessionId": sid,
                "success": false,
                "error": e.to_string(),
            })),
        }
    }

    Ok(envelope::ok_msg(
        json!({
            "total": session_ids.len(),
            "succeeded": succeeded,
            "failed": session_ids.len() - succeeded,
            "results": results,
        }),
        &format!("Batch {action} completed"),
    ))
}

// ------------------------------------------------ Get-or-create (boundary-aware, CRD 447-452)

pub async fn get_or_create(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    let body = parse_json(body)?;
    let conversation_id = body.get("conversation_id").and_then(Value::as_str);
    let message_content = body.get("messageContent").and_then(Value::as_str);
    let sender_type = body.get("senderType").and_then(Value::as_str);
    let (Some(conversation_id), Some(message_content), Some(sender_type)) =
        (conversation_id, message_content, sender_type)
    else {
        return Err(bad("conversation_id, messageContent and senderType are required"));
    };
    let conversation_id = require_uuid(conversation_id, "conversation_id")?;
    require_enum(sender_type, SENDER_TYPES, "senderType")?;

    let current = store::latest_active(&state.db, &conversation_id).await?;
    let detection =
        topics::detect_boundary(current.as_ref(), message_content, sender_type, chrono::Utc::now());

    if detection.should_create_new {
        let now = crate::db::now_iso();
        // The prior active segment (if any) is closed before the new one starts
        // (CRD 450, 478).
        if let Some(prior) = &current {
            sqlx::query(
                "UPDATE conversation_sessions SET is_active = 0, ended_at = ?, updated_at = ?
                 WHERE id = ? AND is_active = 1",
            )
            .bind(&now)
            .bind(&now)
            .bind(&prior.id)
            .execute(&state.db)
            .await?;
        }
        let session = store::create(
            &state.db,
            NewSession {
                conversation_id: &conversation_id,
                session_type: "continuous",
                topic: detection.suggested_topic.clone(),
                priority: None,
                tags: None,
                metadata: None,
            },
        )
        .await?;
        return Ok(envelope::ok(store::session_view(&session)));
    }

    let existing = current.expect("continue decision implies an active session");
    let now = crate::db::now_iso();
    sqlx::query(
        "UPDATE conversation_sessions SET last_activity_at = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&now)
    .bind(&now)
    .bind(&existing.id)
    .execute(&state.db)
    .await?;
    let refreshed = store::find(&state.db, &existing.id).await?.ok_or_else(session_not_found)?;
    Ok(envelope::ok(store::session_view(&refreshed)))
}

// ----------------------------------------------------- Detect boundary (analysis, CRD 454-459)

pub async fn detect_boundary(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    let body = parse_json(body)?;
    let message_content = body
        .get("messageContent")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("messageContent is required"))?;
    let sender_type = body
        .get("senderType")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("senderType is required"))?;
    let current = match body.get("currentSessionId").and_then(Value::as_str) {
        Some(raw) => {
            let id = require_uuid(raw, "currentSessionId")?;
            store::find(&state.db, &id).await?
        }
        None => None,
    };
    let detection =
        topics::detect_boundary(current.as_ref(), message_content, sender_type, chrono::Utc::now());
    Ok(envelope::ok(detection.to_json()))
}

// ----------------------------------------------------------------- Topic family (CRD 461-468)

pub async fn topic_stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<StatsQuery>,
) -> Result {
    let cid = match q.conversation_id.as_deref() {
        Some(c) => Some(require_uuid(c, "conversation_id")?),
        None => None,
    };
    let stats = store::statistics(&state.db, cid.as_deref()).await?;
    Ok(envelope::ok(json!({
        "total": stats["total"],
        "topics": stats["topicDistribution"],
    })))
}

pub async fn analyze_topic(
    Extension(_user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    let body = parse_json(body)?;
    let content = body
        .get("messageContent")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("messageContent is required"))?;
    let result = topics::derive_topic(content);
    Ok(envelope::ok(json!({
        "topic": result.topic,
        "confidence": result.confidence,
        "source": result.source,
    })))
}

pub async fn suggest_topics(
    Extension(_user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    let body = parse_json(body)?;
    let content = body
        .get("messageContent")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("messageContent is required"))?;
    let limit = match body.get("limit") {
        None => 3,
        Some(v) => v
            .as_i64()
            .filter(|n| (1..=10).contains(n))
            .ok_or_else(|| bad("limit must be between 1 and 10"))? as usize,
    };
    let suggestions = topics::suggest_topics(content, limit);
    let count = suggestions.len();
    Ok(ok_count(json!(suggestions), count, None))
}

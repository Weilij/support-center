//! Agents/Operators handlers (CRD §3.3, lines 2154-2321).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::domain::auth::store::find_active_agent_by_email;
use crate::domain::teams::store as teams_store;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::store::{
    self, OperatorRow, PRESENCE_STATES, SKILL_CATEGORIES, SKILL_LEVELS,
};

type Result<T = Response> = std::result::Result<T, AppError>;
type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b).map_err(|_| AppError::BadRequest("Invalid JSON".into()))
}

// ----------------------------------------------------------------------------- helpers

/// Team-leader capability is granted to the distinct "team" role value (CRD 2303, 2313).
fn is_team_leader(user: &AuthUser) -> bool {
    user.role == "team"
}

fn require_privileged(user: &AuthUser) -> Result<()> {
    if user.is_admin() || is_team_leader(user) {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator or team leader role required".into()))
    }
}

fn require_admin(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator role required".into()))
    }
}

/// Operator identifiers are treated as 10–50 characters for routing (CRD 2208, 2303).
fn validate_agent_id(id: &str) -> Result<()> {
    let len = id.chars().count();
    if (10..=50).contains(&len) {
        Ok(())
    } else {
        Err(AppError::BadRequest("Agent ID must be between 10 and 50 characters".into()))
    }
}

/// Access scoping (CRD 2209): administrators and team leaders may target any operator;
/// ordinary operators only themselves.
fn require_scope(user: &AuthUser, agent_id: &str) -> Result<()> {
    if user.is_admin() || is_team_leader(user) || user.id == agent_id {
        Ok(())
    } else {
        Err(AppError::Forbidden("You may only access your own agent record".into()))
    }
}

/// Privilege levels for the role-elevation guard (CRD 2284, 2313).
fn role_level(role: &str) -> u8 {
    match role {
        "admin" => 3,
        "team" => 2,
        _ => 1,
    }
}

fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else { return false };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

/// Validate `agentIds`: non-empty array, ≤50 entries, each 10–50 chars (CRD 2173, 2182).
fn validate_agent_ids(v: Option<&Value>) -> Result<Vec<String>> {
    let arr = v
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest("agentIds must be a non-empty array".into()))?;
    if arr.len() > 50 {
        return Err(AppError::BadRequest("agentIds cannot contain more than 50 entries".into()));
    }
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        let s = e.as_str().unwrap_or("");
        let len = s.chars().count();
        if !(10..=50).contains(&len) {
            return Err(AppError::BadRequest(
                "Each agent ID must be between 10 and 50 characters".into(),
            ));
        }
        out.push(s.to_string());
    }
    Ok(out)
}

// -------------------------------------------------------- List operators (CRD 2163-2169)

#[derive(Deserialize)]
pub struct ListAgentsQuery {
    pub page: Option<String>,
    pub limit: Option<String>,
    #[serde(rename = "includeInactive")]
    pub include_inactive: Option<String>,
    pub search: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
}

pub async fn list_agents(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListAgentsQuery>,
) -> Result {
    require_privileged(&user)?;
    // Invalid pagination is rejected, not clamped (CRD 2169).
    let page = match &q.page {
        None => 1,
        Some(raw) => raw
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|p| (1..=1000).contains(p))
            .ok_or_else(|| AppError::BadRequest("page must be an integer between 1 and 1000".into()))?,
    };
    let limit = match &q.limit {
        None => 20,
        Some(raw) => raw
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|l| (1..=100).contains(l))
            .ok_or_else(|| AppError::BadRequest("limit must be an integer between 1 and 100".into()))?,
    };

    let mut filter = String::new();
    let mut binds: Vec<String> = Vec::new();
    // Only the literal value "true" includes deactivated operators (CRD 2165).
    if q.include_inactive.as_deref() != Some("true") {
        filter.push_str(" AND a.is_active = 1");
    }
    if let Some(search) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        filter.push_str(" AND (LOWER(a.display_name) LIKE ? OR LOWER(a.email) LIKE ?)");
        let p = format!("%{}%", search.to_lowercase());
        binds.push(p.clone());
        binds.push(p);
    }
    if let Some(team_id) = q.team_id.as_deref().and_then(|t| t.trim().parse::<i64>().ok()) {
        filter.push_str(
            " AND EXISTS (SELECT 1 FROM team_members f WHERE f.agent_id = a.id AND f.team_id = $1::bigint)",
        );
        binds.push(team_id.to_string());
    }
    if let Some(role) = q.role.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        filter.push_str(" AND a.role = ?");
        binds.push(role.to_string());
    }
    // `status` is accepted/validated but does not filter results here (CRD 2167);
    // values outside the presence-state set are ignored.
    let _ = q.status.as_deref().filter(|s| PRESENCE_STATES.contains(s));

    let count_sql =
        format!("SELECT COUNT(*) FROM agents a WHERE a.deleted_at IS NULL {filter}");
    let count_sql = crate::db::pg_params(&count_sql);
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        count_q = count_q.bind(b.clone());
    }
    let total = count_q.fetch_one(&state.db).await?;

    let list_sql = format!(
        "{} {filter} ORDER BY a.created_at DESC, a.id DESC LIMIT ? OFFSET ?",
        store::OPERATOR_SELECT
    );
    let list_sql = crate::db::pg_params(&list_sql);
    let mut list_q = sqlx::query_as::<_, OperatorRow>(&list_sql);
    for b in &binds {
        list_q = list_q.bind(b.clone());
    }
    let rows = list_q.bind(limit).bind((page - 1) * limit).fetch_all(&state.db).await?;
    let items: Vec<Value> = rows.iter().map(store::operator_view).collect();
    Ok(envelope::ok_with_pagination(items, page, limit, total))
}

// --------------------------------------------------------- Profile updates (CRD 2281-2288)

struct ProfileUpdates {
    display_name: Option<String>,
    email: Option<String>,
    role: Option<String>,
    team_id: Option<i64>,
    is_active: Option<bool>,
    password_policy: Option<String>,
    position: Option<String>,
}

/// Body validation shared by single update and bulk update (CRD 2283, 2287).
fn validate_profile_body(body: &Value) -> Result<ProfileUpdates> {
    let obj = body
        .as_object()
        .ok_or_else(|| AppError::BadRequest("Request body must be a JSON object".into()))?;

    let display_name = match obj.get("displayName") {
        None => None,
        Some(v) => {
            let s = v.as_str().unwrap_or("").trim().to_string();
            let len = s.chars().count();
            if !(2..=50).contains(&len) {
                return Err(AppError::BadRequest(
                    "displayName must be between 2 and 50 characters".into(),
                ));
            }
            Some(s)
        }
    };
    let email = match obj.get("email") {
        None => None,
        Some(v) => {
            let s = v.as_str().unwrap_or("").trim().to_lowercase();
            if !valid_email(&s) {
                return Err(AppError::BadRequest("Invalid email format".into()));
            }
            Some(s)
        }
    };
    let role = match obj.get("role") {
        None => None,
        Some(v) => {
            let s = v.as_str().unwrap_or("");
            if s != "admin" && s != "agent" {
                return Err(AppError::BadRequest("role must be one of: admin, agent".into()));
            }
            Some(s.to_string())
        }
    };
    let team_id = match obj.get("teamId") {
        None => None,
        Some(v) => {
            let id = match v {
                Value::Number(n) => n.as_i64(),
                Value::String(s) => s.trim().parse::<i64>().ok(),
                _ => None,
            }
            .filter(|t| *t > 0)
            .ok_or_else(|| AppError::BadRequest("Invalid team ID format".into()))?;
            Some(id)
        }
    };
    let is_active = match obj.get("isActive") {
        None => None,
        Some(v) => Some(
            v.as_bool()
                .ok_or_else(|| AppError::BadRequest("isActive must be a boolean".into()))?,
        ),
    };
    let password_policy = match obj.get("passwordPolicy") {
        None => None,
        Some(v) => {
            let s = v.as_str().unwrap_or("");
            if !["changeable", "unchangeable", "must_change"].contains(&s) {
                return Err(AppError::BadRequest(
                    "passwordPolicy must be one of: changeable, unchangeable, must_change".into(),
                ));
            }
            Some(s.to_string())
        }
    };
    let position = match obj.get("position") {
        None => None,
        Some(v) => {
            let s = v.as_str().unwrap_or("");
            if !["system_admin", "supervisor", "agent"].contains(&s) {
                return Err(AppError::BadRequest(
                    "position must be one of: system_admin, supervisor, agent".into(),
                ));
            }
            Some(s.to_string())
        }
    };
    let updates =
        ProfileUpdates { display_name, email, role, team_id, is_active, password_policy, position };
    if updates.display_name.is_none()
        && updates.email.is_none()
        && updates.role.is_none()
        && updates.team_id.is_none()
        && updates.is_active.is_none()
        && updates.password_policy.is_none()
        && updates.position.is_none()
    {
        return Err(AppError::BadRequest("No fields to update".into()));
    }
    Ok(updates)
}

/// Applies validated updates. Email collision and missing target team surface as
/// internal errors per spec (CRD 2287).
async fn apply_profile_updates(
    state: &AppState,
    operator: &OperatorRow,
    updates: &ProfileUpdates,
) -> Result<()> {
    let now = now_iso();
    if let Some(email) = &updates.email {
        if *email != operator.email
            && find_active_agent_by_email(&state.db, email).await?.is_some()
        {
            return Err(AppError::Internal("Email already in use by another agent".into()));
        }
        sqlx::query("UPDATE agents SET email = $1, updated_at = $2 WHERE id = $3")
            .bind(email)
            .bind(&now)
            .bind(&operator.id)
            .execute(&state.db)
            .await?;
    }
    if let Some(name) = &updates.display_name {
        sqlx::query("UPDATE agents SET display_name = $1, updated_at = $2 WHERE id = $3")
            .bind(name)
            .bind(&now)
            .bind(&operator.id)
            .execute(&state.db)
            .await?;
    }
    if let Some(role) = &updates.role {
        sqlx::query("UPDATE agents SET role = $1, updated_at = $2 WHERE id = $3")
            .bind(role)
            .bind(&now)
            .bind(&operator.id)
            .execute(&state.db)
            .await?;
    }
    if let Some(active) = updates.is_active {
        sqlx::query("UPDATE agents SET is_active = $1, updated_at = $2 WHERE id = $3")
            .bind(active as i64)
            .bind(&now)
            .bind(&operator.id)
            .execute(&state.db)
            .await?;
    }
    if let Some(policy) = &updates.password_policy {
        sqlx::query("UPDATE agents SET password_policy = $1, updated_at = $2 WHERE id = $3")
            .bind(policy)
            .bind(&now)
            .bind(&operator.id)
            .execute(&state.db)
            .await?;
    }
    if let Some(team_id) = updates.team_id {
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT id FROM teams WHERE id = $1 AND deleted_at IS NULL")
                .bind(team_id)
                .fetch_optional(&state.db)
                .await?;
        if exists.is_none() {
            return Err(AppError::Internal("Team not found".into()));
        }
        // Prior memberships are replaced with a single primary membership (CRD 2285).
        teams_store::replace_memberships(&state.db, &operator.id, team_id).await?;
        state.team_cache.invalidate(&operator.id);
    }
    if let Some(position) = &updates.position {
        sqlx::query("UPDATE agents SET position = $1, updated_at = $2 WHERE id = $3")
            .bind(position)
            .bind(&now)
            .bind(&operator.id)
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

pub async fn update_agent(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let body = parse_json(body)?;
    let updates = validate_profile_body(&body)?;

    // A caller cannot assign a role above their own privilege (CRD 2284).
    if let Some(role) = &updates.role {
        if role_level(role) > role_level(&user.role) {
            return Err(AppError::Forbidden(
                "You cannot assign a role higher than your own".into(),
            ));
        }
    }
    // Self team transfer is blocked for non-administrators (CRD 2284).
    if updates.team_id.is_some() && !user.is_admin() && agent_id == user.id {
        return Err(AppError::Forbidden(
            "Team changes on your own record require an administrator".into(),
        ));
    }

    let operator = store::find_operator(&state.db, &agent_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Agent not found".into()))?;
    apply_profile_updates(&state, &operator, &updates).await?;

    // TODO(realtime): emit operator-updated domain event (CRD 2319).
    let updated = store::find_operator(&state.db, &agent_id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload agent after update".into()))?;
    Ok(envelope::ok_msg(store::operator_view(&updated), "Agent updated successfully"))
}

// --------------------------------------------------------- Bulk update (CRD 2171-2178)

pub async fn batch_update(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let agent_ids = validate_agent_ids(body.get("agentIds"))?;
    let updates = validate_profile_body(body.get("updates").unwrap_or(&Value::Null))?;

    // Best-effort: individually failing operators are silently omitted (CRD 2178).
    let mut updated: Vec<Value> = Vec::new();
    for agent_id in &agent_ids {
        let Ok(Some(operator)) = store::find_operator(&state.db, agent_id).await else {
            continue;
        };
        if apply_profile_updates(&state, &operator, &updates).await.is_err() {
            continue;
        }
        if let Ok(Some(fresh)) = store::find_operator(&state.db, agent_id).await {
            updated.push(store::operator_view(&fresh));
        }
    }
    let message = format!("{} agents updated", updated.len());
    Ok(envelope::ok_msg(updated, &message))
}

// -------------------------------------------------------- Bulk transfer (CRD 2180-2187)

pub async fn batch_transfer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let agent_ids = validate_agent_ids(body.get("agentIds"))?;
    let to_team_id = body
        .get("toTeamId")
        .and_then(|v| match v {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.trim().parse::<i64>().ok(),
            _ => None,
        })
        .ok_or_else(|| AppError::BadRequest("toTeamId is required".into()))?;

    // A missing target team surfaces as a server error (CRD 2186).
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT id FROM teams WHERE id = $1 AND deleted_at IS NULL")
            .bind(to_team_id)
            .fetch_optional(&state.db)
            .await?;
    if exists.is_none() {
        return Err(AppError::Internal("Target team not found".into()));
    }

    let mut errors: Vec<Value> = Vec::new();
    for agent_id in &agent_ids {
        match store::find_operator(&state.db, agent_id).await? {
            None => errors.push(json!({ "agentId": agent_id, "error": "Agent not found" })),
            Some(_) => {
                // Existing memberships replaced with a single primary membership (CRD 2184).
                teams_store::replace_memberships(&state.db, agent_id, to_team_id).await?;
                state.team_cache.invalidate(agent_id);
            }
        }
    }
    let success = errors.is_empty();
    let message = if success {
        "All agents transferred successfully".to_string()
    } else {
        format!("Transfer completed with {} error(s)", errors.len())
    };
    Ok(envelope::flagged(
        success,
        json!({ "success": success, "errors": errors }),
        Some(&message),
    ))
}

// ------------------------------------------------------ Search operators (CRD 2189-2195)

pub async fn search_agents(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    require_privileged(&user)?;
    let body = parse_json(body)?;

    let mut filter = String::new();
    let mut binds: Vec<String> = Vec::new();
    if let Some(keyword) = body.get("keyword").and_then(Value::as_str).map(str::trim) {
        if !keyword.is_empty() {
            filter.push_str(" AND (LOWER(a.display_name) LIKE ? OR LOWER(a.email) LIKE ?)");
            let p = format!("%{}%", keyword.to_lowercase());
            binds.push(p.clone());
            binds.push(p);
        }
    }
    if let Some(team_ids) = body.get("teamIds").and_then(Value::as_array) {
        let ids: Vec<i64> = team_ids.iter().filter_map(Value::as_i64).collect();
        if !ids.is_empty() {
            let placeholders = vec!["?::bigint"; ids.len()].join(", ");
            filter.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM team_members f WHERE f.agent_id = a.id
                              AND f.team_id IN ({placeholders}))"
            ));
            binds.extend(ids.iter().map(|i| i.to_string()));
        }
    }
    if let Some(roles) = body.get("roles").and_then(Value::as_array) {
        let roles: Vec<&str> = roles.iter().filter_map(Value::as_str).collect();
        if !roles.is_empty() {
            let placeholders = vec!["?"; roles.len()].join(", ");
            filter.push_str(&format!(" AND a.role IN ({placeholders})"));
            binds.extend(roles.iter().map(|r| r.to_string()));
        }
    }
    if let Some(active) = body.get("isActive").and_then(Value::as_bool) {
        filter.push_str(" AND a.is_active = ?::bigint");
        binds.push((active as i64).to_string());
    }
    if let Some(after) = body.get("lastActiveAfter").and_then(Value::as_str) {
        filter.push_str(" AND a.last_active_at >= ?");
        binds.push(after.to_string());
    }
    if let Some(before) = body.get("lastActiveBefore").and_then(Value::as_str) {
        filter.push_str(" AND a.last_active_at <= ?");
        binds.push(before.to_string());
    }
    let limit = body.get("limit").and_then(Value::as_i64).unwrap_or(50).max(1);
    let offset = body.get("offset").and_then(Value::as_i64).unwrap_or(0).max(0);

    // Most recently active first (CRD 2193).
    let sql = format!(
        "{} {filter} ORDER BY a.last_active_at DESC, a.created_at DESC LIMIT ? OFFSET ?",
        store::OPERATOR_SELECT
    );
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as::<_, OperatorRow>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    let rows = q.bind(limit).bind(offset).fetch_all(&state.db).await?;
    Ok(envelope::ok(rows.iter().map(store::operator_view).collect::<Vec<_>>()))
}

// ----------------------------------------------------- Status statistics (CRD 2197-2204)

pub async fn status_statistics(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_privileged(&user)?;
    // Reading statuses passively triggers auto-expiry (CRD 2204).
    let expired: Vec<String> = sqlx::query_scalar(
        "SELECT agent_id FROM agent_status WHERE available_until IS NOT NULL AND available_until <= $1",
    )
    .bind(now_iso())
    .fetch_all(&state.db)
    .await?;
    for agent_id in &expired {
        store::set_status(&state.db, agent_id, "offline", None, Some("auto-expired")).await?;
    }

    // Operators with no recorded presence count as offline (CRD 2201).
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(s.status, 'offline') AS st, COUNT(*)
         FROM agents a
         LEFT JOIN agent_status s ON s.agent_id = a.id
         WHERE a.deleted_at IS NULL AND a.is_active = 1
         GROUP BY st",
    )
    .fetch_all(&state.db)
    .await?;
    let mut stats = serde_json::Map::new();
    for s in PRESENCE_STATES {
        stats.insert(s.to_string(), json!(0));
    }
    for (status, count) in rows {
        if PRESENCE_STATES.contains(&status.as_str()) {
            stats.insert(status, json!(count));
        }
    }
    Ok(envelope::ok(Value::Object(stats)))
}

// --------------------------------------------------------------- Skills (CRD 2206-2245)

pub async fn get_skills(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let skills = store::skills_of(&state.db, &agent_id).await?;
    Ok(envelope::ok(skills.iter().map(store::skill_view).collect::<Vec<_>>()))
}

pub async fn add_skill(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let body = parse_json(body)?;

    let name = body.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let name_len = name.chars().count();
    if !(2..=100).contains(&name_len) {
        return Err(AppError::BadRequest("name must be between 2 and 100 characters".into()));
    }
    let category = body.get("category").and_then(Value::as_str).unwrap_or("");
    if !SKILL_CATEGORIES.contains(&category) {
        return Err(AppError::BadRequest(format!(
            "category must be one of: {}",
            SKILL_CATEGORIES.join(", ")
        )));
    }
    let level = body.get("level").and_then(Value::as_str).unwrap_or("");
    if !SKILL_LEVELS.contains(&level) {
        return Err(AppError::BadRequest(format!(
            "level must be one of: {}",
            SKILL_LEVELS.join(", ")
        )));
    }
    let description = match body.get("description") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let s = v.as_str().unwrap_or("").to_string();
            if s.chars().count() > 500 {
                return Err(AppError::BadRequest(
                    "description cannot exceed 500 characters".into(),
                ));
            }
            Some(s)
        }
    };
    let certified = match body.get("certified") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(AppError::BadRequest("certified must be a boolean".into())),
    };

    // Skill names are unique per operator; duplicates surface as a server error (CRD 2220).
    let dup: Option<String> =
        sqlx::query_scalar("SELECT id FROM agent_skills WHERE agent_id = $1 AND name = $2")
            .bind(&agent_id)
            .bind(&name)
            .fetch_optional(&state.db)
            .await?;
    if dup.is_some() {
        return Err(AppError::Internal("A skill with this name already exists".into()));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let certified_at = certified.then(|| now.clone());
    sqlx::query(
        "INSERT INTO agent_skills (id, agent_id, name, category, level, description,
                                   certified, certified_at, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&id)
    .bind(&agent_id)
    .bind(&name)
    .bind(category)
    .bind(level)
    .bind(&description)
    .bind(certified as i64)
    .bind(&certified_at)
    .bind(&now)
    .execute(&state.db)
    .await?;

    // TODO(realtime): emit skill-added domain event (CRD 2319).
    let skill = store::find_skill(&state.db, &agent_id, &id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload skill after create".into()))?;
    Ok(envelope::with_status(
        StatusCode::CREATED,
        Some(store::skill_view(&skill)),
        Some("Skill added successfully"),
    ))
}

pub async fn update_skill(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((agent_id, skill_id)): Path<(String, String)>,
    body: JsonBody<Value>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let body = parse_json(body)?;
    // A missing skill surfaces as a server error (CRD 2229).
    store::find_skill(&state.db, &agent_id, &skill_id)
        .await?
        .ok_or_else(|| AppError::Internal("Skill not found".into()))?;

    let now = now_iso();
    if let Some(level) = body.get("level").and_then(Value::as_str) {
        if !SKILL_LEVELS.contains(&level) {
            return Err(AppError::BadRequest(format!(
                "level must be one of: {}",
                SKILL_LEVELS.join(", ")
            )));
        }
        sqlx::query("UPDATE agent_skills SET level = $1, updated_at = $2 WHERE id = $3")
            .bind(level)
            .bind(&now)
            .bind(&skill_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(description) = body.get("description") {
        let text = description.as_str().map(String::from);
        if text.as_deref().map(|s| s.chars().count()).unwrap_or(0) > 500 {
            return Err(AppError::BadRequest("description cannot exceed 500 characters".into()));
        }
        sqlx::query("UPDATE agent_skills SET description = $1, updated_at = $2 WHERE id = $3")
            .bind(text)
            .bind(&now)
            .bind(&skill_id)
            .execute(&state.db)
            .await?;
    }
    match body.get("certified") {
        // Absent: the prior certification timestamp is preserved (CRD 2227).
        None => {}
        Some(Value::Bool(c)) => {
            let certified_at = c.then(|| now.clone());
            sqlx::query(
                "UPDATE agent_skills SET certified = $1, certified_at = $2, updated_at = $3 WHERE id = $4",
            )
            .bind(*c as i64)
            .bind(&certified_at)
            .bind(&now)
            .bind(&skill_id)
            .execute(&state.db)
            .await?;
        }
        Some(_) => return Err(AppError::BadRequest("certified must be a boolean".into())),
    }

    // TODO(realtime): emit skill-updated domain event (CRD 2319).
    let updated = store::find_skill(&state.db, &agent_id, &skill_id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload skill after update".into()))?;
    Ok(envelope::ok_msg(store::skill_view(&updated), "Skill updated successfully"))
}

pub async fn delete_skill(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((agent_id, skill_id)): Path<(String, String)>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let res = sqlx::query("DELETE FROM agent_skills WHERE agent_id = $1 AND id = $2")
        .bind(&agent_id)
        .bind(&skill_id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("Skill not found".into()));
    }
    Ok(envelope::message_only("Skill deleted successfully"))
}

pub async fn skill_statistics(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let skills = store::skills_of(&state.db, &agent_id).await?;

    let mut by_category = serde_json::Map::new();
    let mut by_level = serde_json::Map::new();
    let mut certified = 0i64;
    for s in &skills {
        let c = by_category.entry(s.category.clone()).or_insert(json!(0));
        *c = json!(c.as_i64().unwrap_or(0) + 1);
        let l = by_level.entry(s.level.clone()).or_insert(json!(0));
        *l = json!(l.as_i64().unwrap_or(0) + 1);
        if s.certified != 0 {
            certified += 1;
        }
    }
    let total = skills.len() as i64;
    // Certification rate as a percentage rounded to two decimals; zero when no skills
    // (CRD 2244).
    let rate = if total == 0 {
        0.0
    } else {
        ((certified as f64 / total as f64) * 10000.0).round() / 100.0
    };
    Ok(envelope::ok(json!({
        "total": total,
        "byCategory": by_category,
        "byLevel": by_level,
        "certifiedCount": certified,
        "certificationRate": rate,
    })))
}

// ------------------------------------------------------------- Presence (CRD 2247-2271)

pub async fn get_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let status = store::status_with_expiry(&state.db, &agent_id).await?;
    Ok(envelope::ok(store::status_view(&status)))
}

pub async fn update_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let body = parse_json(body)?;

    let status = body.get("status").and_then(Value::as_str).unwrap_or("");
    if status.is_empty() {
        return Err(AppError::BadRequest("status is required".into()));
    }
    if !PRESENCE_STATES.contains(&status) {
        return Err(AppError::BadRequest(format!(
            "status must be one of: {}",
            PRESENCE_STATES.join(", ")
        )));
    }
    let available_until = match body.get("availableUntil") {
        None | Some(Value::Null) => None,
        Some(v) => {
            // Must be a valid date strictly in the future (CRD 2258).
            let raw = v.as_str().unwrap_or("");
            let parsed = chrono::DateTime::parse_from_rfc3339(raw)
                .map_err(|_| AppError::BadRequest("availableUntil must be a valid date".into()))?;
            if parsed <= chrono::Utc::now() {
                return Err(AppError::BadRequest("availableUntil must be in the future".into()));
            }
            Some(parsed.to_utc().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        }
    };
    let note = match body.get("note") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let s = v.as_str().unwrap_or("").to_string();
            if s.chars().count() > 200 {
                return Err(AppError::BadRequest("note cannot exceed 200 characters".into()));
            }
            Some(s)
        }
    };

    if store::find_operator(&state.db, &agent_id).await?.is_none() {
        return Err(AppError::NotFound("Agent not found".into()));
    }
    let new_status =
        store::set_status(&state.db, &agent_id, status, available_until.as_deref(), note.as_deref())
            .await?;
    // Realtime: presence change broadcast to the operator's team(s) and to
    // administrators (CRD 2319, 3446) so assignment-eligibility consumers see
    // the transition; best-effort by construction.
    {
        let team_ids: Vec<i64> =
            sqlx::query_scalar("SELECT team_id FROM team_members WHERE agent_id = $1")
                .bind(&agent_id)
                .fetch_all(&state.db)
                .await
                .unwrap_or_default();
        let display_name: String =
            sqlx::query_scalar("SELECT display_name FROM agents WHERE id = $1")
                .bind(&agent_id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
        state.realtime.presence(&agent_id, &display_name, status, &team_ids);
    }
    Ok(envelope::ok_msg(store::status_view(&new_status), "Status updated successfully"))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<String>,
}

pub async fn status_history(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    // Default 20; caller-supplied with no enforced ceiling here (CRD 2267).
    let limit = q.limit.as_deref().and_then(|l| l.trim().parse::<i64>().ok()).unwrap_or(20).max(0);

    #[derive(sqlx::FromRow)]
    struct Row {
        status: String,
        since: String,
        available_until: Option<String>,
        note: Option<String>,
        recorded_at: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT status, since, available_until, note, recorded_at
         FROM agent_status_history WHERE agent_id = $1
         ORDER BY id DESC LIMIT $2",
    )
    .bind(&agent_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(rows
        .iter()
        .map(|r| json!({
            "status": r.status,
            "since": r.since,
            "availableUntil": r.available_until,
            "note": r.note,
            "recordedAt": r.recorded_at,
        }))
        .collect::<Vec<_>>()))
}

// ----------------------------------------------- Operator details & delete (CRD 2273-2297)

pub async fn get_agent(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
) -> Result {
    validate_agent_id(&agent_id)?;
    require_scope(&user, &agent_id)?;
    let operator = store::find_operator(&state.db, &agent_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Agent not found".into()))?;
    let skills = store::skills_of(&state.db, &agent_id).await?;
    let status = store::status_with_expiry(&state.db, &agent_id).await?;

    let mut view = store::operator_view(&operator);
    view["skills"] = json!(skills.iter().map(store::skill_view).collect::<Vec<_>>());
    view["currentStatus"] = store::status_view(&status);
    Ok(envelope::ok(view))
}

pub async fn delete_agent(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
) -> Result {
    require_admin(&user)?;
    validate_agent_id(&agent_id)?;
    if store::find_operator(&state.db, &agent_id).await?.is_none() {
        return Err(AppError::NotFound("Agent not found".into()));
    }
    // Reference cleanup + anonymization to the placeholder identity (CRD 2294).
    teams_store::purge_member(&state.db, &agent_id).await?;
    state.team_cache.invalidate(&agent_id);
    // TODO(realtime): emit operator-deleted domain event (CRD 2319).
    Ok(envelope::message_only("Agent deleted successfully"))
}

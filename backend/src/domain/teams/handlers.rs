//! Teams domain handlers (CRD §3.2, lines 1792-2154).
//!
//! `POST /api/teams/members/{memberId}/reset` lives in `crate::domain::auth`; the
//! self-service password change is mounted at `/api/auth/change-password`.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::now_iso;
use crate::domain::auth::store::{
    find_active_agent_by_email, find_deleted_agent_by_email, hash_password, log_activity,
};
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::{team_role_level, AuthUser};
use crate::state::AppState;

use super::store::{self, MemberRow, MembershipRow, QrRow, TeamWithCounts};

type Result<T = Response> = std::result::Result<T, AppError>;
type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

const BATCH_LIMIT: usize = 50;
const TEAM_ROLES: [&str; 3] = ["member", "lead", "supervisor"];
const GLOBAL_ROLES: [&str; 2] = ["admin", "agent"];

// ----------------------------------------------------------------------------- helpers

fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b).map_err(|_| AppError::BadRequest("Invalid JSON".into()))
}

/// Path id must be a positive integer (CRD 1831, 1835: non-integer id -> 400).
fn parse_team_id(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| AppError::BadRequest("Invalid team id".into()))
}

fn require_admin(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator role required".into()))
    }
}

/// Team-access check (CRD 1809): admins always pass; agents only for their own teams.
/// The denial message names the user's team and the requested team (CRD 1835).
fn require_team_access(user: &AuthUser, team_id: i64) -> Result<()> {
    if user.can_access_team(team_id) {
        return Ok(());
    }
    Err(AppError::Forbidden(format!(
        "Access denied: you belong to team {} but requested team {team_id}",
        user.primary_team_id.map_or_else(|| "none".to_string(), |t| t.to_string()),
    )))
}

/// In-team rank gate (CRD 1808): requires `required` rank or higher in this specific
/// team; administrators bypass. Denials carry required role, current role, and team id.
fn require_team_rank(user: &AuthUser, team_id: i64, required: &str) -> Result<()> {
    if user.is_admin() {
        return Ok(());
    }
    let current = user.team_role(team_id).unwrap_or("none");
    if team_role_level(current) >= team_role_level(required) {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!(
            "Insufficient role: requires {required} in team {team_id} (current role: {current})"
        )))
    }
}

fn lenient_i64(raw: &Option<String>) -> Option<i64> {
    raw.as_deref().and_then(|v| v.trim().parse::<i64>().ok())
}

/// Extract a non-empty string array of at most `BATCH_LIMIT` items (CRD 1903, 1925, 1984).
fn string_array(v: Option<&Value>, field: &str) -> Result<Vec<String>> {
    let arr = v
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest(format!("{field} must be a non-empty array")))?;
    if arr.len() > BATCH_LIMIT {
        return Err(AppError::BadRequest(format!(
            "{field} cannot contain more than {BATCH_LIMIT} entries"
        )));
    }
    Ok(arr
        .iter()
        .map(|e| match e {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect())
}

async fn team_exists(state: &AppState, id: i64) -> Result<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT id FROM teams WHERE id = ? AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    Ok(found.is_some())
}

async fn agent_exists(state: &AppState, id: &str) -> Result<bool> {
    let found: Option<String> =
        sqlx::query_scalar("SELECT id FROM agents WHERE id = ? AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    Ok(found.is_some())
}

// ------------------------------------------------------- Health & info (CRD 1814-1821)

pub async fn health() -> Response {
    envelope::ok(json!({
        "status": "healthy",
        "timestamp": now_iso(),
        "module": "teams",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub async fn info() -> Response {
    envelope::ok(json!({
        "module": "teams",
        "version": env!("CARGO_PKG_VERSION"),
        "endpoints": [
            "GET / - list teams",
            "POST / - create team",
            "GET /:id - get team",
            "PUT /:id - update team",
            "DELETE /:id - delete team",
            "GET /search/:query - search teams",
            "GET /:id/stats - team statistics",
            "GET /stats/all - all team statistics",
            "POST /transfer - transfer agents between teams",
            "GET|POST /:id/members - team member management",
            "GET|POST /members - member account management",
            "GET|PUT|DELETE /agent-teams - agent-team associations",
            "GET|POST /:id/qr-code - team QR codes",
        ],
    }))
}

// ------------------------------------------------------------ List teams (CRD 1823-1828)

#[derive(Deserialize)]
pub struct ListTeamsQuery {
    pub page: Option<String>,
    pub limit: Option<String>,
    #[serde(rename = "includeInactive")]
    pub include_inactive: Option<String>,
    pub search: Option<String>,
}

pub async fn list_teams(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListTeamsQuery>,
) -> Result {
    // Scoping: an agent with a primary team receives only that team; listing
    // parameters are ignored for that user (CRD 1826).
    if !user.is_admin() {
        if let Some(primary) = user.primary_team_id {
            let team = store::team_with_counts(&state.db, primary).await?;
            let items: Vec<Value> = team.iter().map(store::team_view).collect();
            return Ok(envelope::ok(items));
        }
    }

    let page = lenient_i64(&q.page).unwrap_or(1).max(1);
    let limit = lenient_i64(&q.limit).unwrap_or(20).clamp(1, 100);
    let include_inactive = q.include_inactive.as_deref() == Some("true");
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let mut filter = String::new();
    if !include_inactive {
        filter.push_str(" AND t.is_active = 1");
    }
    if search.is_some() {
        filter.push_str(" AND (LOWER(t.name) LIKE ? OR LOWER(COALESCE(t.description,'')) LIKE ?)");
    }
    let pattern = search.map(|s| format!("%{}%", s.to_lowercase()));

    let count_sql = format!(
        "SELECT COUNT(*) FROM teams t WHERE t.deleted_at IS NULL {filter}"
    );
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    if let Some(p) = &pattern {
        count_q = count_q.bind(p.clone()).bind(p.clone());
    }
    let total = count_q.fetch_one(&state.db).await?;

    let list_sql = store::team_select(&filter, "ORDER BY t.created_at DESC, t.id DESC LIMIT ? OFFSET ?");
    let mut list_q = sqlx::query_as::<_, TeamWithCounts>(&list_sql);
    if let Some(p) = &pattern {
        list_q = list_q.bind(p.clone()).bind(p.clone());
    }
    let rows = list_q.bind(limit).bind((page - 1) * limit).fetch_all(&state.db).await?;
    let items: Vec<Value> = rows.iter().map(store::team_view).collect();
    Ok(envelope::ok_with_pagination(items, page, limit, total))
}

// -------------------------------------------------------------- Get team (CRD 1830-1835)

pub async fn get_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let team = store::team_with_counts(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Team not found".into()))?;
    let mut view = store::team_view(&team);
    // QR-scan count currently always zero within the behavioral boundary (CRD 1833).
    view["qrScanCount"] = json!(0);
    Ok(envelope::ok(view))
}

// ----------------------------------------------------------- Create team (CRD 1837-1844)

#[derive(Deserialize)]
pub struct CreateTeamBody {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "qrCode")]
    pub qr_code: Option<String>,
    #[serde(rename = "isActive")]
    pub is_active: Option<bool>,
}

pub async fn create_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<CreateTeamBody>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let name = body.name.as_deref().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Team name is required".into()));
    }
    // QR-code value uniqueness IS enforced when provided (CRD 1844).
    if let Some(qr) = body.qr_code.as_deref().filter(|s| !s.is_empty()) {
        let used: Option<i64> = sqlx::query_scalar("SELECT id FROM teams WHERE qr_code = ?")
            .bind(qr)
            .fetch_optional(&state.db)
            .await?;
        if used.is_some() {
            return Err(AppError::Conflict("A team with this QR code already exists".into()));
        }
    }

    let now = now_iso();
    let is_active = body.is_active.unwrap_or(true);
    let team_id = sqlx::query(
        "INSERT INTO teams (name, description, is_active, qr_code, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&name)
    .bind(&body.description)
    .bind(is_active as i64)
    .bind(&body.qr_code)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?
    .last_insert_rowid();

    // Reversible create audit entry (CRD 1840).
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "team create", "team", Some(&team_id.to_string()),
        Some(json!({
            "reversible": true,
            "old": null,
            "new": { "name": name, "description": body.description, "isActive": is_active },
        })),
        None, None,
    )
    .await;

    // QR artifacts are generated after persistence; failures do not fail the
    // request — the team is returned without the failed artifact (CRD 1840).
    let join_qr = store::create_join_qr(&state.db, &state.config, team_id, None, None, None, None)
        .await
        .ok();
    if let Some(qr) = &join_qr {
        let _ = sqlx::query("UPDATE teams SET qr_code_image = ? WHERE id = ?")
            .bind(&qr.image_url)
            .bind(team_id)
            .execute(&state.db)
            .await;
    }
    let liff = store::upsert_liff(&state.db, team_id).await.ok();

    let team = store::team_with_counts(&state.db, team_id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload team after create".into()))?;
    let mut view = store::team_view(&team);
    if let Some(qr) = &join_qr {
        view["qrCodeImage"] = json!(qr.image_url);
        view["joinUrl"] = json!(qr.url);
    }
    if let Some(l) = &liff {
        view["liffQr"] = json!({ "id": l.id, "url": l.url, "imageUrl": l.image_url });
    }
    Ok(envelope::with_status(StatusCode::CREATED, Some(view), Some("Team created successfully")))
}

// ----------------------------------------------------------- Update team (CRD 1846-1852)

#[derive(Deserialize)]
pub struct UpdateTeamBody {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "isActive")]
    pub is_active: Option<bool>,
}

pub async fn update_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<UpdateTeamBody>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "supervisor")?;
    let body = parse_json(body)?;
    let current = store::team_with_counts(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Team not found".into()))?;

    let mut old = serde_json::Map::new();
    let mut new = serde_json::Map::new();
    let now = now_iso();
    if let Some(name) = body.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        old.insert("name".into(), json!(current.name));
        new.insert("name".into(), json!(name));
        sqlx::query("UPDATE teams SET name = ?, updated_at = ? WHERE id = ?")
            .bind(name)
            .bind(&now)
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    if let Some(description) = &body.description {
        old.insert("description".into(), json!(current.description));
        new.insert("description".into(), json!(description));
        sqlx::query("UPDATE teams SET description = ?, updated_at = ? WHERE id = ?")
            .bind(description)
            .bind(&now)
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    if let Some(active) = body.is_active {
        old.insert("isActive".into(), json!(current.is_active != 0));
        new.insert("isActive".into(), json!(active));
        sqlx::query("UPDATE teams SET is_active = ?, updated_at = ? WHERE id = ?")
            .bind(active as i64)
            .bind(&now)
            .bind(id)
            .execute(&state.db)
            .await?;
    }

    // Reversible update audit entry capturing before/after state (CRD 1849).
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "team update", "team", Some(&id.to_string()),
        Some(json!({ "reversible": true, "old": old, "new": new })),
        None, None,
    )
    .await;

    // TODO(realtime): broadcast team-information-update (name/description/active/member
    // count + initiating user) to management views (CRD 2152).
    let updated = store::team_with_counts(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload team after update".into()))?;
    Ok(envelope::ok_msg(store::team_view(&updated), "Team updated successfully"))
}

// ----------------------------------------------------------- Delete team (CRD 1854-1860)

pub async fn delete_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    require_admin(&user)?;
    let id = parse_team_id(&raw_id)?;
    let current = store::team_with_counts(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Team not found".into()))?;

    // Soft delete plus reversible delete audit entry (CRD 1857).
    let now = now_iso();
    sqlx::query("UPDATE teams SET deleted_at = ?, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(&state.db)
        .await?;
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "team delete", "team", Some(&id.to_string()),
        Some(json!({
            "reversible": true,
            "old": { "name": current.name, "deletedAt": null },
            "new": { "deletedAt": now },
        })),
        None, None,
    )
    .await;
    Ok(envelope::message_only("Team deleted successfully"))
}

// ---------------------------------------------------------- Search teams (CRD 1862-1867)

pub async fn search_teams(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_query): Path<String>,
) -> Result {
    let query = raw_query.trim().to_string();
    if query.is_empty() {
        return Err(AppError::BadRequest("Search query is required".into()));
    }
    let pattern = format!("%{}%", query.to_lowercase());
    let sql = store::team_select(
        "AND t.is_active = 1
         AND (LOWER(t.name) LIKE ? OR LOWER(COALESCE(t.description,'')) LIKE ?)",
        "ORDER BY t.created_at DESC, t.id DESC LIMIT 20",
    );
    let rows: Vec<TeamWithCounts> = sqlx::query_as(&sql)
        .bind(&pattern)
        .bind(&pattern)
        .fetch_all(&state.db)
        .await?;
    Ok(envelope::ok(rows.iter().map(store::team_view).collect::<Vec<_>>()))
}

// ------------------------------------------------------- Team statistics (CRD 1869-1880)

#[derive(Deserialize)]
pub struct StatsQuery {
    #[serde(rename = "dateFrom")]
    pub date_from: Option<String>,
    #[serde(rename = "dateTo")]
    pub date_to: Option<String>,
    #[serde(rename = "includeMembers")]
    pub include_members: Option<String>,
}

async fn build_team_stats(
    state: &AppState,
    team: &TeamWithCounts,
    from: &str,
    to: &str,
    include_members: bool,
) -> Result<Value> {
    let conversations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversations
         WHERE team_id = ? AND deleted_at IS NULL AND created_at >= ? AND created_at <= ?",
    )
    .bind(team.id)
    .bind(from)
    .bind(to)
    .fetch_one(&state.db)
    .await?;
    let messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages m
         JOIN conversations c ON c.id = m.conversation_id AND c.deleted_at IS NULL
         WHERE c.team_id = ? AND m.deleted_at IS NULL
           AND m.created_at >= ? AND m.created_at <= ?",
    )
    .bind(team.id)
    .bind(from)
    .bind(to)
    .fetch_one(&state.db)
    .await?;

    let mut stats = json!({
        "teamId": team.id,
        "teamName": team.name,
        "totalMembers": team.member_count,
        "activeMembers": team.active_member_count,
        "conversationsHandled": conversations,
        "totalMessages": messages,
        // Always zero within the current behavioral boundary (CRD 1872, 2137).
        "averageResponseTime": 0,
        "qrScans": 0,
        "period": { "from": from, "to": to },
    });
    if include_members {
        stats["members"] = team_member_list(state, team.id).await?;
    }
    Ok(stats)
}

fn default_period(q: &StatsQuery) -> (String, String) {
    let from = q.date_from.clone().unwrap_or_else(|| {
        (chrono::Utc::now() - chrono::Duration::days(30))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    });
    let to = q.date_to.clone().unwrap_or_else(now_iso);
    (from, to)
}

pub async fn team_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    Query(q): Query<StatsQuery>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    // Nonexistent team surfaces as a server error (CRD 1874).
    let team = store::team_with_counts(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Internal("Team not found".into()))?;
    let (from, to) = default_period(&q);
    let include_members = q.include_members.as_deref() == Some("true");
    let stats = build_team_stats(&state, &team, &from, &to, include_members).await?;
    Ok(envelope::ok(stats))
}

pub async fn all_team_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<StatsQuery>,
) -> Result {
    require_admin(&user)?;
    let (from, to) = default_period(&q);
    let include_members = q.include_members.as_deref() == Some("true");
    let sql = store::team_select("AND t.is_active = 1", "ORDER BY t.id");
    let teams: Vec<TeamWithCounts> = sqlx::query_as(&sql).fetch_all(&state.db).await?;
    let mut out = Vec::with_capacity(teams.len());
    for team in &teams {
        out.push(build_team_stats(&state, team, &from, &to, include_members).await?);
    }
    Ok(envelope::ok(out))
}

// ------------------------------------------------------- Transfer agents (CRD 1882-1887)

#[derive(Deserialize)]
pub struct TransferBody {
    #[serde(rename = "fromTeamId")]
    pub from_team_id: Option<i64>,
    #[serde(rename = "toTeamId")]
    pub to_team_id: Option<i64>,
    #[serde(rename = "agentIds")]
    pub agent_ids: Option<Value>,
    pub reason: Option<String>,
}

pub async fn transfer_agents(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<TransferBody>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let from = body.from_team_id.ok_or_else(|| AppError::BadRequest("fromTeamId is required".into()))?;
    let to = body.to_team_id.ok_or_else(|| AppError::BadRequest("toTeamId is required".into()))?;
    let agent_ids = string_array(body.agent_ids.as_ref(), "agentIds")?;

    let mut transferred: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let now = now_iso();
    for agent_id in &agent_ids {
        let Some(membership) = store::find_membership(&state.db, agent_id, from).await? else {
            failed.push(json!({
                "agentId": agent_id,
                "reason": "Agent is not a member of the source team",
            }));
            continue;
        };
        sqlx::query("DELETE FROM team_members WHERE agent_id = ? AND team_id = ?")
            .bind(agent_id)
            .bind(from)
            .execute(&state.db)
            .await?;
        // Primary-team flag and in-team role are preserved across the move (CRD 1885).
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(agent_id, team_id)
             DO UPDATE SET is_primary = MAX(is_primary, excluded.is_primary)",
        )
        .bind(agent_id)
        .bind(to)
        .bind(&membership.role)
        .bind(membership.is_primary)
        .bind(&now)
        .execute(&state.db)
        .await?;
        state.team_cache.invalidate(agent_id);
        transferred.push(agent_id.clone());
    }

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "team transfer", "team", Some(&to.to_string()),
        Some(json!({
            "fromTeamId": from, "toTeamId": to,
            "transferred": transferred, "reason": body.reason,
        })),
        None, None,
    )
    .await;

    // Overall success flag is true only when there were no failures (CRD 1886).
    let success = failed.is_empty();
    Ok(envelope::flagged(
        success,
        json!({ "transferred": transferred, "failed": failed }),
        Some(if success { "All agents transferred" } else { "Some transfers failed" }),
    ))
}

// ------------------------------------------------ Team-scoped member ops (CRD 1889-1929)

async fn team_member_list(state: &AppState, team_id: i64) -> Result<Value> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        email: String,
        display_name: String,
        global_role: String,
        is_active: i64,
        created_at: String,
        updated_at: Option<String>,
        team_role: String,
        is_primary: i64,
        joined_at: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT a.id, a.email, a.display_name, a.role AS global_role, a.is_active,
                a.created_at, a.updated_at, m.role AS team_role, m.is_primary, m.joined_at
         FROM team_members m
         JOIN agents a ON a.id = m.agent_id AND a.deleted_at IS NULL
         WHERE m.team_id = ?
         ORDER BY a.display_name",
    )
    .bind(team_id)
    .fetch_all(&state.db)
    .await?;
    Ok(json!(rows
        .iter()
        .map(|r| json!({
            "id": r.id,
            "email": r.email,
            "displayName": r.display_name,
            "role": r.global_role,
            "roleInTeam": r.team_role,
            "isActive": r.is_active != 0,
            "isPrimary": r.is_primary != 0,
            "joinedAt": r.joined_at,
            "createdAt": r.created_at,
            "updatedAt": r.updated_at,
        }))
        .collect::<Vec<_>>()))
}

pub async fn team_members(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    Ok(envelope::ok(team_member_list(&state, id).await?))
}

#[derive(Deserialize)]
pub struct AddMemberBody {
    #[serde(rename = "agentId")]
    pub agent_id: Option<String>,
    pub role: Option<String>,
}

pub async fn add_member(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<AddMemberBody>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "lead")?;
    let body = parse_json(body)?;
    let agent_id = body.agent_id.as_deref().unwrap_or("").trim().to_string();
    if agent_id.is_empty() {
        return Err(AppError::BadRequest("agentId is required".into()));
    }
    if !agent_exists(&state, &agent_id).await? {
        return Err(AppError::NotFound("Agent not found".into()));
    }
    if !team_exists(&state, id).await? {
        return Err(AppError::NotFound("Team not found".into()));
    }

    let membership = match store::find_membership(&state.db, &agent_id, id).await? {
        // If a membership already exists, no duplicate is created (CRD 1898).
        Some(m) => m,
        None => {
            // First-ever membership is automatically primary; the new in-team role is
            // the base member level regardless of the `role` input (CRD 1898).
            let is_primary = store::memberships_of(&state.db, &agent_id).await?.is_empty();
            let now = now_iso();
            sqlx::query(
                "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
                 VALUES (?, ?, 'member', ?, ?)",
            )
            .bind(&agent_id)
            .bind(id)
            .bind(is_primary as i64)
            .bind(&now)
            .execute(&state.db)
            .await?;
            state.team_cache.invalidate(&agent_id);
            MembershipRow { agent_id: agent_id.clone(), team_id: id, role: "member".into(), is_primary: is_primary as i64, joined_at: now }
        }
    };
    Ok(envelope::with_status(
        StatusCode::CREATED,
        Some(store::membership_view(&membership)),
        Some("Member added to team"),
    ))
}

#[derive(Deserialize)]
pub struct BatchAddBody {
    #[serde(rename = "agentIds")]
    pub agent_ids: Option<Value>,
    #[serde(rename = "roleInTeam")]
    pub role_in_team: Option<String>,
}

pub async fn batch_add_members(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<BatchAddBody>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "lead")?;
    let body = parse_json(body)?;
    let agent_ids = string_array(body.agent_ids.as_ref(), "agentIds")?;
    let role = body.role_in_team.as_deref().unwrap_or("member").to_string();
    if !TEAM_ROLES.contains(&role.as_str()) {
        return Err(AppError::BadRequest(
            "roleInTeam must be one of: member, lead, supervisor".into(),
        ));
    }
    if !team_exists(&state, id).await? {
        return Err(AppError::NotFound("Team not found".into()));
    }

    let mut added: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let now = now_iso();
    for agent_id in &agent_ids {
        if !agent_exists(&state, agent_id).await? {
            errors.push(json!({ "agentId": agent_id, "error": "Agent not found" }));
            continue;
        }
        if store::find_membership(&state.db, agent_id, id).await?.is_some() {
            skipped.push(agent_id.clone());
            continue;
        }
        // New batch memberships are NOT primary (CRD 1905).
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
             VALUES (?, ?, ?, 0, ?)",
        )
        .bind(agent_id)
        .bind(id)
        .bind(&role)
        .bind(&now)
        .execute(&state.db)
        .await?;
        state.team_cache.invalidate(agent_id);
        added.push(agent_id.clone());
        // TODO(realtime): emit "member added" event with refreshed member count per
        // newly added membership, asynchronously (CRD 1908, 2149).
    }

    // Audit logging runs asynchronously / non-blocking (CRD 1905).
    {
        let db = state.db.clone();
        let (uid, uname, urole) = (user.id.clone(), user.display_name.clone(), user.role.clone());
        let detail = json!({ "teamId": id, "added": added, "skipped": skipped, "roleInTeam": role });
        tokio::spawn(async move {
            log_activity(&db, &uid, &uname, &urole, "team batch add members", "team",
                Some(&id.to_string()), Some(detail), None, None).await;
        });
    }

    let status = if added.is_empty() { StatusCode::OK } else { StatusCode::CREATED };
    Ok(envelope::with_status(
        status,
        Some(json!({
            "added": added,
            "skipped": skipped,
            "errors": errors,
            "addedCount": added.len(),
        })),
        Some("Batch add completed"),
    ))
}

#[derive(Deserialize)]
pub struct UpdateTeamMemberBody {
    pub role: Option<String>,
    #[serde(rename = "isActive")]
    pub is_active: Option<bool>,
}

pub async fn update_team_member(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((raw_id, raw_agent)): Path<(String, String)>,
    body: JsonBody<UpdateTeamMemberBody>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "lead")?;
    let agent_id = raw_agent.trim().to_string();
    if agent_id.is_empty() {
        return Err(AppError::BadRequest("agentId is required".into()));
    }
    let body = parse_json(body)?;
    let member = store::find_member(&state.db, &agent_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    // This endpoint updates the global account record, not the per-team role (CRD 1913).
    let now = now_iso();
    if let Some(role) = body.role.as_deref() {
        if !GLOBAL_ROLES.contains(&role) {
            return Err(AppError::BadRequest("role must be one of: admin, agent".into()));
        }
        sqlx::query("UPDATE agents SET role = ?, updated_at = ? WHERE id = ?")
            .bind(role)
            .bind(&now)
            .bind(&agent_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(active) = body.is_active {
        sqlx::query("UPDATE agents SET is_active = ?, updated_at = ? WHERE id = ?")
            .bind(active as i64)
            .bind(&now)
            .bind(&agent_id)
            .execute(&state.db)
            .await?;
    }
    let updated = store::find_member(&state.db, &agent_id).await?.unwrap_or(member);
    Ok(envelope::ok_msg(store::member_view(&updated), "Member updated"))
}

pub async fn remove_team_member(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((raw_id, raw_agent)): Path<(String, String)>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "lead")?;
    let agent_id = raw_agent.trim().to_string();
    if agent_id.is_empty() {
        return Err(AppError::BadRequest("agentId is required".into()));
    }

    // Removing an agent not in the team is a no-success outcome (CRD 1922).
    let Some(membership) = store::find_membership(&state.db, &agent_id, id).await? else {
        return Ok(envelope::flagged(
            false,
            json!({ "removed": false }),
            Some("Agent is not a member of this team"),
        ));
    };

    sqlx::query("DELETE FROM team_members WHERE agent_id = ? AND team_id = ?")
        .bind(&agent_id)
        .bind(id)
        .execute(&state.db)
        .await?;
    // If the removed membership was primary, promote another remaining team (CRD 1920).
    let promoted = store::promote_primary_if_needed(&state.db, &agent_id).await?;
    state.team_cache.invalidate(&agent_id);

    // Reversible audit entry capturing prior membership and any promotion (CRD 1920).
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "team remove member", "team_member", Some(&agent_id),
        Some(json!({
            "reversible": true,
            "old": store::membership_view(&membership),
            "new": null,
            "promotedTeamId": promoted,
        })),
        None, None,
    )
    .await;

    Ok(envelope::flagged(
        true,
        json!({ "removed": true, "promotedTeamId": promoted }),
        Some("Member removed from team"),
    ))
}

#[derive(Deserialize)]
pub struct BulkRemoveBody {
    #[serde(rename = "agentIds")]
    pub agent_ids: Option<Value>,
}

pub async fn bulk_remove_members(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<BulkRemoveBody>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "lead")?;
    let body = parse_json(body)?;
    let agent_ids = string_array(body.agent_ids.as_ref(), "agentIds")?;

    let mut removed: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    for agent_id in &agent_ids {
        if store::find_membership(&state.db, agent_id, id).await?.is_none() {
            failed.push(json!({ "agentId": agent_id, "reason": "Agent is not a member of this team" }));
            continue;
        }
        // The bulk path does not perform primary-team promotion (CRD 1927).
        sqlx::query("DELETE FROM team_members WHERE agent_id = ? AND team_id = ?")
            .bind(agent_id)
            .bind(id)
            .execute(&state.db)
            .await?;
        state.team_cache.invalidate(agent_id);
        removed.push(agent_id.clone());
    }
    Ok(envelope::ok(json!({
        "removed": removed,
        "failed": failed,
        "removedCount": removed.len(),
    })))
}

// ----------------------------------------------------- Member accounts (CRD 1933-2009)

#[derive(sqlx::FromRow)]
struct MemberTeamRow {
    agent_id: String,
    team_id: i64,
    team_name: String,
    role: String,
    is_primary: i64,
    joined_at: String,
}

async fn memberships_with_names(state: &AppState) -> Result<HashMap<String, Vec<MemberTeamRow>>> {
    let rows: Vec<MemberTeamRow> = sqlx::query_as(
        "SELECT m.agent_id, m.team_id, t.name AS team_name, m.role, m.is_primary, m.joined_at
         FROM team_members m
         JOIN teams t ON t.id = m.team_id AND t.deleted_at IS NULL
         ORDER BY m.is_primary DESC, m.team_id",
    )
    .fetch_all(&state.db)
    .await?;
    let mut map: HashMap<String, Vec<MemberTeamRow>> = HashMap::new();
    for row in rows {
        map.entry(row.agent_id.clone()).or_default().push(row);
    }
    Ok(map)
}

fn enriched_member_view(m: &MemberRow, teams: Option<&Vec<MemberTeamRow>>) -> Value {
    let mut view = store::member_view(m);
    let empty = Vec::new();
    let teams = teams.unwrap_or(&empty);
    view["teams"] = json!(teams
        .iter()
        .map(|t| json!({
            "teamId": t.team_id,
            "teamName": t.team_name,
            "roleInTeam": t.role,
            "isPrimary": t.is_primary != 0,
            "joinedAt": t.joined_at,
        }))
        .collect::<Vec<_>>());
    view["teamCount"] = json!(teams.len());
    let primary = teams.iter().find(|t| t.is_primary != 0);
    view["primaryTeamId"] = json!(primary.map(|t| t.team_id));
    view["primaryTeamName"] = json!(primary.map(|t| t.team_name.clone()));
    view
}

pub async fn list_all_members(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_admin(&user)?;
    let members: Vec<MemberRow> = sqlx::query_as(&format!(
        "SELECT {} FROM agents WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC",
        store::MEMBER_COLUMNS
    ))
    .fetch_all(&state.db)
    .await?;
    let team_map = memberships_with_names(&state).await?;
    let items: Vec<Value> = members
        .iter()
        .map(|m| enriched_member_view(m, team_map.get(&m.id)))
        .collect();
    Ok(envelope::ok(items))
}

#[derive(Deserialize)]
pub struct CheckEmailQuery {
    pub email: Option<String>,
}

pub async fn check_email(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<CheckEmailQuery>,
) -> Result {
    require_admin(&user)?;
    let email = q.email.as_deref().unwrap_or("").trim().to_string();
    if email.is_empty() {
        return Err(AppError::BadRequest("email is required".into()));
    }
    // Active accounts take precedence over soft-deleted ones (CRD 1942).
    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        display_name: String,
        role: String,
        is_active: i64,
        last_login_at: Option<String>,
        created_at: String,
        deleted_at: Option<String>,
    }
    let found: Option<Row> = sqlx::query_as(
        "SELECT id, display_name, role, is_active, last_login_at, created_at, deleted_at
         FROM agents WHERE email = ?
         ORDER BY (deleted_at IS NULL) DESC, created_at DESC LIMIT 1",
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;

    let Some(row) = found else {
        return Ok(envelope::ok(json!({ "exists": false })));
    };
    let primary_team_name: Option<String> = sqlx::query_scalar(
        "SELECT t.name FROM team_members m JOIN teams t ON t.id = m.team_id
         WHERE m.agent_id = ? AND m.is_primary = 1 LIMIT 1",
    )
    .bind(&row.id)
    .fetch_optional(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "exists": true,
        "isDeleted": row.deleted_at.is_some(),
        "isActive": row.is_active != 0,
        "displayName": row.display_name,
        "role": row.role,
        "primaryTeamName": primary_team_name,
        "lastLoginAt": row.last_login_at,
        "createdAt": row.created_at,
        "deletedAt": row.deleted_at,
    })))
}

#[derive(Deserialize)]
pub struct CreateMemberBody {
    pub email: Option<String>,
    pub password: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub role: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
    #[serde(rename = "isActive")]
    pub is_active: Option<bool>,
}

pub async fn create_member(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<CreateMemberBody>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let email = body.email.as_deref().unwrap_or("").trim().to_string();
    let password = body.password.as_deref().unwrap_or("").to_string();
    let display_name = body.display_name.as_deref().unwrap_or("").trim().to_string();
    if email.is_empty() || password.is_empty() || display_name.is_empty() {
        return Err(AppError::BadRequest("email, password and displayName are required".into()));
    }
    let role = body.role.as_deref().unwrap_or("agent").to_string();
    if !GLOBAL_ROLES.contains(&role.as_str()) {
        return Err(AppError::BadRequest("role must be one of: admin, agent".into()));
    }
    if find_active_agent_by_email(&state.db, &email).await?.is_some() {
        return Err(AppError::Conflict("A member with this email already exists".into()));
    }

    let hash = hash_password(&password)
        .map_err(|e| AppError::Internal(format!("password hashing failed: {e}")))?;
    let now = now_iso();
    let is_active = body.is_active.unwrap_or(true);

    // A soft-deleted same-email account is reactivated; its prior team memberships
    // are cleared (CRD 1949).
    let member_id = if let Some(old) = find_deleted_agent_by_email(&state.db, &email).await? {
        sqlx::query("DELETE FROM team_members WHERE agent_id = ?")
            .bind(&old.id)
            .execute(&state.db)
            .await?;
        sqlx::query(
            "UPDATE agents SET password_hash = ?, display_name = ?, role = ?, is_active = ?,
             password_policy = 'changeable', deleted_at = NULL, updated_at = ? WHERE id = ?",
        )
        .bind(&hash)
        .bind(&display_name)
        .bind(&role)
        .bind(is_active as i64)
        .bind(&now)
        .bind(&old.id)
        .execute(&state.db)
        .await?;
        old.id
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO agents (id, email, password_hash, display_name, role, is_active, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&email)
        .bind(&hash)
        .bind(&display_name)
        .bind(&role)
        .bind(is_active as i64)
        .bind(&now)
        .execute(&state.db)
        .await?;
        id
    };

    if let Some(team_id) = body.team_id {
        if !team_exists(&state, team_id).await? {
            return Err(AppError::BadRequest(format!("Team {team_id} not found")));
        }
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
             VALUES (?, ?, 'member', 1, ?)",
        )
        .bind(&member_id)
        .bind(team_id)
        .bind(&now)
        .execute(&state.db)
        .await?;
    }
    state.team_cache.invalidate(&member_id);

    // Account-create audit entry (CRD 1949).
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "member create", "agent", Some(&member_id),
        Some(json!({ "email": email, "displayName": display_name, "role": role, "teamId": body.team_id })),
        None, None,
    )
    .await;

    let member = store::find_member(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload member after create".into()))?;
    Ok(envelope::with_status(
        StatusCode::CREATED,
        Some(store::member_view(&member)),
        Some("Member created successfully"),
    ))
}

pub async fn set_member_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(member_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let Some(is_active) = body.get("isActive").and_then(Value::as_bool) else {
        return Err(AppError::BadRequest("isActive is required".into()));
    };
    if member_id == user.id {
        return Err(AppError::Forbidden("You cannot change your own status".into()));
    }
    let member = store::find_member(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    sqlx::query("UPDATE agents SET is_active = ?, updated_at = ? WHERE id = ?")
        .bind(is_active as i64)
        .bind(now_iso())
        .bind(&member_id)
        .execute(&state.db)
        .await?;

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "member status update", "agent", Some(&member_id),
        Some(json!({
            "old": { "isActive": member.is_active != 0 },
            "new": { "isActive": is_active },
            "reason": body.get("reason"),
        })),
        None, None,
    )
    .await;

    let updated = store::find_member(&state.db, &member_id).await?.unwrap_or(member);
    Ok(envelope::ok_msg(store::member_view(&updated), "Member status updated"))
}

pub async fn set_member_role(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(member_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let Some(role) = body.get("role").and_then(Value::as_str) else {
        return Err(AppError::BadRequest("role is required".into()));
    };
    if !GLOBAL_ROLES.contains(&role) {
        return Err(AppError::BadRequest("role must be one of: admin, agent".into()));
    }
    if member_id == user.id {
        return Err(AppError::Forbidden("You cannot change your own role".into()));
    }
    let member = store::find_member(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    sqlx::query("UPDATE agents SET role = ?, updated_at = ? WHERE id = ?")
        .bind(role)
        .bind(now_iso())
        .bind(&member_id)
        .execute(&state.db)
        .await?;

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "member role update", "agent", Some(&member_id),
        Some(json!({
            "old": { "role": member.role },
            "new": { "role": role },
            "reason": body.get("reason"),
        })),
        None, None,
    )
    .await;

    let updated = store::find_member(&state.db, &member_id).await?.unwrap_or(member);
    Ok(envelope::ok_msg(store::member_view(&updated), "Member role updated"))
}

/// Applies a subset of email/displayName/role/isActive to an account, returning the
/// (old, new) diff of applied fields. Shared by single and bulk/batch updates.
async fn apply_member_updates(
    state: &AppState,
    member: &MemberRow,
    updates: &Value,
) -> Result<(Value, Value)> {
    let mut old = serde_json::Map::new();
    let mut new = serde_json::Map::new();
    let now = now_iso();

    if let Some(email) = updates.get("email").and_then(Value::as_str) {
        let email = email.trim().to_string();
        if email.is_empty() {
            return Err(AppError::BadRequest("email cannot be empty".into()));
        }
        if email != member.email {
            if find_active_agent_by_email(&state.db, &email).await?.is_some() {
                return Err(AppError::Conflict("Email already in use by another member".into()));
            }
            sqlx::query("UPDATE agents SET email = ?, updated_at = ? WHERE id = ?")
                .bind(&email)
                .bind(&now)
                .bind(&member.id)
                .execute(&state.db)
                .await?;
            old.insert("email".into(), json!(member.email));
            new.insert("email".into(), json!(email));
        }
    }
    if let Some(name) = updates.get("displayName").and_then(Value::as_str) {
        let name = name.trim().to_string();
        if !name.is_empty() && name != member.display_name {
            sqlx::query("UPDATE agents SET display_name = ?, updated_at = ? WHERE id = ?")
                .bind(&name)
                .bind(&now)
                .bind(&member.id)
                .execute(&state.db)
                .await?;
            old.insert("displayName".into(), json!(member.display_name));
            new.insert("displayName".into(), json!(name));
        }
    }
    if let Some(role) = updates.get("role").and_then(Value::as_str) {
        if !GLOBAL_ROLES.contains(&role) {
            return Err(AppError::BadRequest("role must be one of: admin, agent".into()));
        }
        if role != member.role {
            sqlx::query("UPDATE agents SET role = ?, updated_at = ? WHERE id = ?")
                .bind(role)
                .bind(&now)
                .bind(&member.id)
                .execute(&state.db)
                .await?;
            old.insert("role".into(), json!(member.role));
            new.insert("role".into(), json!(role));
        }
    }
    if let Some(active) = updates.get("isActive").and_then(Value::as_bool) {
        if active != (member.is_active != 0) {
            sqlx::query("UPDATE agents SET is_active = ?, updated_at = ? WHERE id = ?")
                .bind(active as i64)
                .bind(&now)
                .bind(&member.id)
                .execute(&state.db)
                .await?;
            old.insert("isActive".into(), json!(member.is_active != 0));
            new.insert("isActive".into(), json!(active));
        }
    }
    Ok((Value::Object(old), Value::Object(new)))
}

pub async fn update_member_account(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(member_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let member = store::find_member(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    let (old, new) = apply_member_updates(&state, &member, &body).await?;

    // Audit entry diffing all supplied fields (CRD 1971).
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "member update", "agent", Some(&member_id),
        Some(json!({ "old": old, "new": new })),
        None, None,
    )
    .await;

    let updated = store::find_member(&state.db, &member_id).await?.unwrap_or(member);
    Ok(envelope::ok_msg(store::member_view(&updated), "Member updated"))
}

pub async fn delete_member_account(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(member_id): Path<String>,
) -> Result {
    require_admin(&user)?;
    if member_id == user.id {
        return Err(AppError::Forbidden("You cannot delete your own account".into()));
    }
    let member = store::find_member(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    // Permanent deletion with reference cleanup (CRD 1978).
    store::purge_member(&state.db, &member_id).await?;
    state.team_cache.invalidate(&member_id);

    // Audit logging of the deletion is attempted but never fails the request (CRD 1978):
    // log_activity is best-effort by construction.
    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "member delete", "agent", Some(&member_id),
        Some(json!({ "email": member.email, "displayName": member.display_name, "permanent": true })),
        None, None,
    )
    .await;

    Ok(envelope::ok_msg(
        json!({ "deletedMemberId": member_id }),
        "Member deleted permanently",
    ))
}

#[derive(Deserialize)]
pub struct BulkDeleteBody {
    #[serde(rename = "memberIds")]
    pub member_ids: Option<Value>,
    pub reason: Option<String>,
}

pub async fn bulk_delete_members(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<BulkDeleteBody>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let member_ids = string_array(body.member_ids.as_ref(), "memberIds")?;
    // A list including one's own id is rejected outright (CRD 1988).
    if member_ids.contains(&user.id) {
        return Err(AppError::Forbidden("You cannot delete your own account".into()));
    }

    let mut deleted: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    for member_id in &member_ids {
        if store::find_member(&state.db, member_id).await?.is_none() {
            failed.push(json!({ "memberId": member_id, "reason": "Member not found" }));
            continue;
        }
        store::purge_member(&state.db, member_id).await?;
        state.team_cache.invalidate(member_id);
        deleted.push(member_id.clone());
    }

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "member bulk delete", "agent", None,
        Some(json!({ "deleted": deleted, "reason": body.reason, "permanent": true })),
        None, None,
    )
    .await;

    Ok(envelope::ok(json!({
        "deleted": deleted,
        "failed": failed,
        "deletedCount": deleted.len(),
    })))
}

#[derive(Deserialize)]
pub struct BulkUpdateBody {
    #[serde(rename = "memberIds")]
    pub member_ids: Option<Value>,
    pub updates: Option<Value>,
    pub reason: Option<String>,
}

pub async fn bulk_update_members(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<BulkUpdateBody>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let member_ids = string_array(body.member_ids.as_ref(), "memberIds")?;
    let updates = body.updates.unwrap_or(Value::Null);
    let role = updates.get("role").and_then(Value::as_str);
    let is_active = updates.get("isActive").and_then(Value::as_bool);
    if role.is_none() && is_active.is_none() {
        return Err(AppError::BadRequest(
            "updates must include at least one of role or isActive".into(),
        ));
    }
    if let Some(r) = role {
        if !GLOBAL_ROLES.contains(&r) {
            return Err(AppError::BadRequest("role must be one of: admin, agent".into()));
        }
    }

    let mut updated: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for member_id in &member_ids {
        // The caller's own account is skipped, not failed (CRD 1993).
        if *member_id == user.id {
            skipped.push(json!({ "memberId": member_id, "reason": "Cannot update your own account" }));
            continue;
        }
        let Some(member) = store::find_member(&state.db, member_id).await? else {
            failed.push(json!({ "memberId": member_id, "reason": "Member not found" }));
            continue;
        };
        apply_member_updates(&state, &member, &updates).await?;
        updated.push(member_id.clone());
    }

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "member bulk update", "agent", None,
        Some(json!({ "updated": updated, "updates": updates, "reason": body.reason })),
        None, None,
    )
    .await;

    Ok(envelope::ok(json!({
        "updated": updated,
        "failed": failed,
        "skipped": skipped,
        "updatedCount": updated.len(),
    })))
}

// ----------------------------------------------------- Batch edit & undo (CRD 1997-2009)

/// Advertised undo validity window (~10 seconds, CRD 2000).
const UNDO_ADVERTISED_SECS: i64 = 10;

fn entry_has_profile(entry: &Value) -> bool {
    entry
        .get("profile")
        .and_then(Value::as_object)
        .is_some_and(|p| ["displayName", "email", "role"].iter().any(|k| p.contains_key(*k)))
}

fn entry_team_changes(entry: &Value) -> (Vec<i64>, Vec<i64>) {
    let ids = |key: &str| -> Vec<i64> {
        entry
            .get("teamChanges")
            .and_then(|tc| tc.get(key))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default()
    };
    (ids("add"), ids("remove"))
}

async fn member_snapshot(state: &AppState, member: &MemberRow) -> Result<Value> {
    let memberships = store::memberships_of(&state.db, &member.id).await?;
    Ok(json!({
        "memberId": member.id,
        "profile": {
            "displayName": member.display_name,
            "email": member.email,
            "role": member.role,
        },
        "memberships": memberships.iter().map(store::membership_view).collect::<Vec<_>>(),
    }))
}

pub async fn batch_edit_members(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let entries = body
        .get("members")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest("members must be a non-empty array".into()))?
        .clone();
    if entries.len() > BATCH_LIMIT {
        return Err(AppError::BadRequest(format!(
            "members cannot contain more than {BATCH_LIMIT} entries"
        )));
    }
    // Upfront validation: every entry needs a member id and at least one change (CRD 2002).
    for entry in &entries {
        let member_id = entry.get("memberId").and_then(Value::as_str).unwrap_or("").trim();
        if member_id.is_empty() {
            return Err(AppError::BadRequest("Each entry requires a memberId".into()));
        }
        let (add, remove) = entry_team_changes(entry);
        if !entry_has_profile(entry) && add.is_empty() && remove.is_empty() {
            return Err(AppError::BadRequest(format!(
                "Entry for member {member_id} contains no changes"
            )));
        }
    }

    let mut results: Vec<Value> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut snapshots: Vec<Value> = Vec::new();
    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    let now = now_iso();

    for entry in &entries {
        let member_id = entry.get("memberId").and_then(Value::as_str).unwrap_or("").trim().to_string();
        // The caller's own account is skipped (CRD 2000).
        if member_id == user.id {
            skipped.push(member_id);
            continue;
        }
        let Some(member) = store::find_member(&state.db, &member_id).await? else {
            failure_count += 1;
            results.push(json!({ "memberId": member_id, "success": false, "error": "Member not found" }));
            continue;
        };
        let snapshot = member_snapshot(&state, &member).await?;

        let mut profile_updated = false;
        if entry_has_profile(entry) {
            let profile = entry.get("profile").cloned().unwrap_or(Value::Null);
            match apply_member_updates(&state, &member, &profile).await {
                Ok((_, new)) => profile_updated = new.as_object().is_some_and(|o| !o.is_empty()),
                Err(e) => {
                    failure_count += 1;
                    results.push(json!({
                        "memberId": member_id, "success": false, "error": e.to_string(),
                    }));
                    continue;
                }
            }
        }

        let (add, remove) = entry_team_changes(entry);
        let mut teams_added: Vec<i64> = Vec::new();
        let mut teams_removed: Vec<i64> = Vec::new();
        for team_id in add {
            if !team_exists(&state, team_id).await? {
                continue;
            }
            let res = sqlx::query(
                "INSERT OR IGNORE INTO team_members (agent_id, team_id, role, is_primary, joined_at)
                 VALUES (?, ?, 'member', 0, ?)",
            )
            .bind(&member_id)
            .bind(team_id)
            .bind(&now)
            .execute(&state.db)
            .await?;
            if res.rows_affected() > 0 {
                teams_added.push(team_id);
            }
        }
        for team_id in remove {
            let res = sqlx::query("DELETE FROM team_members WHERE agent_id = ? AND team_id = ?")
                .bind(&member_id)
                .bind(team_id)
                .execute(&state.db)
                .await?;
            if res.rows_affected() > 0 {
                teams_removed.push(team_id);
            }
        }
        if !teams_added.is_empty() || !teams_removed.is_empty() {
            store::promote_primary_if_needed(&state.db, &member_id).await?;
            state.team_cache.invalidate(&member_id);
        }

        success_count += 1;
        snapshots.push(snapshot);
        results.push(json!({
            "memberId": member_id,
            "success": true,
            "error": null,
            "profileUpdated": profile_updated,
            "teamsAdded": teams_added,
            "teamsRemoved": teams_removed,
        }));
    }

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "member batch edit", "agent", None,
        Some(json!({ "successCount": success_count, "failureCount": failure_count,
                     "skipped": skipped, "reason": body.get("reason") })),
        None, None,
    )
    .await;

    let mut payload = json!({
        "results": results,
        "successCount": success_count,
        "failureCount": failure_count,
        "skipped": skipped,
    });
    // An undo token is issued only when at least one edit succeeded (CRD 2001).
    if success_count > 0 {
        let token = uuid::Uuid::new_v4().to_string();
        state.batch_undo.put(&token, &user.id, json!(snapshots));
        let expires = (chrono::Utc::now() + chrono::Duration::seconds(UNDO_ADVERTISED_SECS))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        payload["undoToken"] = json!(token);
        payload["undoExpiresAt"] = json!(expires);
    }
    Ok(envelope::ok_msg(payload, "Batch edit completed"))
}

pub async fn undo_batch_edit(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let token = body.get("undoToken").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if token.is_empty() {
        return Err(AppError::BadRequest("undoToken is required".into()));
    }
    let Some((owner, snapshot)) = state.batch_undo.take(&token) else {
        return Err(AppError::BadRequest("Invalid or expired undo token".into()));
    };
    // Only the original editor may undo (CRD 2006); reinstate the token on rejection.
    if owner != user.id {
        state.batch_undo.restore(&token, owner, snapshot);
        return Err(AppError::Forbidden("Undo token belongs to a different user".into()));
    }

    let mut results: Vec<Value> = Vec::new();
    let mut restored = 0usize;
    let entries = snapshot.as_array().cloned().unwrap_or_default();
    for entry in &entries {
        let member_id = entry.get("memberId").and_then(Value::as_str).unwrap_or("").to_string();
        let profile = &entry["profile"];
        let now = now_iso();
        sqlx::query(
            "UPDATE agents SET display_name = ?, email = ?, role = ?, updated_at = ?
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(profile.get("displayName").and_then(Value::as_str).unwrap_or(""))
        .bind(profile.get("email").and_then(Value::as_str).unwrap_or(""))
        .bind(profile.get("role").and_then(Value::as_str).unwrap_or("agent"))
        .bind(&now)
        .bind(&member_id)
        .execute(&state.db)
        .await?;
        sqlx::query("DELETE FROM team_members WHERE agent_id = ?")
            .bind(&member_id)
            .execute(&state.db)
            .await?;
        for m in entry.get("memberships").and_then(Value::as_array).into_iter().flatten() {
            sqlx::query(
                "INSERT OR IGNORE INTO team_members (agent_id, team_id, role, is_primary, joined_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&member_id)
            .bind(m.get("teamId").and_then(Value::as_i64))
            .bind(m.get("roleInTeam").and_then(Value::as_str).unwrap_or("member"))
            .bind(m.get("isPrimary").and_then(Value::as_bool).unwrap_or(false) as i64)
            .bind(m.get("joinedAt").and_then(Value::as_str).unwrap_or(&now))
            .execute(&state.db)
            .await?;
        }
        state.team_cache.invalidate(&member_id);
        restored += 1;
        results.push(json!({ "memberId": member_id, "success": true }));
    }

    Ok(envelope::ok_msg(
        json!({ "restoredCount": restored, "results": results }),
        "Batch edit undone",
    ))
}

// ------------------------------------------------ Agent-team associations (CRD 2029-2074)

pub async fn agent_teams(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
) -> Result {
    // Administrators may view anyone's teams; an agent only their own (CRD 2031).
    if !user.is_admin() && user.id != agent_id {
        return Err(AppError::Forbidden("You can only view your own teams".into()));
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        team_id: i64,
        role: String,
        is_primary: i64,
        joined_at: String,
        team_name: String,
        description: Option<String>,
        team_active: i64,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT m.team_id, m.role, m.is_primary, m.joined_at,
                t.name AS team_name, t.description, t.is_active AS team_active
         FROM team_members m
         JOIN teams t ON t.id = m.team_id AND t.deleted_at IS NULL
         WHERE m.agent_id = ?
         ORDER BY m.is_primary DESC, m.team_id",
    )
    .bind(&agent_id)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(rows
        .iter()
        .map(|r| json!({
            "teamId": r.team_id,
            "teamName": r.team_name,
            "description": r.description,
            "teamIsActive": r.team_active != 0,
            "roleInTeam": r.role,
            "isPrimary": r.is_primary != 0,
            "joinedAt": r.joined_at,
            "createdAt": r.joined_at,
        }))
        .collect::<Vec<_>>()))
}

pub async fn team_members_detail(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(raw_team_id): Path<String>,
) -> Result {
    let team_id = parse_team_id(&raw_team_id)?;
    let members: Vec<MemberRow> = sqlx::query_as(&format!(
        "SELECT {} FROM agents a WHERE a.deleted_at IS NULL
         AND EXISTS (SELECT 1 FROM team_members m WHERE m.agent_id = a.id AND m.team_id = ?)
         ORDER BY a.display_name",
        store::MEMBER_COLUMNS
            .split(',')
            .map(|c| format!("a.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(team_id)
    .fetch_all(&state.db)
    .await?;
    let team_map = memberships_with_names(&state).await?;
    Ok(envelope::ok(members
        .iter()
        .map(|m| enriched_member_view(m, team_map.get(&m.id)))
        .collect::<Vec<_>>()))
}

#[derive(Deserialize)]
pub struct JoinBody {
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
    #[serde(rename = "roleInTeam")]
    pub role_in_team: Option<String>,
    #[serde(rename = "isPrimary")]
    pub is_primary: Option<bool>,
}

pub async fn join_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    body: JsonBody<JoinBody>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let team_id = body.team_id.ok_or_else(|| AppError::BadRequest("teamId is required".into()))?;
    let role = body.role_in_team.as_deref().unwrap_or("member").to_string();
    if !TEAM_ROLES.contains(&role.as_str()) {
        return Err(AppError::BadRequest(
            "roleInTeam must be one of: member, lead, supervisor".into(),
        ));
    }
    if !team_exists(&state, team_id).await? {
        return Err(AppError::NotFound("Team not found".into()));
    }
    if !agent_exists(&state, &agent_id).await? {
        return Err(AppError::NotFound("Agent not found".into()));
    }
    if store::find_membership(&state.db, &agent_id, team_id).await?.is_some() {
        return Err(AppError::Conflict("Agent is already a member of this team".into()));
    }

    let is_primary = body.is_primary.unwrap_or(false);
    if is_primary {
        // Exactly one membership stays primary (CRD 2045).
        store::clear_primary(&state.db, &agent_id).await?;
    }
    let now = now_iso();
    sqlx::query(
        "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&agent_id)
    .bind(team_id)
    .bind(&role)
    .bind(is_primary as i64)
    .bind(&now)
    .execute(&state.db)
    .await?;
    state.team_cache.invalidate(&agent_id);

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "agent join team", "team_member", Some(&agent_id),
        Some(json!({ "teamId": team_id, "roleInTeam": role, "isPrimary": is_primary })),
        None, None,
    )
    .await;
    // TODO(realtime): broadcast "member added" with team id/name, agent id/name,
    // refreshed member count and the acting user (CRD 2045, 2149).

    Ok(envelope::with_status(
        StatusCode::CREATED,
        Some(json!({
            "agentId": agent_id,
            "teamId": team_id,
            "roleInTeam": role,
            "isPrimary": is_primary,
            "joinedAt": now,
        })),
        Some("Agent added to team"),
    ))
}

#[derive(Deserialize)]
pub struct JoinMultipleBody {
    #[serde(rename = "teamIds")]
    pub team_ids: Option<Value>,
    #[serde(rename = "roleInTeam")]
    pub role_in_team: Option<String>,
}

pub async fn join_multiple(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    body: JsonBody<JoinMultipleBody>,
) -> Result {
    require_admin(&user)?;
    let body = parse_json(body)?;
    let team_ids: Vec<i64> = body
        .team_ids
        .as_ref()
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest("teamIds must be a non-empty array".into()))?
        .iter()
        .filter_map(Value::as_i64)
        .collect();
    let role = body.role_in_team.as_deref().unwrap_or("member").to_string();
    if !TEAM_ROLES.contains(&role.as_str()) {
        return Err(AppError::BadRequest(
            "roleInTeam must be one of: member, lead, supervisor".into(),
        ));
    }
    if !agent_exists(&state, &agent_id).await? {
        return Err(AppError::NotFound("Agent not found".into()));
    }

    let mut added: Vec<i64> = Vec::new();
    let mut skipped: Vec<i64> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let now = now_iso();
    for team_id in team_ids {
        if !team_exists(&state, team_id).await? {
            errors.push(json!({ "teamId": team_id, "error": "Team not found" }));
            continue;
        }
        if store::find_membership(&state.db, &agent_id, team_id).await?.is_some() {
            skipped.push(team_id);
            continue;
        }
        // New memberships are not primary (CRD 2052).
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
             VALUES (?, ?, ?, 0, ?)",
        )
        .bind(&agent_id)
        .bind(team_id)
        .bind(&role)
        .bind(&now)
        .execute(&state.db)
        .await?;
        added.push(team_id);
        // TODO(realtime): broadcast "member added" with this team's refreshed member
        // count, asynchronously (CRD 2052, 2149).
    }
    state.team_cache.invalidate(&agent_id);

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "agent join multiple teams", "team_member", Some(&agent_id),
        Some(json!({ "added": added, "skipped": skipped, "roleInTeam": role })),
        None, None,
    )
    .await;

    let message = format!("Added to {} team(s), {} skipped", added.len(), skipped.len());
    Ok(envelope::ok_msg(
        json!({ "added": added, "skipped": skipped, "errors": errors }),
        &message,
    ))
}

pub async fn leave_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((agent_id, raw_team_id)): Path<(String, String)>,
) -> Result {
    let team_id = parse_team_id(&raw_team_id)?;
    require_team_rank(&user, team_id, "lead")?;
    let team_name: String =
        sqlx::query_scalar("SELECT name FROM teams WHERE id = ? AND deleted_at IS NULL")
            .bind(team_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound("Team not found".into()))?;
    let membership = store::find_membership(&state.db, &agent_id, team_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Agent is not a member of this team".into()))?;

    sqlx::query("DELETE FROM team_members WHERE agent_id = ? AND team_id = ?")
        .bind(&agent_id)
        .bind(team_id)
        .execute(&state.db)
        .await?;
    let promoted = store::promote_primary_if_needed(&state.db, &agent_id).await?;
    state.team_cache.invalidate(&agent_id);

    // Conversations assigned to this team that the removed agent can no longer access.
    let conversation_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE team_id = ? AND deleted_at IS NULL",
    )
    .bind(team_id)
    .fetch_all(&state.db)
    .await?;

    // High-priority personal notification, persisted (and pushed in real time) (CRD 2151).
    let now = now_iso();
    sqlx::query(
        "INSERT INTO notifications (id, agent_id, type, title, content, data, created_at)
         VALUES (?, ?, 'team_removal', ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&agent_id)
    .bind(format!("Removed from team {team_name}"))
    .bind(format!("You were removed from team {team_name} by {}", user.display_name))
    .bind(
        json!({
            "teamId": team_id,
            "teamName": team_name,
            "removedBy": { "id": user.id, "name": user.display_name },
            "conversationIds": conversation_ids,
            "priority": "high",
        })
        .to_string(),
    )
    .bind(&now)
    .execute(&state.db)
    .await?;
    // TODO(realtime): push the personal notification to the removed agent and broadcast
    // a team-wide "member removed" event with refreshed member count (CRD 2059, 2150-2151).

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "agent leave team", "team_member", Some(&agent_id),
        Some(json!({
            "teamId": team_id,
            "prior": store::membership_view(&membership),
            "promotedTeamId": promoted,
        })),
        None, None,
    )
    .await;

    Ok(envelope::ok_msg(
        json!({
            "teamName": team_name,
            "affectedConversations": conversation_ids.len(),
        }),
        "Agent removed from team",
    ))
}

#[derive(Deserialize)]
pub struct MembershipRoleBody {
    #[serde(rename = "roleInTeam")]
    pub role_in_team: Option<String>,
    #[serde(rename = "isPrimary")]
    pub is_primary: Option<bool>,
}

pub async fn update_membership_role(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((agent_id, raw_team_id)): Path<(String, String)>,
    body: JsonBody<MembershipRoleBody>,
) -> Result {
    let team_id = parse_team_id(&raw_team_id)?;
    require_team_rank(&user, team_id, "lead")?;
    let body = parse_json(body)?;
    // Missing membership surfaces as a server error (CRD 2067).
    let membership = store::find_membership(&state.db, &agent_id, team_id)
        .await?
        .ok_or_else(|| AppError::Internal("Membership not found".into()))?;

    if let Some(role) = body.role_in_team.as_deref() {
        if !TEAM_ROLES.contains(&role) {
            return Err(AppError::BadRequest(
                "roleInTeam must be one of: member, lead, supervisor".into(),
            ));
        }
        sqlx::query("UPDATE team_members SET role = ? WHERE agent_id = ? AND team_id = ?")
            .bind(role)
            .bind(&agent_id)
            .bind(team_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(primary) = body.is_primary {
        if primary {
            store::clear_primary(&state.db, &agent_id).await?;
        }
        sqlx::query("UPDATE team_members SET is_primary = ? WHERE agent_id = ? AND team_id = ?")
            .bind(primary as i64)
            .bind(&agent_id)
            .bind(team_id)
            .execute(&state.db)
            .await?;
    }
    state.team_cache.invalidate(&agent_id);

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "membership role update", "team_member", Some(&agent_id),
        Some(json!({
            "teamId": team_id,
            "old": store::membership_view(&membership),
            "new": { "roleInTeam": body.role_in_team, "isPrimary": body.is_primary },
        })),
        None, None,
    )
    .await;

    let updated = store::find_membership(&state.db, &agent_id, team_id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload membership".into()))?;
    Ok(envelope::ok_msg(store::membership_view(&updated), "Membership updated"))
}

pub async fn set_primary_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((agent_id, raw_team_id)): Path<(String, String)>,
) -> Result {
    let team_id = parse_team_id(&raw_team_id)?;
    require_team_rank(&user, team_id, "lead")?;
    // Agent must already be a member; otherwise a server error (CRD 2074).
    if store::find_membership(&state.db, &agent_id, team_id).await?.is_none() {
        return Err(AppError::Internal("Agent is not a member of this team".into()));
    }
    store::clear_primary(&state.db, &agent_id).await?;
    sqlx::query("UPDATE team_members SET is_primary = 1 WHERE agent_id = ? AND team_id = ?")
        .bind(&agent_id)
        .bind(team_id)
        .execute(&state.db)
        .await?;
    state.team_cache.invalidate(&agent_id);

    log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "set primary team", "team_member", Some(&agent_id),
        Some(json!({ "teamId": team_id })),
        None, None,
    )
    .await;
    Ok(envelope::message_only("Primary team updated"))
}

// ------------------------------------------------------------ QR family (CRD 2078-2129)

pub async fn generate_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "supervisor")?;
    if !team_exists(&state, id).await? {
        return Err(AppError::NotFound("Team not found".into()));
    }
    // Missing/invalid body is tolerated (CRD 2079).
    let body = body.map(|Json(b)| b).unwrap_or(Value::Null);
    let campaign = body.get("campaignName").and_then(Value::as_str);
    let description = body.get("description").and_then(Value::as_str);
    let expires_at = body.get("expiresAt").and_then(Value::as_str);
    let max_uses = body.get("maxUses").and_then(Value::as_i64);

    let qr = store::create_join_qr(
        &state.db, &state.config, id, campaign, description, expires_at, max_uses,
    )
    .await?;
    Ok(envelope::with_status(
        StatusCode::CREATED,
        Some(store::qr_view(&qr)),
        Some("QR code generated"),
    ))
}

pub async fn list_qr_codes(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let rows: Vec<QrRow> = sqlx::query_as(
        "SELECT id, team_id, token, url, image_url, campaign, description, scan_count,
                max_scans, is_active, expires_at, created_at
         FROM qr_codes WHERE team_id = ? ORDER BY created_at DESC, id",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(rows.iter().map(store::qr_view).collect::<Vec<_>>()))
}

/// Cached image (team record) or the latest active QR record; optionally caches back.
async fn resolve_team_qr(
    state: &Arc<AppState>,
    team_id: i64,
) -> Result<Option<(String, Option<String>, bool)>> {
    let cached: Option<Option<String>> =
        sqlx::query_scalar("SELECT qr_code_image FROM teams WHERE id = ? AND deleted_at IS NULL")
            .bind(team_id)
            .fetch_optional(&state.db)
            .await?;
    let cached = cached.ok_or_else(|| AppError::NotFound("Team not found".into()))?;

    let latest: Option<QrRow> = sqlx::query_as(
        "SELECT id, team_id, token, url, image_url, campaign, description, scan_count,
                max_scans, is_active, expires_at, created_at
         FROM qr_codes WHERE team_id = ? AND is_active = 1
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(team_id)
    .fetch_optional(&state.db)
    .await?;

    if let Some(image) = cached {
        return Ok(Some((image, latest.and_then(|q| q.url), true)));
    }
    let Some(qr) = latest else { return Ok(None) };
    let Some(image) = qr.image_url.clone() else { return Ok(None) };
    // Asynchronously cache the image back onto the team record (CRD 2092, 2098).
    let db = state.db.clone();
    let img = image.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE teams SET qr_code_image = ? WHERE id = ?")
            .bind(img)
            .bind(team_id)
            .execute(&db)
            .await;
    });
    Ok(Some((image, qr.url, false)))
}

pub async fn latest_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let Some((image, join_url, from_cache)) = resolve_team_qr(&state, id).await? else {
        return Err(AppError::NotFound("No QR code found for this team".into()));
    };
    Ok(envelope::ok(json!({
        "qrCodeImage": image,
        "joinUrl": join_url,
        "fromCache": from_cache,
    })))
}

pub async fn fast_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let Some((image, join_url, from_cache)) = resolve_team_qr(&state, id).await? else {
        return Err(AppError::NotFound("No QR code found for this team".into()));
    };
    Ok(envelope::ok(json!({
        "qrCodeImage": image,
        "joinUrl": join_url,
        "source": if from_cache { "cache" } else { "database" },
        "performance": if from_cache { "fast" } else { "fallback" },
    })))
}

pub async fn deactivate_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((raw_id, raw_qr_id)): Path<(String, String)>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "supervisor")?;
    let qr_id = raw_qr_id.trim().to_string();
    if qr_id.is_empty() {
        return Err(AppError::BadRequest("QR code id is required".into()));
    }
    let res = sqlx::query(
        "UPDATE qr_codes SET is_active = 0, updated_at = ? WHERE id = ? AND team_id = ?",
    )
    .bind(now_iso())
    .bind(&qr_id)
    .bind(id)
    .execute(&state.db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("QR code not found".into()));
    }
    Ok(envelope::message_only("QR code deactivated"))
}

fn liff_view(l: &store::LiffRow) -> Value {
    json!({
        "id": l.id,
        "teamId": l.team_id,
        "url": l.url,
        "imageUrl": l.image_url,
        "scanCount": l.scan_count,
        "isActive": l.is_active != 0,
        "createdAt": l.created_at,
        "updatedAt": l.updated_at,
    })
}

pub async fn get_liff_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let liff = store::find_liff(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("No LIFF QR code found for this team".into()))?;
    Ok(envelope::ok(liff_view(&liff)))
}

pub async fn generate_liff_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    require_admin(&user)?;
    let id = parse_team_id(&raw_id)?;
    if !team_exists(&state, id).await? {
        return Err(AppError::NotFound("Team not found".into()));
    }
    let liff = store::upsert_liff(&state.db, id).await?;
    Ok(envelope::ok_msg(liff_view(&liff), "LIFF QR code generated"))
}

pub async fn liff_qr_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let liff = store::find_liff(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("No LIFF QR code found for this team".into()))?;
    let assignments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM customer_team_assignments WHERE liff_link_id = ?",
    )
    .bind(&liff.id)
    .fetch_one(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "scanCount": liff.scan_count,
        "customerAssignments": assignments,
        "createdAt": liff.created_at,
        "lastScanAt": null,
        "isActive": liff.is_active != 0,
    })))
}

/// Unauthenticated diagnostics endpoint; nothing is persisted (CRD 2126-2129).
pub async fn qr_code_test(Path(raw_id): Path<String>) -> Result {
    let id = parse_team_id(&raw_id)?;
    let token = uuid::Uuid::new_v4().to_string();
    Ok(envelope::ok(json!({
        "test": true,
        "teamId": id,
        "token": token,
        "imageUrl": store::qr_image_url(&token),
        "generatedAt": now_iso(),
    })))
}

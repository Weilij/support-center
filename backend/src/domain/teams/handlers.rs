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
use std::sync::Arc;

use crate::db::now_iso;
use crate::domain::auth::store::log_activity;
use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::{team_role_level, AuthUser};
use crate::state::AppState;

use super::store::{self, TeamWithCounts};

mod agent_teams;
mod member_accounts;
mod qr;
mod team_scoped_members;
pub use agent_teams::*;
pub use member_accounts::*;
pub use qr::*;
pub use team_scoped_members::*;

pub(super) type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

pub(super) const BATCH_LIMIT: usize = 50;
pub(super) const TEAM_ROLES: [&str; 3] = ["member", "lead", "supervisor"];
pub(super) const GLOBAL_ROLES: [&str; 2] = ["admin", "agent"];

// ----------------------------------------------------------------------------- helpers

pub(super) fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b)
        .map_err(|_| AppError::BadRequest("Invalid JSON".into()))
}

/// Refreshed member count for realtime member-change events (CRD 2149).
pub(super) async fn live_member_count(state: &AppState, team_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM team_members WHERE team_id = $1")
        .bind(team_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0)
}

/// Realtime `member_added` event to administrators and the affected team
/// (CRD 2149, 3460); best-effort by construction.
pub(super) async fn emit_member_added(
    state: &AppState,
    team_id: i64,
    agent_id: &str,
    actor: &AuthUser,
) {
    let count = live_member_count(state, team_id).await;
    state.realtime.to_teams_and_admins(
        &[team_id],
        "member_added",
        json!({
            "teamId": team_id,
            "agentId": agent_id,
            "memberCount": count,
            "addedBy": { "id": actor.id, "name": actor.display_name },
            "timestamp": now_iso(),
        }),
    );
}

/// Path id must be a positive integer (CRD 1831, 1835: non-integer id -> 400).
pub(super) fn parse_team_id(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| AppError::BadRequest("Invalid team id".into()))
}

pub(super) fn require_admin(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator role required".into()))
    }
}

/// Team-access check (CRD 1809): admins always pass; agents only for their own teams.
/// The denial message names the user's team and the requested team (CRD 1835).
pub(super) fn require_team_access(user: &AuthUser, team_id: i64) -> Result<()> {
    if user.can_access_team(team_id) {
        return Ok(());
    }
    Err(AppError::Forbidden(format!(
        "Access denied: you belong to team {} but requested team {team_id}",
        user.primary_team_id
            .map_or_else(|| "none".to_string(), |t| t.to_string()),
    )))
}

/// In-team rank gate (CRD 1808): requires `required` rank or higher in this specific
/// team; administrators bypass. Denials carry required role, current role, and team id.
pub(super) fn require_team_rank(user: &AuthUser, team_id: i64, required: &str) -> Result<()> {
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
pub(super) fn string_array(v: Option<&Value>, field: &str) -> Result<Vec<String>> {
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

pub(super) async fn team_exists(state: &AppState, id: i64) -> Result<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT id FROM teams WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    Ok(found.is_some())
}

pub(super) async fn agent_exists(state: &AppState, id: &str) -> Result<bool> {
    let found: Option<String> =
        sqlx::query_scalar("SELECT id FROM agents WHERE id = $1 AND deleted_at IS NULL")
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
    // Scoping: a non-admin sees EVERY team they belong to (not just their primary),
    // so e.g. an agent who is supervisor of a non-primary team can still open and
    // manage it. Soft-deleted teams are skipped (team_with_counts filters them).
    // Listing/pagination params are ignored for this path (CRD 1826, revised).
    if !user.is_admin() {
        let mut items: Vec<Value> = Vec::new();
        for membership in &user.teams {
            if let Some(team) = store::team_with_counts(&state.db, membership.team_id).await? {
                items.push(store::team_view(&team));
            }
        }
        return Ok(envelope::ok(items));
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

    let count_sql = format!("SELECT COUNT(*) FROM teams t WHERE t.deleted_at IS NULL {filter}");
    let count_sql = crate::db::pg_params(&count_sql);
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    if let Some(p) = &pattern {
        count_q = count_q.bind(p.clone()).bind(p.clone());
    }
    let total = count_q.fetch_one(&state.db).await?;

    let list_sql = store::team_select(
        &filter,
        "ORDER BY t.created_at DESC, t.id DESC LIMIT ? OFFSET ?",
    );
    let list_sql = crate::db::pg_params(&list_sql);
    let mut list_q = sqlx::query_as::<_, TeamWithCounts>(&list_sql);
    if let Some(p) = &pattern {
        list_q = list_q.bind(p.clone()).bind(p.clone());
    }
    let rows = list_q
        .bind(limit)
        .bind((page - 1) * limit)
        .fetch_all(&state.db)
        .await?;
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
        let used: Option<i64> = sqlx::query_scalar("SELECT id FROM teams WHERE qr_code = $1")
            .bind(qr)
            .fetch_optional(&state.db)
            .await?;
        if used.is_some() {
            return Err(AppError::Conflict(
                "A team with this QR code already exists".into(),
            ));
        }
    }

    let now = now_iso();
    let is_active = body.is_active.unwrap_or(true);
    let team_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO teams (name, description, is_active, qr_code, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(&name)
    .bind(&body.description)
    .bind(is_active as i64)
    .bind(&body.qr_code)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    // Reversible create audit entry (CRD 1840).
    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "team create",
        "team",
        Some(&team_id.to_string()),
        Some(json!({
            "reversible": true,
            "old": null,
            "new": { "name": name, "description": body.description, "isActive": is_active },
        })),
        None,
        None,
    )
    .await;

    // QR artifacts are generated after persistence; failures do not fail the
    // request — the team is returned without the failed artifact (CRD 1840).
    let join_qr = store::create_join_qr(&state.db, &state.config, team_id, None, None, None, None)
        .await
        .ok();
    if let Some(qr) = &join_qr {
        if let Err(error) = sqlx::query("UPDATE teams SET qr_code_image = $1 WHERE id = $2")
            .bind(&qr.image_url)
            .bind(team_id)
            .execute(&state.db)
            .await
        {
            tracing::warn!(error = %error, team_id, "team QR image cache update failed");
        }
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
    Ok(envelope::with_status(
        StatusCode::CREATED,
        Some(view),
        Some("Team created successfully"),
    ))
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
    if let Some(name) = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        old.insert("name".into(), json!(current.name));
        new.insert("name".into(), json!(name));
        sqlx::query("UPDATE teams SET name = $1, updated_at = $2 WHERE id = $3")
            .bind(name)
            .bind(&now)
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    if let Some(description) = &body.description {
        old.insert("description".into(), json!(current.description));
        new.insert("description".into(), json!(description));
        sqlx::query("UPDATE teams SET description = $1, updated_at = $2 WHERE id = $3")
            .bind(description)
            .bind(&now)
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    if let Some(active) = body.is_active {
        old.insert("isActive".into(), json!(current.is_active != 0));
        new.insert("isActive".into(), json!(active));
        sqlx::query("UPDATE teams SET is_active = $1, updated_at = $2 WHERE id = $3")
            .bind(active as i64)
            .bind(&now)
            .bind(id)
            .execute(&state.db)
            .await?;
    }

    // Reversible update audit entry capturing before/after state (CRD 1849).
    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "team update",
        "team",
        Some(&id.to_string()),
        Some(json!({ "reversible": true, "old": old, "new": new })),
        None,
        None,
    )
    .await;

    let updated = store::team_with_counts(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload team after update".into()))?;
    // Realtime: team-information update to administrators and the affected
    // team (CRD 2152, 3460); best-effort by construction.
    state.realtime.to_teams_and_admins(
        &[id],
        "team_updated",
        json!({
            "teamId": id,
            "team": store::team_view(&updated),
            "updatedBy": { "id": user.id, "name": user.display_name },
            "timestamp": now_iso(),
        }),
    );
    Ok(envelope::ok_msg(
        store::team_view(&updated),
        "Team updated successfully",
    ))
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
    sqlx::query("UPDATE teams SET deleted_at = $1, updated_at = $2 WHERE id = $3")
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(&state.db)
        .await?;
    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "team delete",
        "team",
        Some(&id.to_string()),
        Some(json!({
            "reversible": true,
            "old": { "name": current.name, "deletedAt": null },
            "new": { "deletedAt": now },
        })),
        None,
        None,
    )
    .await;
    Ok(envelope::message_only("Team deleted successfully"))
}

// ---------------------------------------------------------- Search teams (CRD 1862-1867)

pub async fn search_teams(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_query): Path<String>,
) -> Result {
    let query = raw_query.trim().to_string();
    if query.is_empty() {
        return Err(AppError::BadRequest("Search query is required".into()));
    }
    let pattern = format!("%{}%", query.to_lowercase());
    let visible_team_ids: Vec<i64> = user.teams.iter().map(|t| t.team_id).collect();
    if !user.is_admin() && visible_team_ids.is_empty() {
        return Ok(envelope::ok(Vec::<Value>::new()));
    }

    let scope_filter = if user.is_admin() {
        String::new()
    } else {
        " AND t.id = ANY(?)".to_string()
    };
    let sql = store::team_select(
        &format!(
            "AND t.is_active = 1
             AND (LOWER(t.name) LIKE ? OR LOWER(COALESCE(t.description,'')) LIKE ?){scope_filter}"
        ),
        "ORDER BY t.created_at DESC, t.id DESC LIMIT 20",
    );
    let sql = crate::db::pg_params(&sql);
    let mut query = sqlx::query_as::<_, TeamWithCounts>(&sql)
        .bind(&pattern)
        .bind(&pattern);
    if !user.is_admin() {
        query = query.bind(visible_team_ids);
    }
    let rows = query.fetch_all(&state.db).await?;
    Ok(envelope::ok(
        rows.iter().map(store::team_view).collect::<Vec<_>>(),
    ))
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
         WHERE team_id = $1 AND deleted_at IS NULL AND created_at >= $2 AND created_at <= $3",
    )
    .bind(team.id)
    .bind(from)
    .bind(to)
    .fetch_one(&state.db)
    .await?;
    let messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages m
         JOIN conversations c ON c.id = m.conversation_id AND c.deleted_at IS NULL
         WHERE c.team_id = $1 AND m.deleted_at IS NULL
           AND m.created_at >= $2 AND m.created_at <= $3",
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
        stats["members"] = team_scoped_members::team_member_list(state, team.id).await?;
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
    let teams: Vec<TeamWithCounts> = sqlx::query_as(&crate::db::pg_params(&sql))
        .fetch_all(&state.db)
        .await?;
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
    let from = body
        .from_team_id
        .ok_or_else(|| AppError::BadRequest("fromTeamId is required".into()))?;
    let to = body
        .to_team_id
        .ok_or_else(|| AppError::BadRequest("toTeamId is required".into()))?;
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
        sqlx::query("DELETE FROM team_members WHERE agent_id = $1 AND team_id = $2")
            .bind(agent_id)
            .bind(from)
            .execute(&state.db)
            .await?;
        // Primary-team flag and in-team role are preserved across the move (CRD 1885).
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT(agent_id, team_id)
             DO UPDATE SET is_primary = GREATEST(team_members.is_primary, EXCLUDED.is_primary)",
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
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "team transfer",
        "team",
        Some(&to.to_string()),
        Some(json!({
            "fromTeamId": from, "toTeamId": to,
            "transferred": transferred, "reason": body.reason,
        })),
        None,
        None,
    )
    .await;

    // Overall success flag is true only when there were no failures (CRD 1886).
    let success = failed.is_empty();
    Ok(envelope::flagged(
        success,
        json!({ "transferred": transferred, "failed": failed }),
        Some(if success {
            "All agents transferred"
        } else {
            "Some transfers failed"
        }),
    ))
}

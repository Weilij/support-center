use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::domain::auth::store::log_activity;
use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::teams::store::{self, MembershipRow};

use super::{
    agent_exists, emit_member_added, parse_json, parse_team_id, require_admin, require_team_access,
    require_team_rank, string_array, team_exists, JsonBody, GLOBAL_ROLES, TEAM_ROLES,
};

pub(super) async fn team_member_list(state: &AppState, team_id: i64) -> Result<Value> {
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
         WHERE m.team_id = $1
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
        Some(m) => m,
        None => {
            let is_primary = store::memberships_of(&state.db, &agent_id)
                .await?
                .is_empty();
            let now = now_iso();
            sqlx::query(
                "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
                 VALUES ($1, $2, 'member', $3, $4)",
            )
            .bind(&agent_id)
            .bind(id)
            .bind(is_primary as i64)
            .bind(&now)
            .execute(&state.db)
            .await?;
            state.team_cache.invalidate(&agent_id);
            MembershipRow {
                agent_id: agent_id.clone(),
                team_id: id,
                role: "member".into(),
                is_primary: is_primary as i64,
                joined_at: now,
            }
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
        if store::find_membership(&state.db, agent_id, id)
            .await?
            .is_some()
        {
            skipped.push(agent_id.clone());
            continue;
        }
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
             VALUES ($1, $2, $3, 0, $4)",
        )
        .bind(agent_id)
        .bind(id)
        .bind(&role)
        .bind(&now)
        .execute(&state.db)
        .await?;
        state.team_cache.invalidate(agent_id);
        added.push(agent_id.clone());
        emit_member_added(&state, id, agent_id, &user).await;
    }

    {
        let db = state.db.clone();
        let (uid, uname, urole) = (
            user.id.clone(),
            user.display_name.clone(),
            user.role.clone(),
        );
        let detail =
            json!({ "teamId": id, "added": added, "skipped": skipped, "roleInTeam": role });
        tokio::spawn(async move {
            log_activity(
                &db,
                &uid,
                &uname,
                &urole,
                "team batch add members",
                "team",
                Some(&id.to_string()),
                Some(detail),
                None,
                None,
            )
            .await;
        });
    }

    let status = if added.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
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
    let _ = parse_team_id(&raw_id)?;
    require_admin(&user)?;
    let agent_id = raw_agent.trim().to_string();
    if agent_id.is_empty() {
        return Err(AppError::BadRequest("agentId is required".into()));
    }
    let body = parse_json(body)?;
    let member = store::find_member(&state.db, &agent_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    let now = now_iso();
    if let Some(role) = body.role.as_deref() {
        if !GLOBAL_ROLES.contains(&role) {
            return Err(AppError::BadRequest(
                "role must be one of: admin, agent".into(),
            ));
        }
        sqlx::query("UPDATE agents SET role = $1, updated_at = $2 WHERE id = $3")
            .bind(role)
            .bind(&now)
            .bind(&agent_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(active) = body.is_active {
        sqlx::query("UPDATE agents SET is_active = $1, updated_at = $2 WHERE id = $3")
            .bind(active as i64)
            .bind(&now)
            .bind(&agent_id)
            .execute(&state.db)
            .await?;
    }
    let updated = store::find_member(&state.db, &agent_id)
        .await?
        .unwrap_or(member);
    Ok(envelope::ok_msg(
        store::member_view(&updated),
        "Member updated",
    ))
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

    let Some(membership) = store::find_membership(&state.db, &agent_id, id).await? else {
        return Ok(envelope::flagged(
            false,
            json!({ "removed": false }),
            Some("Agent is not a member of this team"),
        ));
    };

    sqlx::query("DELETE FROM team_members WHERE agent_id = $1 AND team_id = $2")
        .bind(&agent_id)
        .bind(id)
        .execute(&state.db)
        .await?;
    let promoted = store::promote_primary_if_needed(&state.db, &agent_id).await?;
    state.team_cache.invalidate(&agent_id);

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "team remove member",
        "team_member",
        Some(&agent_id),
        Some(json!({
            "reversible": true,
            "old": store::membership_view(&membership),
            "new": null,
            "promotedTeamId": promoted,
        })),
        None,
        None,
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
        if store::find_membership(&state.db, agent_id, id)
            .await?
            .is_none()
        {
            failed.push(
                json!({ "agentId": agent_id, "reason": "Agent is not a member of this team" }),
            );
            continue;
        }
        sqlx::query("DELETE FROM team_members WHERE agent_id = $1 AND team_id = $2")
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

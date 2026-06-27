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

use crate::domain::teams::store::{self, MemberRow};

use super::{
    agent_exists, emit_member_added, enriched_member_view, live_member_count,
    memberships_with_names, parse_json, parse_team_id, require_admin, require_team_access,
    require_team_rank, team_exists, JsonBody, TEAM_ROLES,
};

pub async fn agent_teams(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
) -> Result {
    if !user.is_admin() && user.id != agent_id {
        return Err(AppError::Forbidden(
            "You can only view your own teams".into(),
        ));
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
         WHERE m.agent_id = $1
         ORDER BY m.is_primary DESC, m.team_id",
    )
    .bind(&agent_id)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(
        rows.iter()
            .map(|r| {
                json!({
                    "teamId": r.team_id,
                    "teamName": r.team_name,
                    "description": r.description,
                    "teamIsActive": r.team_active != 0,
                    "roleInTeam": r.role,
                    "isPrimary": r.is_primary != 0,
                    "joinedAt": r.joined_at,
                    "createdAt": r.joined_at,
                })
            })
            .collect::<Vec<_>>(),
    ))
}

pub async fn team_members_detail(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_team_id): Path<String>,
) -> Result {
    let team_id = parse_team_id(&raw_team_id)?;
    require_team_access(&user, team_id)?;
    let members: Vec<MemberRow> = sqlx::query_as(&crate::db::pg_params(&format!(
        "SELECT {} FROM agents a WHERE a.deleted_at IS NULL
         AND EXISTS (SELECT 1 FROM team_members m WHERE m.agent_id = a.id AND m.team_id = $1)
         ORDER BY a.display_name",
        store::MEMBER_COLUMNS
            .split(',')
            .map(|c| format!("a.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    )))
    .bind(team_id)
    .fetch_all(&state.db)
    .await?;
    let team_map = memberships_with_names(&state).await?;
    Ok(envelope::ok(
        members
            .iter()
            .map(|m| enriched_member_view(m, team_map.get(&m.id)))
            .collect::<Vec<_>>(),
    ))
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
    let team_id = body
        .team_id
        .ok_or_else(|| AppError::BadRequest("teamId is required".into()))?;
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
    if store::find_membership(&state.db, &agent_id, team_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(
            "Agent is already a member of this team".into(),
        ));
    }

    let is_primary = body.is_primary.unwrap_or(false);
    if is_primary {
        store::clear_primary(&state.db, &agent_id).await?;
    }
    let now = now_iso();
    sqlx::query(
        "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
         VALUES ($1, $2, $3, $4, $5)",
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
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "agent join team",
        "team_member",
        Some(&agent_id),
        Some(json!({ "teamId": team_id, "roleInTeam": role, "isPrimary": is_primary })),
        None,
        None,
    )
    .await;
    emit_member_added(&state, team_id, &agent_id, &user).await;

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
        if store::find_membership(&state.db, &agent_id, team_id)
            .await?
            .is_some()
        {
            skipped.push(team_id);
            continue;
        }
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
             VALUES ($1, $2, $3, 0, $4)",
        )
        .bind(&agent_id)
        .bind(team_id)
        .bind(&role)
        .bind(&now)
        .execute(&state.db)
        .await?;
        added.push(team_id);
        emit_member_added(&state, team_id, &agent_id, &user).await;
    }
    state.team_cache.invalidate(&agent_id);

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "agent join multiple teams",
        "team_member",
        Some(&agent_id),
        Some(json!({ "added": added, "skipped": skipped, "roleInTeam": role })),
        None,
        None,
    )
    .await;

    let message = format!(
        "Added to {} team(s), {} skipped",
        added.len(),
        skipped.len()
    );
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
        sqlx::query_scalar("SELECT name FROM teams WHERE id = $1 AND deleted_at IS NULL")
            .bind(team_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound("Team not found".into()))?;
    let membership = store::find_membership(&state.db, &agent_id, team_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Agent is not a member of this team".into()))?;

    sqlx::query("DELETE FROM team_members WHERE agent_id = $1 AND team_id = $2")
        .bind(&agent_id)
        .bind(team_id)
        .execute(&state.db)
        .await?;
    let promoted = store::promote_primary_if_needed(&state.db, &agent_id).await?;
    state.team_cache.invalidate(&agent_id);

    let conversation_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE team_id = $1 AND deleted_at IS NULL",
    )
    .bind(team_id)
    .fetch_all(&state.db)
    .await?;

    let now = now_iso();
    sqlx::query(
        "INSERT INTO notifications (id, agent_id, type, title, content, data, created_at)
         VALUES ($1, $2, 'team_removal', $3, $4, $5, $6)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&agent_id)
    .bind(format!("Removed from team {team_name}"))
    .bind(format!(
        "You were removed from team {team_name} by {}",
        user.display_name
    ))
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
    state.realtime.to_user(
        &agent_id,
        "notification",
        json!({
            "type": "team_removal",
            "title": format!("Removed from team {team_name}"),
            "content": format!(
                "You were removed from team {team_name} by {}",
                user.display_name
            ),
            "teamId": team_id,
            "teamName": team_name,
            "removedBy": { "id": user.id, "name": user.display_name },
            "priority": "high",
            "timestamp": now,
        }),
    );
    let remaining = live_member_count(&state, team_id).await;
    state.realtime.to_teams_and_admins(
        &[team_id],
        "member_removed",
        json!({
            "teamId": team_id,
            "teamName": team_name,
            "agentId": agent_id,
            "memberCount": remaining,
            "removedBy": { "id": user.id, "name": user.display_name },
            "timestamp": now,
        }),
    );

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "agent leave team",
        "team_member",
        Some(&agent_id),
        Some(json!({
            "teamId": team_id,
            "prior": store::membership_view(&membership),
            "promotedTeamId": promoted,
        })),
        None,
        None,
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
    let membership = store::find_membership(&state.db, &agent_id, team_id)
        .await?
        .ok_or_else(|| AppError::Internal("Membership not found".into()))?;

    if let Some(role) = body.role_in_team.as_deref() {
        if !TEAM_ROLES.contains(&role) {
            return Err(AppError::BadRequest(
                "roleInTeam must be one of: member, lead, supervisor".into(),
            ));
        }
        sqlx::query("UPDATE team_members SET role = $1 WHERE agent_id = $2 AND team_id = $3")
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
        sqlx::query("UPDATE team_members SET is_primary = $1 WHERE agent_id = $2 AND team_id = $3")
            .bind(primary as i64)
            .bind(&agent_id)
            .bind(team_id)
            .execute(&state.db)
            .await?;
    }
    state.team_cache.invalidate(&agent_id);

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "membership role update",
        "team_member",
        Some(&agent_id),
        Some(json!({
            "teamId": team_id,
            "old": store::membership_view(&membership),
            "new": { "roleInTeam": body.role_in_team, "isPrimary": body.is_primary },
        })),
        None,
        None,
    )
    .await;

    let updated = store::find_membership(&state.db, &agent_id, team_id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload membership".into()))?;
    Ok(envelope::ok_msg(
        store::membership_view(&updated),
        "Membership updated",
    ))
}

pub async fn set_primary_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((agent_id, raw_team_id)): Path<(String, String)>,
) -> Result {
    let team_id = parse_team_id(&raw_team_id)?;
    require_team_rank(&user, team_id, "lead")?;
    if store::find_membership(&state.db, &agent_id, team_id)
        .await?
        .is_none()
    {
        return Err(AppError::Internal(
            "Agent is not a member of this team".into(),
        ));
    }
    store::clear_primary(&state.db, &agent_id).await?;
    sqlx::query("UPDATE team_members SET is_primary = 1 WHERE agent_id = $1 AND team_id = $2")
        .bind(&agent_id)
        .bind(team_id)
        .execute(&state.db)
        .await?;
    state.team_cache.invalidate(&agent_id);

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "set primary team",
        "team_member",
        Some(&agent_id),
        Some(json!({ "teamId": team_id })),
        None,
        None,
    )
    .await;
    Ok(envelope::message_only("Primary team updated"))
}

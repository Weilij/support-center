use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::now_iso;
use crate::domain::auth::store::{
    find_active_agent_by_email, find_deleted_agent_by_email, hash_password, log_activity,
};
use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::teams::store::{self, MemberRow};

use super::{
    parse_json, require_admin, string_array, team_exists, JsonBody, BATCH_LIMIT, GLOBAL_ROLES,
};

#[derive(sqlx::FromRow)]
pub(super) struct MemberTeamRow {
    agent_id: String,
    team_id: i64,
    team_name: String,
    role: String,
    is_primary: i64,
    joined_at: String,
}

pub(super) async fn memberships_with_names(
    state: &AppState,
) -> Result<HashMap<String, Vec<MemberTeamRow>>> {
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

pub(super) fn enriched_member_view(m: &MemberRow, teams: Option<&Vec<MemberTeamRow>>) -> Value {
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
    let members: Vec<MemberRow> = sqlx::query_as(&crate::db::pg_params(&format!(
        "SELECT {} FROM agents WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC",
        store::MEMBER_COLUMNS
    )))
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
         FROM agents WHERE email = $1
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
         WHERE m.agent_id = $1 AND m.is_primary = 1 LIMIT 1",
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
    let display_name = body
        .display_name
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    if email.is_empty() || password.is_empty() || display_name.is_empty() {
        return Err(AppError::BadRequest(
            "email, password and displayName are required".into(),
        ));
    }
    let role = body.role.as_deref().unwrap_or("agent").to_string();
    if !GLOBAL_ROLES.contains(&role.as_str()) {
        return Err(AppError::BadRequest(
            "role must be one of: admin, agent".into(),
        ));
    }
    if find_active_agent_by_email(&state.db, &email)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(
            "A member with this email already exists".into(),
        ));
    }

    let hash = hash_password(&password)
        .map_err(|e| AppError::Internal(format!("password hashing failed: {e}")))?;
    let now = now_iso();
    let is_active = body.is_active.unwrap_or(true);

    let member_id = if let Some(old) = find_deleted_agent_by_email(&state.db, &email).await? {
        sqlx::query("DELETE FROM team_members WHERE agent_id = $1")
            .bind(&old.id)
            .execute(&state.db)
            .await?;
        sqlx::query(
            "UPDATE agents SET password_hash = $1, display_name = $2, role = $3, is_active = $4,
             password_policy = 'changeable', deleted_at = NULL, updated_at = $5 WHERE id = $6",
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
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
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
             VALUES ($1, $2, 'member', 1, $3)",
        )
        .bind(&member_id)
        .bind(team_id)
        .bind(&now)
        .execute(&state.db)
        .await?;
    }
    state.team_cache.invalidate(&member_id);

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "member create",
        "agent",
        Some(&member_id),
        Some(json!({ "email": email, "displayName": display_name, "role": role, "teamId": body.team_id })),
        None,
        None,
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
        return Err(AppError::Forbidden(
            "You cannot change your own status".into(),
        ));
    }
    let member = store::find_member(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    sqlx::query("UPDATE agents SET is_active = $1, updated_at = $2 WHERE id = $3")
        .bind(is_active as i64)
        .bind(now_iso())
        .bind(&member_id)
        .execute(&state.db)
        .await?;

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "member status update",
        "agent",
        Some(&member_id),
        Some(json!({
            "old": { "isActive": member.is_active != 0 },
            "new": { "isActive": is_active },
            "reason": body.get("reason"),
        })),
        None,
        None,
    )
    .await;

    let updated = store::find_member(&state.db, &member_id)
        .await?
        .unwrap_or(member);
    Ok(envelope::ok_msg(
        store::member_view(&updated),
        "Member status updated",
    ))
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
        return Err(AppError::BadRequest(
            "role must be one of: admin, agent".into(),
        ));
    }
    if member_id == user.id {
        return Err(AppError::Forbidden(
            "You cannot change your own role".into(),
        ));
    }
    let member = store::find_member(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    sqlx::query("UPDATE agents SET role = $1, updated_at = $2 WHERE id = $3")
        .bind(role)
        .bind(now_iso())
        .bind(&member_id)
        .execute(&state.db)
        .await?;

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "member role update",
        "agent",
        Some(&member_id),
        Some(json!({
            "old": { "role": member.role },
            "new": { "role": role },
            "reason": body.get("reason"),
        })),
        None,
        None,
    )
    .await;

    let updated = store::find_member(&state.db, &member_id)
        .await?
        .unwrap_or(member);
    Ok(envelope::ok_msg(
        store::member_view(&updated),
        "Member role updated",
    ))
}

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
            if find_active_agent_by_email(&state.db, &email)
                .await?
                .is_some()
            {
                return Err(AppError::Conflict(
                    "Email already in use by another member".into(),
                ));
            }
            sqlx::query("UPDATE agents SET email = $1, updated_at = $2 WHERE id = $3")
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
            sqlx::query("UPDATE agents SET display_name = $1, updated_at = $2 WHERE id = $3")
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
            return Err(AppError::BadRequest(
                "role must be one of: admin, agent".into(),
            ));
        }
        if role != member.role {
            sqlx::query("UPDATE agents SET role = $1, updated_at = $2 WHERE id = $3")
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
            sqlx::query("UPDATE agents SET is_active = $1, updated_at = $2 WHERE id = $3")
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

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "member update",
        "agent",
        Some(&member_id),
        Some(json!({ "old": old, "new": new })),
        None,
        None,
    )
    .await;

    let updated = store::find_member(&state.db, &member_id)
        .await?
        .unwrap_or(member);
    Ok(envelope::ok_msg(
        store::member_view(&updated),
        "Member updated",
    ))
}

pub async fn delete_member_account(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(member_id): Path<String>,
) -> Result {
    require_admin(&user)?;
    if member_id == user.id {
        return Err(AppError::Forbidden(
            "You cannot delete your own account".into(),
        ));
    }
    let member = store::find_member(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    store::purge_member(&state.db, &member_id).await?;
    state.team_cache.invalidate(&member_id);

    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "member delete",
        "agent",
        Some(&member_id),
        Some(
            json!({ "email": member.email, "displayName": member.display_name, "permanent": true }),
        ),
        None,
        None,
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
    if member_ids.contains(&user.id) {
        return Err(AppError::Forbidden(
            "You cannot delete your own account".into(),
        ));
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
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "member bulk delete",
        "agent",
        None,
        Some(json!({ "deleted": deleted, "reason": body.reason, "permanent": true })),
        None,
        None,
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
            return Err(AppError::BadRequest(
                "role must be one of: admin, agent".into(),
            ));
        }
    }

    let mut updated: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for member_id in &member_ids {
        if *member_id == user.id {
            skipped
                .push(json!({ "memberId": member_id, "reason": "Cannot update your own account" }));
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
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "member bulk update",
        "agent",
        None,
        Some(json!({ "updated": updated, "updates": updates, "reason": body.reason })),
        None,
        None,
    )
    .await;

    Ok(envelope::ok(json!({
        "updated": updated,
        "failed": failed,
        "skipped": skipped,
        "updatedCount": updated.len(),
    })))
}

const UNDO_ADVERTISED_SECS: i64 = 10;

fn entry_has_profile(entry: &Value) -> bool {
    entry
        .get("profile")
        .and_then(Value::as_object)
        .is_some_and(|p| {
            ["displayName", "email", "role"]
                .iter()
                .any(|k| p.contains_key(*k))
        })
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
    for entry in &entries {
        let member_id = entry
            .get("memberId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if member_id.is_empty() {
            return Err(AppError::BadRequest(
                "Each entry requires a memberId".into(),
            ));
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
        let member_id = entry
            .get("memberId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if member_id == user.id {
            skipped.push(member_id);
            continue;
        }
        let Some(member) = store::find_member(&state.db, &member_id).await? else {
            failure_count += 1;
            results.push(
                json!({ "memberId": member_id, "success": false, "error": "Member not found" }),
            );
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
                "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
                 VALUES ($1, $2, 'member', 0, $3) ON CONFLICT DO NOTHING",
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
            let res = sqlx::query("DELETE FROM team_members WHERE agent_id = $1 AND team_id = $2")
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
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "member batch edit",
        "agent",
        None,
        Some(
            json!({ "successCount": success_count, "failureCount": failure_count,
                     "skipped": skipped, "reason": body.get("reason") }),
        ),
        None,
        None,
    )
    .await;

    let mut payload = json!({
        "results": results,
        "successCount": success_count,
        "failureCount": failure_count,
        "skipped": skipped,
    });
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
    let token = body
        .get("undoToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(AppError::BadRequest("undoToken is required".into()));
    }
    let Some((owner, snapshot)) = state.batch_undo.take(&token) else {
        return Err(AppError::BadRequest("Invalid or expired undo token".into()));
    };
    if owner != user.id {
        state.batch_undo.restore(&token, owner, snapshot);
        return Err(AppError::Forbidden(
            "Undo token belongs to a different user".into(),
        ));
    }

    let mut results: Vec<Value> = Vec::new();
    let mut restored = 0usize;
    let entries = snapshot.as_array().cloned().unwrap_or_default();
    for entry in &entries {
        let member_id = entry
            .get("memberId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let profile = &entry["profile"];
        let now = now_iso();
        sqlx::query(
            "UPDATE agents SET display_name = $1, email = $2, role = $3, updated_at = $4
             WHERE id = $5 AND deleted_at IS NULL",
        )
        .bind(
            profile
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(""),
        )
        .bind(profile.get("email").and_then(Value::as_str).unwrap_or(""))
        .bind(
            profile
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("agent"),
        )
        .bind(&now)
        .bind(&member_id)
        .execute(&state.db)
        .await?;
        sqlx::query("DELETE FROM team_members WHERE agent_id = $1")
            .bind(&member_id)
            .execute(&state.db)
            .await?;
        for m in entry
            .get("memberships")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            sqlx::query(
                "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
            )
            .bind(&member_id)
            .bind(m.get("teamId").and_then(Value::as_i64))
            .bind(
                m.get("roleInTeam")
                    .and_then(Value::as_str)
                    .unwrap_or("member"),
            )
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

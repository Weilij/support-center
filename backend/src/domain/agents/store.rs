//! Agents/Operators persistence helpers (CRD §3.3, lines 2154-2321): operator profiles,
//! per-operator skill inventory, and the presence/availability system.

use serde_json::{json, Value};
use sqlx::PgPool;

use crate::db::now_iso;

pub const PRESENCE_STATES: [&str; 6] = ["online", "busy", "away", "offline", "break", "meeting"];
pub const SKILL_CATEGORIES: [&str; 6] =
    ["communication", "technical", "product", "language", "platform", "soft_skill"];
pub const SKILL_LEVELS: [&str; 4] = ["beginner", "intermediate", "advanced", "expert"];

/// Presence history is retained to the most recent 100 entries per operator (CRD 2263).
pub const HISTORY_CAP: i64 = 100;

// -------------------------------------------------------------------- operator profiles

#[derive(Debug, sqlx::FromRow)]
pub struct OperatorRow {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub is_active: i64,
    pub password_policy: String,
    pub last_active_at: Option<String>,
    pub last_login_at: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub primary_team_id: Option<i64>,
    pub team_name: Option<String>,
    pub position: Option<String>,
}

pub const OPERATOR_SELECT: &str =
    "SELECT a.id, a.email, a.display_name, a.role, a.is_active, a.password_policy,
            a.last_active_at, a.last_login_at, a.created_at, a.updated_at,
            tm.team_id AS primary_team_id, t.name AS team_name, a.position
     FROM agents a
     LEFT JOIN team_members tm ON tm.agent_id = a.id AND tm.is_primary = 1
     LEFT JOIN teams t ON t.id = tm.team_id
     WHERE a.deleted_at IS NULL";

pub async fn find_operator(pool: &PgPool, id: &str) -> sqlx::Result<Option<OperatorRow>> {
    sqlx::query_as(&crate::db::pg_params(&format!("{OPERATOR_SELECT} AND a.id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Password material is always blank in responses (CRD 2168, 2303).
pub fn operator_view(o: &OperatorRow) -> Value {
    json!({
        "id": o.id,
        "email": o.email,
        "displayName": o.display_name,
        "role": o.role,
        "isActive": o.is_active != 0,
        "password": "",
        "passwordPolicy": o.password_policy,
        "teamId": o.primary_team_id,
        "teamName": o.team_name,
        "lastActiveAt": o.last_active_at,
        "lastLoginAt": o.last_login_at,
        "createdAt": o.created_at,
        "updatedAt": o.updated_at,
        "position": o.position,
    })
}

// ------------------------------------------------------------------------------- skills

#[derive(Debug, sqlx::FromRow)]
pub struct SkillRow {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub category: String,
    pub level: String,
    pub description: Option<String>,
    pub certified: i64,
    pub certified_at: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
}

pub fn skill_view(s: &SkillRow) -> Value {
    json!({
        "id": s.id,
        "agentId": s.agent_id,
        "name": s.name,
        "category": s.category,
        "level": s.level,
        "description": s.description,
        "certified": s.certified != 0,
        "certifiedAt": s.certified_at,
        "createdAt": s.created_at,
        "updatedAt": s.updated_at,
    })
}

pub async fn skills_of(pool: &PgPool, agent_id: &str) -> sqlx::Result<Vec<SkillRow>> {
    sqlx::query_as(
        "SELECT id, agent_id, name, category, level, description, certified, certified_at,
                created_at, updated_at
         FROM agent_skills WHERE agent_id = $1 ORDER BY created_at, id",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
}

pub async fn find_skill(
    pool: &PgPool,
    agent_id: &str,
    skill_id: &str,
) -> sqlx::Result<Option<SkillRow>> {
    sqlx::query_as(
        "SELECT id, agent_id, name, category, level, description, certified, certified_at,
                created_at, updated_at
         FROM agent_skills WHERE agent_id = $1 AND id = $2",
    )
    .bind(agent_id)
    .bind(skill_id)
    .fetch_optional(pool)
    .await
}

// ----------------------------------------------------------------------------- presence

#[derive(Debug, sqlx::FromRow)]
pub struct StatusRow {
    pub status: String,
    pub since: String,
    pub available_until: Option<String>,
    pub note: Option<String>,
}

pub fn status_view(s: &StatusRow) -> Value {
    json!({
        "status": s.status,
        "since": s.since,
        "availableUntil": s.available_until,
        "note": s.note,
    })
}

/// Replaces the operator's presence and prepends a history entry, trimming history to
/// the most recent `HISTORY_CAP` entries (CRD 2260-2263).
pub async fn set_status(
    pool: &PgPool,
    agent_id: &str,
    status: &str,
    available_until: Option<&str>,
    note: Option<&str>,
) -> sqlx::Result<StatusRow> {
    let now = now_iso();
    sqlx::query(
        "INSERT INTO agent_status (agent_id, status, since, available_until, note, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT(agent_id) DO UPDATE SET status = excluded.status, since = excluded.since,
             available_until = excluded.available_until, note = excluded.note,
             updated_at = excluded.updated_at",
    )
    .bind(agent_id)
    .bind(status)
    .bind(&now)
    .bind(available_until)
    .bind(note)
    .bind(&now)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO agent_status_history (agent_id, status, since, available_until, note, recorded_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(agent_id)
    .bind(status)
    .bind(&now)
    .bind(available_until)
    .bind(note)
    .bind(&now)
    .execute(pool)
    .await?;
    sqlx::query(
        "DELETE FROM agent_status_history WHERE agent_id = $1 AND id NOT IN
         (SELECT id FROM agent_status_history WHERE agent_id = $2 ORDER BY id DESC LIMIT $3)",
    )
    .bind(agent_id)
    .bind(agent_id)
    .bind(HISTORY_CAP)
    .execute(pool)
    .await?;
    Ok(StatusRow {
        status: status.to_string(),
        since: now,
        available_until: available_until.map(String::from),
        note: note.map(String::from),
    })
}

/// Current presence with passive auto-expiry: a past availability-until forces a
/// transition to offline (annotated as auto-expired) recorded in history (CRD 2251, 2315).
/// Operators with no record are reported as a default offline presence (not persisted).
pub async fn status_with_expiry(pool: &PgPool, agent_id: &str) -> sqlx::Result<StatusRow> {
    let row: Option<StatusRow> = sqlx::query_as(
        "SELECT status, since, available_until, note FROM agent_status WHERE agent_id = $1",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(StatusRow {
            status: "offline".into(),
            since: now_iso(),
            available_until: None,
            note: None,
        });
    };
    if let Some(until) = &row.available_until {
        if *until <= now_iso() {
            return set_status(pool, agent_id, "offline", None, Some("auto-expired")).await;
        }
    }
    Ok(row)
}

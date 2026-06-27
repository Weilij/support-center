//! Teams domain persistence helpers (CRD §3.2, lines 1792-2154).

use serde_json::{json, Value};
use sqlx::PgPool;

use crate::config::Config;
use crate::db::now_iso;

/// Placeholder sentinel identity that anonymized references are reassigned to when an
/// account is permanently deleted (CRD 1978, 2294). Stored soft-deleted so it never
/// surfaces in listings nor collides with the active-email uniqueness rule.
pub const PLACEHOLDER_AGENT_ID: &str = "deleted-user";

// ------------------------------------------------------------------------------ teams

#[derive(Debug, sqlx::FromRow)]
pub struct TeamWithCounts {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_active: i64,
    pub qr_code: Option<String>,
    pub qr_code_image: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub member_count: i64,
    pub active_member_count: i64,
    pub conversation_count: i64,
}

/// SELECT producing a team plus its derived statistics (CRD 1828, 1833).
pub fn team_select(filter: &str, tail: &str) -> String {
    format!(
        "SELECT t.id, t.name, t.description, t.is_active, t.qr_code, t.qr_code_image,
                t.created_at, t.updated_at,
                (SELECT COUNT(*) FROM team_members m WHERE m.team_id = t.id) AS member_count,
                (SELECT COUNT(*) FROM team_members m
                   JOIN agents a ON a.id = m.agent_id AND a.deleted_at IS NULL AND a.is_active = 1
                  WHERE m.team_id = t.id) AS active_member_count,
                (SELECT COUNT(*) FROM conversations c
                  WHERE c.team_id = t.id AND c.deleted_at IS NULL) AS conversation_count
         FROM teams t WHERE t.deleted_at IS NULL {filter} {tail}"
    )
}

pub async fn team_with_counts(pool: &PgPool, id: i64) -> sqlx::Result<Option<TeamWithCounts>> {
    sqlx::query_as(&crate::db::pg_params(&team_select("AND t.id = ?", "")))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub fn team_view(t: &TeamWithCounts) -> Value {
    json!({
        "id": t.id,
        "name": t.name,
        "description": t.description,
        "isActive": t.is_active != 0,
        "qrCode": t.qr_code,
        "memberCount": t.member_count,
        "activeMemberCount": t.active_member_count,
        "conversationCount": t.conversation_count,
        "createdAt": t.created_at,
        "updatedAt": t.updated_at,
    })
}

// ---------------------------------------------------------------------- member accounts

#[derive(Debug, sqlx::FromRow)]
pub struct MemberRow {
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
}

pub const MEMBER_COLUMNS: &str = "id, email, display_name, role, is_active, password_policy,
    last_active_at, last_login_at, created_at, updated_at";

pub async fn find_member(pool: &PgPool, id: &str) -> sqlx::Result<Option<MemberRow>> {
    sqlx::query_as(&crate::db::pg_params(&format!(
        "SELECT {MEMBER_COLUMNS} FROM agents WHERE id = $1 AND deleted_at IS NULL"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub fn member_view(m: &MemberRow) -> Value {
    json!({
        "id": m.id,
        "email": m.email,
        "displayName": m.display_name,
        "role": m.role,
        "isActive": m.is_active != 0,
        "passwordPolicy": m.password_policy,
        "lastActiveAt": m.last_active_at,
        "lastLoginAt": m.last_login_at,
        "createdAt": m.created_at,
        "updatedAt": m.updated_at,
    })
}

// ------------------------------------------------------------------------- memberships

#[derive(Debug, sqlx::FromRow)]
pub struct MembershipRow {
    pub agent_id: String,
    pub team_id: i64,
    pub role: String,
    pub is_primary: i64,
    pub joined_at: String,
}

pub async fn find_membership(
    pool: &PgPool,
    agent_id: &str,
    team_id: i64,
) -> sqlx::Result<Option<MembershipRow>> {
    sqlx::query_as(
        "SELECT agent_id, team_id, role, is_primary, joined_at FROM team_members
         WHERE agent_id = $1 AND team_id = $2",
    )
    .bind(agent_id)
    .bind(team_id)
    .fetch_optional(pool)
    .await
}

pub async fn memberships_of(pool: &PgPool, agent_id: &str) -> sqlx::Result<Vec<MembershipRow>> {
    sqlx::query_as(
        "SELECT agent_id, team_id, role, is_primary, joined_at FROM team_members
         WHERE agent_id = $1 ORDER BY is_primary DESC, team_id",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
}

pub fn membership_view(m: &MembershipRow) -> Value {
    json!({
        "agentId": m.agent_id,
        "teamId": m.team_id,
        "roleInTeam": m.role,
        "isPrimary": m.is_primary != 0,
        "joinedAt": m.joined_at,
    })
}

/// Clears every primary flag the agent holds so that exactly one team can then be
/// marked primary (CRD 2045, 2145).
pub async fn clear_primary(pool: &PgPool, agent_id: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE team_members SET is_primary = 0 WHERE agent_id = $1")
        .bind(agent_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// When the agent has memberships but no primary one, promotes one remaining membership
/// to primary (CRD 1920, 2135, 2145). Returns the promoted team id, if any.
pub async fn promote_primary_if_needed(pool: &PgPool, agent_id: &str) -> sqlx::Result<Option<i64>> {
    let has_primary: Option<i64> = sqlx::query_scalar(
        "SELECT team_id FROM team_members WHERE agent_id = $1 AND is_primary = 1 LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    if has_primary.is_some() {
        return Ok(None);
    }
    let next: Option<i64> = sqlx::query_scalar(
        "SELECT team_id FROM team_members WHERE agent_id = $1 ORDER BY joined_at, id LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    if let Some(team_id) = next {
        sqlx::query("UPDATE team_members SET is_primary = 1 WHERE agent_id = $1 AND team_id = $2")
            .bind(agent_id)
            .bind(team_id)
            .execute(pool)
            .await?;
        return Ok(Some(team_id));
    }
    Ok(None)
}

/// Replaces the agent's memberships with a single primary membership in `team_id`
/// (profile team-change and bulk transfer semantics, CRD 2187, 2285, 2304).
pub async fn replace_memberships(pool: &PgPool, agent_id: &str, team_id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM team_members WHERE agent_id = $1")
        .bind(agent_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
         VALUES ($1, $2, 'member', 1, $3)",
    )
    .bind(agent_id)
    .bind(team_id)
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(())
}

// ------------------------------------------------------------- permanent member deletion

/// Irreversible account deletion with reference cleanup (CRD 1978, 2294): related
/// records are deleted, nulled, or reassigned to the placeholder identity so history
/// survives with authorship anonymized.
pub async fn purge_member(pool: &PgPool, member_id: &str) -> sqlx::Result<()> {
    let now = now_iso();
    // Ensure the placeholder sentinel exists (soft-deleted, inactive).
    sqlx::query(
        "INSERT INTO agents
            (id, email, password_hash, display_name, role, is_active, password_policy,
             deleted_at, created_at)
         VALUES ($1, 'deleted-user@system.local', '', 'Deleted User', 'agent', 0,
                 'unchangeable', $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(PLACEHOLDER_AGENT_ID)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    let mut tx = pool.begin().await?;

    // Deleted outright: notifications and pending scheduled outbound messages.
    sqlx::query("DELETE FROM notifications WHERE agent_id = $1")
        .bind(member_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM scheduled_messages WHERE agent_id = $1 AND status = 'pending'")
        .bind(member_id)
        .execute(&mut *tx)
        .await?;
    // Non-pending scheduled records are kept, anonymized.
    sqlx::query("UPDATE scheduled_messages SET agent_id = $1 WHERE agent_id = $2")
        .bind(PLACEHOLDER_AGENT_ID)
        .bind(member_id)
        .execute(&mut *tx)
        .await?;

    // Nulled references (sender_name snapshots keep history readable).
    for sql in [
        "UPDATE messages SET agent_id = NULL WHERE agent_id = $1",
        "UPDATE attachments SET uploaded_by = NULL WHERE uploaded_by = $1",
        "UPDATE customer_tags SET assigned_by = NULL WHERE assigned_by = $1",
        "UPDATE conversation_tags SET assigned_by = NULL WHERE assigned_by = $1",
        "UPDATE channel_integrations SET configured_by = NULL WHERE configured_by = $1",
        "UPDATE customer_feedback SET agent_id = NULL WHERE agent_id = $1",
    ] {
        sqlx::query(sql).bind(member_id).execute(&mut *tx).await?;
    }

    // Reassigned to the placeholder identity (NOT NULL / RESTRICT references).
    for sql in [
        "UPDATE message_recall_logs SET agent_id = $1 WHERE agent_id = $2",
        "UPDATE conversation_transfers SET transferred_by = $1 WHERE transferred_by = $2",
        "UPDATE activity_logs SET agent_id = $1 WHERE agent_id = $2",
        "UPDATE tags SET created_by = $1 WHERE created_by = $2",
        "UPDATE reports SET created_by = $1 WHERE created_by = $2",
        "UPDATE scheduled_reports SET created_by = $1 WHERE created_by = $2",
        "UPDATE report_downloads SET downloaded_by = $1 WHERE downloaded_by = $2",
        "UPDATE report_templates SET created_by = $1 WHERE created_by = $2",
    ] {
        sqlx::query(sql)
            .bind(PLACEHOLDER_AGENT_ID)
            .bind(member_id)
            .execute(&mut *tx)
            .await?;
    }

    // Session/refresh credentials become unusable once the row is gone; tidy them up.
    sqlx::query("DELETE FROM auth_sessions WHERE agent_id = $1")
        .bind(member_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM refresh_tokens WHERE agent_id = $1")
        .bind(member_id)
        .execute(&mut *tx)
        .await?;

    // Team memberships, reminders, skills, and presence rows cascade with the account.
    sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(member_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await
}

// ----------------------------------------------------------------------- QR artifacts

pub fn join_url(config: &Config, team_id: i64, token: &str) -> String {
    let base = config
        .frontend_url
        .clone()
        .or_else(|| config.backend_url.clone())
        .unwrap_or_else(|| "http://localhost:3000".into());
    format!("{base}/join/team/{team_id}?token={token}")
}

pub fn qr_image_url(data: &str) -> String {
    format!("https://api.qrserver.com/v1/create-qr-code/?size=300x300&data={data}")
}

pub fn liff_url(team_id: i64) -> String {
    format!("https://liff.line.me/join?teamId={team_id}")
}

#[derive(Debug, sqlx::FromRow)]
pub struct QrRow {
    pub id: String,
    pub team_id: i64,
    pub token: String,
    pub url: Option<String>,
    pub image_url: Option<String>,
    pub campaign: Option<String>,
    pub description: Option<String>,
    pub scan_count: i64,
    pub max_scans: Option<i64>,
    pub is_active: i64,
    pub expires_at: Option<String>,
    pub created_at: String,
}

pub fn qr_view(q: &QrRow) -> Value {
    json!({
        "id": q.id,
        "teamId": q.team_id,
        "token": q.token,
        "url": q.url,
        "imageUrl": q.image_url,
        "campaignName": q.campaign,
        "description": q.description,
        "scanCount": q.scan_count,
        "maxUses": q.max_scans,
        "isActive": q.is_active != 0,
        "expiresAt": q.expires_at,
        "createdAt": q.created_at,
    })
}

/// Creates a scan-to-join QR record for the team and returns it (CRD 1840, 2081).
pub async fn create_join_qr(
    pool: &PgPool,
    config: &Config,
    team_id: i64,
    campaign: Option<&str>,
    description: Option<&str>,
    expires_at: Option<&str>,
    max_scans: Option<i64>,
) -> sqlx::Result<QrRow> {
    let id = uuid::Uuid::new_v4().to_string();
    let token = uuid::Uuid::new_v4().to_string();
    let url = join_url(config, team_id, &token);
    let image_url = qr_image_url(&token);
    let now = now_iso();
    sqlx::query(
        "INSERT INTO qr_codes (id, team_id, token, url, image_url, campaign, description,
                               max_scans, is_active, expires_at, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9, $10)",
    )
    .bind(&id)
    .bind(team_id)
    .bind(&token)
    .bind(&url)
    .bind(&image_url)
    .bind(campaign)
    .bind(description)
    .bind(max_scans)
    .bind(expires_at)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(QrRow {
        id,
        team_id,
        token,
        url: Some(url),
        image_url: Some(image_url),
        campaign: campaign.map(String::from),
        description: description.map(String::from),
        scan_count: 0,
        max_scans,
        is_active: 1,
        expires_at: expires_at.map(String::from),
        created_at: now,
    })
}

#[derive(Debug, sqlx::FromRow)]
pub struct LiffRow {
    pub id: String,
    pub team_id: i64,
    pub url: Option<String>,
    pub image_url: Option<String>,
    pub scan_count: i64,
    pub is_active: i64,
    pub created_at: String,
    pub updated_at: Option<String>,
}

pub async fn find_liff(pool: &PgPool, team_id: i64) -> sqlx::Result<Option<LiffRow>> {
    sqlx::query_as(
        "SELECT id, team_id, url, image_url, scan_count, is_active, created_at, updated_at
         FROM team_liff_links WHERE team_id = $1",
    )
    .bind(team_id)
    .fetch_optional(pool)
    .await
}

/// (Re)generates the team's LIFF deep-link QR; the scan count survives regeneration.
pub async fn upsert_liff(pool: &PgPool, team_id: i64) -> sqlx::Result<LiffRow> {
    let url = liff_url(team_id);
    let image_url = qr_image_url(&format!("liff-{team_id}"));
    let now = now_iso();
    sqlx::query(
        "INSERT INTO team_liff_links (id, team_id, url, image_url, is_active, created_at)
         VALUES ($1, $2, $3, $4, 1, $5)
         ON CONFLICT(team_id) DO UPDATE SET url = excluded.url, image_url = excluded.image_url,
             is_active = 1, updated_at = excluded.created_at",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(team_id)
    .bind(&url)
    .bind(&image_url)
    .bind(&now)
    .execute(pool)
    .await?;
    find_liff(pool, team_id)
        .await
        .map(|r| r.expect("liff row exists after upsert"))
}

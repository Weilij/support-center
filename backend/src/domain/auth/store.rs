//! Persistence for accounts, sessions, refresh-token rotation, revocation, activity log.

use sqlx::PgPool;

use crate::db::now_iso;
use crate::state::TeamMembership;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentRow {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub display_name: String,
    pub role: String,
    pub is_active: i64,
    pub password_policy: String,
    pub last_active_at: Option<String>,
    pub last_login_at: Option<String>,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub position: Option<String>,
    pub tokens_valid_after: Option<String>,
}

impl AgentRow {
    pub fn created_at_millis(&self) -> i64 {
        chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|t| t.timestamp_millis())
            .unwrap_or(0)
    }
}

pub async fn find_active_agent_by_email(
    pool: &PgPool,
    email: &str,
) -> sqlx::Result<Option<AgentRow>> {
    sqlx::query_as::<_, AgentRow>("SELECT * FROM agents WHERE email = $1 AND deleted_at IS NULL")
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn find_deleted_agent_by_email(
    pool: &PgPool,
    email: &str,
) -> sqlx::Result<Option<AgentRow>> {
    sqlx::query_as::<_, AgentRow>(
        "SELECT * FROM agents WHERE email = $1 AND deleted_at IS NOT NULL ORDER BY created_at DESC LIMIT 1",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
}

pub async fn find_agent_by_id(pool: &PgPool, id: &str) -> sqlx::Result<Option<AgentRow>> {
    sqlx::query_as::<_, AgentRow>("SELECT * FROM agents WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn memberships(pool: &PgPool, agent_id: &str) -> sqlx::Result<Vec<TeamMembership>> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT team_id, role, is_primary FROM team_members WHERE agent_id = $1 ORDER BY is_primary DESC, team_id",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(team_id, role, is_primary)| TeamMembership {
            team_id,
            role,
            is_primary: is_primary != 0,
        })
        .collect())
}

pub async fn team_name(pool: &PgPool, team_id: i64) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT name FROM teams WHERE id = $1 AND deleted_at IS NULL")
        .bind(team_id)
        .fetch_optional(pool)
        .await
}

// --- auth sessions (CRD §1.2 Part A, lines 301-328) ---

pub const SESSION_TTL_HOURS: i64 = 24;

pub async fn create_session(pool: &PgPool, agent: &AgentRow) -> sqlx::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let expires = (chrono::Utc::now() + chrono::Duration::hours(SESSION_TTL_HOURS))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let data = serde_json::json!({
        "userId": agent.id,
        "email": agent.email,
        "displayName": agent.display_name,
        "role": agent.role,
    })
    .to_string();
    sqlx::query(
        "INSERT INTO auth_sessions (id, agent_id, data, expires_at, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&id)
    .bind(&agent.id)
    .bind(data)
    .bind(expires)
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(id)
}

/// Returns the owning agent id when the session exists and is unexpired.
pub async fn lookup_session(pool: &PgPool, session_id: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT agent_id FROM auth_sessions WHERE id = $1 AND expires_at > $2")
        .bind(session_id)
        .bind(now_iso())
        .fetch_optional(pool)
        .await
}

pub async fn delete_session(pool: &PgPool, session_id: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM auth_sessions WHERE id = $1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

// --- refresh-token rotation (CRD lines 165-172) ---

pub async fn record_refresh_token(
    pool: &PgPool,
    jti: &str,
    agent_id: &str,
    exp_unix: i64,
) -> sqlx::Result<()> {
    let expires = chrono::DateTime::from_timestamp(exp_unix, 0)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    sqlx::query(
        "INSERT INTO refresh_tokens (jti, agent_id, issued_at, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(jti)
    .bind(agent_id)
    .bind(now_iso())
    .bind(expires)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
pub struct RefreshRow {
    pub jti: String,
    pub agent_id: String,
    pub consumed_at: Option<String>,
    pub revoked_at: Option<String>,
}

pub async fn get_refresh_token(pool: &PgPool, jti: &str) -> sqlx::Result<Option<RefreshRow>> {
    sqlx::query_as::<_, RefreshRow>(
        "SELECT jti, agent_id, consumed_at, revoked_at FROM refresh_tokens WHERE jti = $1",
    )
    .bind(jti)
    .fetch_optional(pool)
    .await
}

pub async fn consume_refresh_token(pool: &PgPool, jti: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE refresh_tokens SET consumed_at = $1 WHERE jti = $2")
        .bind(now_iso())
        .bind(jti)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke_refresh_token(pool: &PgPool, jti: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE refresh_tokens SET revoked_at = $1 WHERE jti = $2")
        .bind(now_iso())
        .bind(jti)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke_user_credentials(
    pool: &PgPool,
    agent_id: &str,
    revoked_at: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = $1
         WHERE agent_id = $2 AND revoked_at IS NULL",
    )
    .bind(revoked_at)
    .bind(agent_id)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM auth_sessions WHERE agent_id = $1")
        .bind(agent_id)
        .execute(pool)
        .await?;
    Ok(())
}

// --- jti revocation (access credentials, CRD line 269) ---

pub async fn revoke_jti(
    pool: &PgPool,
    jti: &str,
    agent_id: Option<&str>,
    exp_unix: Option<i64>,
) -> sqlx::Result<()> {
    let expires = exp_unix
        .and_then(|e| chrono::DateTime::from_timestamp(e, 0))
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    sqlx::query(
        "INSERT INTO revoked_tokens (jti, agent_id, revoked_at, expires_at) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(jti)
    .bind(agent_id)
    .bind(now_iso())
    .bind(expires)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn is_jti_revoked(pool: &PgPool, jti: &str) -> sqlx::Result<bool> {
    let found: Option<String> = sqlx::query_scalar("SELECT jti FROM revoked_tokens WHERE jti = $1")
        .bind(jti)
        .fetch_optional(pool)
        .await?;
    Ok(found.is_some())
}

// --- activity log (consumed by §3.5; written here per CRD line 290) ---

#[allow(clippy::too_many_arguments)]
pub async fn log_activity(
    pool: &PgPool,
    agent_id: &str,
    agent_name: &str,
    agent_role: &str,
    action: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    details: Option<serde_json::Value>,
    ip: Option<&str>,
    user_agent: Option<&str>,
) {
    if let Err(error) = sqlx::query(
        "INSERT INTO activity_logs (agent_id, agent_name, agent_role, action, resource_type, resource_id, details, ip_address, user_agent, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(agent_id)
    .bind(agent_name)
    .bind(agent_role)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
    .bind(details.map(|d| d.to_string()))
    .bind(ip)
    .bind(user_agent)
    .bind(now_iso())
    .execute(pool)
    .await
    {
        tracing::warn!(
            error = %error,
            agent_id,
            action,
            resource_type,
            "auth activity log insert failed"
        );
    }
}

// --- password hashing ---

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    Ok(argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash)
        .map(|parsed| {
            argon2::Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// A fixed hash verified on unknown-account login attempts so all failure modes take
/// comparable time (CRD line 139 anti-enumeration guarantee).
pub fn dummy_verify(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let hash = DUMMY.get_or_init(|| hash_password("timing-equalizer").unwrap_or_default());
    let _ = verify_password(password, hash);
}

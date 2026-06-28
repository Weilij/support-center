//! Channel-integration persistence helpers (CRD §4.1, lines 2612-2720).

use serde_json::{json, Map, Value};
use sqlx::PgPool;

use crate::crypto;
use crate::db::now_iso;
use crate::error::AppError;

/// One channel-connection row (CRD 2700: the central record).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChannelRow {
    pub id: i64,
    pub team_id: i64,
    pub platform: String,
    pub config: Option<String>,
    pub credentials: Option<String>,
    pub webhook_config: Option<String>,
    pub stats: Option<String>,
    pub is_active: i64,
    pub is_verified: i64,
    pub verified_at: Option<String>,
    pub configured_by: Option<String>,
    pub metadata: Option<String>,
    pub last_error: Option<String>,
    pub error_count: i64,
    pub created_at: String,
    pub updated_at: Option<String>,
}

const SELECT: &str = "SELECT id, team_id, platform, config, credentials, webhook_config, stats,
        is_active, is_verified, verified_at, configured_by, metadata, last_error, error_count,
        created_at, updated_at
 FROM channel_integrations";

fn parse_json(raw: &Option<String>) -> Value {
    raw.as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null)
}

/// Usage statistics with zeroed defaults when absent or unparseable (CRD 2704).
pub fn stats_view(raw: &Option<String>) -> Value {
    let parsed = parse_json(raw);
    json!({
        "messagesSent": parsed.get("messagesSent").and_then(Value::as_i64).unwrap_or(0),
        "messagesReceived": parsed.get("messagesReceived").and_then(Value::as_i64).unwrap_or(0),
        "lastMessageAt": parsed.get("lastMessageAt").cloned().unwrap_or(Value::Null),
    })
}

/// Sanitized client-facing record: the encrypted-credential blob is stripped
/// out before serialization, always (CRD 2622).
pub fn view(row: &ChannelRow) -> Value {
    // Which secret fields have a stored value — the field NAMES only, never any
    // value (decrypted or otherwise) (CRD 2622).
    let creds_set: Vec<String> = row
        .credentials
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default();
    json!({
        "id": row.id,
        "teamId": row.team_id,
        "platform": row.platform,
        "config": parse_json(&row.config),
        "credentialsSet": creds_set,
        "webhookConfig": parse_json(&row.webhook_config),
        "stats": stats_view(&row.stats),
        "isActive": row.is_active != 0,
        "isVerified": row.is_verified != 0,
        "verifiedAt": row.verified_at,
        "configuredBy": row.configured_by,
        "metadata": parse_json(&row.metadata),
        "lastError": parse_json(&row.last_error),
        "errorCount": row.error_count,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
    })
}

pub async fn find_by_id(db: &PgPool, id: i64) -> Result<Option<ChannelRow>, AppError> {
    let sql = format!("{SELECT} WHERE id = $1");
    Ok(sqlx::query_as(&crate::db::pg_params(&sql))
        .bind(id)
        .fetch_optional(db)
        .await?)
}

/// Connections matching the team (and optional platform) filter, newest first
/// (CRD 2627, 2705). `team_id = None` lists across all teams (admin only).
pub async fn list(
    db: &PgPool,
    team_id: Option<i64>,
    platform: Option<&str>,
) -> Result<Vec<ChannelRow>, AppError> {
    let mut sql = format!("{SELECT} WHERE 1=1");
    if team_id.is_some() {
        sql.push_str(" AND team_id = ?");
    }
    if platform.is_some() {
        sql.push_str(" AND platform = ?");
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC");
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_as(&sql);
    if let Some(t) = team_id {
        q = q.bind(t);
    }
    if let Some(p) = platform {
        q = q.bind(p);
    }
    Ok(q.fetch_all(db).await?)
}

/// Whether the team already has an *enabled* connection for this platform
/// (CRD 2716: at most one enabled connection per (team, platform)).
pub async fn active_exists(
    db: &PgPool,
    team_id: i64,
    platform: &str,
    exclude_id: Option<i64>,
) -> Result<bool, AppError> {
    let row: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM channel_integrations
         WHERE team_id = $1 AND platform = $2 AND is_active = 1 AND ($3::bigint IS NULL OR id != $3::bigint)
         LIMIT 1",
    )
    .bind(team_id)
    .bind(platform)
    .bind(exclude_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

pub struct NewChannel<'a> {
    pub team_id: i64,
    pub platform: &'a str,
    pub config: Value,
    pub credentials: Value,
    pub webhook_config: Value,
    pub metadata: Option<Value>,
    pub configured_by: &'a str,
}

/// Persist a new connection: enabled, not-yet-verified, zeroed statistics
/// (CRD 2637).
pub async fn insert(db: &PgPool, new: NewChannel<'_>) -> Result<ChannelRow, AppError> {
    let stats = json!({ "messagesSent": 0, "messagesReceived": 0, "lastMessageAt": null });
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO channel_integrations
            (team_id, platform, config, credentials, webhook_config, stats,
             is_active, is_verified, configured_by, metadata, error_count, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, 1, 0, $7, $8, 0, $9) RETURNING id",
    )
    .bind(new.team_id)
    .bind(new.platform)
    .bind(new.config.to_string())
    .bind(new.credentials.to_string())
    .bind(new.webhook_config.to_string())
    .bind(stats.to_string())
    .bind(new.configured_by)
    .bind(new.metadata.map(|m| m.to_string()))
    .bind(now_iso())
    .fetch_one(db)
    .await?;
    find_by_id(db, id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to create channel integration".into()))
}

/// Resolve an enabled connection from a presented (platform, team, token)
/// triple, rejecting a token mismatch or a disabled connection (CRD 2722).
pub async fn resolve_by_webhook_token(
    db: &PgPool,
    platform: &str,
    team_id: i64,
    token: &str,
) -> Result<Option<ChannelRow>, AppError> {
    let rows = list(db, Some(team_id), Some(platform)).await?;
    Ok(rows.into_iter().filter(|r| r.is_active != 0).find(|r| {
        parse_json(&r.webhook_config)
            .get("webhookToken")
            .and_then(Value::as_str)
            .is_some_and(|t| t == token)
    }))
}

/// Decrypt the stored credentials blob into a field map, tolerating both the
/// protected and historical plaintext formats per field (CRD 2702, 5724).
pub fn decrypt_credentials(
    key: Option<&str>,
    raw: &Option<String>,
) -> Result<Map<String, Value>, AppError> {
    let parsed = parse_json(raw);
    let Some(obj) = parsed.as_object() else {
        return Ok(Map::new());
    };
    let mut out = Map::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            out.insert(k.clone(), Value::String(crypto::reveal(key, s)?));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(out)
}

/// Structured last-error record (CRD 2706).
pub fn error_record(kind: &str, message: &str, attempts: i64, context: Option<Value>) -> Value {
    json!({
        "timestamp": now_iso(),
        "type": kind,
        "message": message,
        "attempts": attempts,
        "stack": null,
        "context": context,
    })
}

//! Channel-integration management handlers (CRD §4.1, lines 2612-2720).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::crypto;
use crate::db::now_iso;
use crate::domain::auth::store::log_activity;
use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::store::{self, ChannelRow};

pub const PLATFORMS: [&str; 4] = ["line", "facebook", "instagram", "whatsapp"];

/// Per-platform configuration descriptor: JSON body key, required non-secret
/// identifier fields, optional non-secret fields, and required secret credential
/// fields (CRD 2634-2636). Optional plain fields are stored when supplied.
fn platform_fields(
    platform: &str,
) -> (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
) {
    match platform {
        "line" => (
            "lineConfig",
            &["channelId"],
            &["liffId"],
            &["channelAccessToken", "channelSecret"],
        ),
        "facebook" => (
            "facebookConfig",
            &["pageId"],
            &[],
            &["accessToken", "appSecret"],
        ),
        "instagram" => ("instagramConfig", &["igId"], &[], &["accessToken"]),
        "whatsapp" => (
            "whatsappConfig",
            &["phoneNumber", "businessAccountId"],
            &[],
            &["accessToken"],
        ),
        _ => ("config", &[], &[], &[]),
    }
}

fn client_ip(headers: &HeaderMap) -> Option<String> {
    for h in ["cf-connecting-ip", "x-forwarded-for", "x-real-ip"] {
        if let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()) {
            let first = v.split(',').next().unwrap_or(v).trim();
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
    }
    None
}

/// Positive-integer path-parameter guard (CRD §7.1 conventions).
fn parse_id(raw: &str) -> Result<i64> {
    match raw.parse::<i64>() {
        Ok(v) if v > 0 => Ok(v),
        _ => Err(AppError::BadRequest(format!(
            "invalid id: must be a positive integer (got '{raw}')"
        ))),
    }
}

fn admin_gate(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "Only administrators can manage channel integrations".into(),
        ))
    }
}

/// Strict ownership: connection must belong to the caller's primary team
/// (no admin override — used by stats/health, CRD 2685; update/delete/verify
/// also use the caller's primary team, CRD 2655, 2664, 2672).
fn own_team_gate(user: &AuthUser, row: &ChannelRow) -> Result<i64> {
    let team = user
        .primary_team_id
        .ok_or_else(|| AppError::BadRequest("Team context required".into()))?;
    if row.team_id != team {
        return Err(AppError::Forbidden(
            "Channel integration belongs to another team".into(),
        ));
    }
    Ok(team)
}

fn not_found() -> AppError {
    AppError::NotFound("Channel integration not found".into())
}

// --------------------------------------------------- List connections (CRD 2624-2630)

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(rename = "teamId")]
    team_id: Option<String>,
    platform: Option<String>,
}

pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    // A caller with a primary team is scoped to it; an admin without one may
    // pass a team filter or list across all teams; a non-admin without a team
    // is rejected (CRD 2627).
    let team_scope: Option<i64> = if let Some(team) = user.primary_team_id {
        Some(team)
    } else if user.is_admin() {
        match &q.team_id {
            None => None,
            Some(raw) => Some(
                raw.parse::<i64>()
                    .map_err(|_| AppError::BadRequest("Invalid team ID parameter".into()))?,
            ),
        }
    } else {
        return Err(AppError::BadRequest(
            "Team not found for current user".into(),
        ));
    };

    let platform = match &q.platform {
        None => None,
        Some(p) if PLATFORMS.contains(&p.as_str()) => Some(p.as_str()),
        Some(_) => {
            return Err(AppError::BadRequest(
                "Invalid platform. Supported platforms: line, facebook, instagram, whatsapp".into(),
            ));
        }
    };

    let rows = store::list(&state.db, team_scope, platform).await?;
    let data: Vec<Value> = rows.iter().map(store::view).collect();
    Ok((
        StatusCode::OK,
        Json(json!({
            "success": true,
            "data": data,
            "count": data.len(),
            "timestamp": now_iso(),
            "requestId": envelope::request_id(),
        })),
    )
        .into_response())
}

// --------------------------------------------------- Create a connection (CRD 2632-2642)

pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result {
    admin_gate(&user)?;

    // The target team is the caller's primary team or, for an admin without
    // one, the team identifier from the body (CRD 2638).
    let team_id = user
        .primary_team_id
        .or_else(|| body.get("teamId").and_then(Value::as_i64))
        .ok_or_else(|| AppError::BadRequest("Team ID is required".into()))?;

    let platform = body
        .get("platform")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("Platform is required".into()))?
        .to_string();
    if !PLATFORMS.contains(&platform.as_str()) {
        return Err(AppError::BadRequest(
            "Invalid platform. Supported platforms: line, facebook, instagram, whatsapp".into(),
        ));
    }

    let (config_key, plain_fields, optional_plain, secret_fields) = platform_fields(&platform);
    let supplied = body
        .get(config_key)
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::BadRequest(format!("{config_key} is required")))?;
    for field in plain_fields.iter().chain(secret_fields.iter()) {
        let present = supplied
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        if !present {
            return Err(AppError::BadRequest(format!(
                "Missing required {platform} configuration field: {field}"
            )));
        }
    }

    // One *enabled* connection per (team, platform) (CRD 2637, 2716).
    if store::active_exists(&state.db, team_id, &platform, None).await? {
        return Err(AppError::BadRequest(format!(
            "An active {platform} integration already exists for this team"
        )));
    }

    // Fresh random secret routing token + per-connection inbound address that
    // embeds platform, team, and token (CRD 2637, 2722).
    let token = uuid::Uuid::new_v4().simple().to_string();
    let base = state
        .config
        .backend_url
        .as_deref()
        .map(|b| b.trim_end_matches('/').to_string())
        .unwrap_or_else(|| format!("http://localhost:{}", state.config.port));
    let webhook_url = format!("{base}/api/webhooks/{platform}/{team_id}/{token}");

    // Separate the non-sensitive configuration from the encrypted credentials
    // (CRD 2701-2702).
    let mut config = Map::new();
    for field in plain_fields {
        config.insert((*field).to_string(), supplied[*field].clone());
    }
    for field in optional_plain {
        if let Some(v) = supplied
            .get(*field)
            .filter(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
        {
            config.insert((*field).to_string(), v.clone());
        }
    }
    let key = state.config.encryption_key.as_deref();
    let mut credentials = Map::new();
    for field in secret_fields {
        let plaintext = supplied[*field].as_str().unwrap_or_default();
        credentials.insert(
            (*field).to_string(),
            Value::String(crypto::protect(key, plaintext)?),
        );
    }

    let row = store::insert(
        &state.db,
        store::NewChannel {
            team_id,
            platform: &platform,
            config: Value::Object(config),
            credentials: Value::Object(credentials),
            webhook_config: json!({
                "webhookUrl": webhook_url,
                "webhookToken": token,
                "verifyToken": null,
            }),
            metadata: body.get("metadata").filter(|m| m.is_object()).cloned(),
            configured_by: &user.id,
        },
    )
    .await?;

    // Audit entry capturing actor, platform, team and the new identifier,
    // plus the caller's network address and user-agent (CRD 2640).
    log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "channel_integration create",
        "channel_integration",
        Some(&row.id.to_string()),
        Some(json!({ "platform": platform, "teamId": team_id, "integrationId": row.id })),
        client_ip(&headers).as_deref(),
        headers.get("user-agent").and_then(|v| v.to_str().ok()),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "success": true,
            "data": store::view(&row),
            "webhookUrl": webhook_url,
            "timestamp": now_iso(),
            "requestId": envelope::request_id(),
        })),
    )
        .into_response())
}

// --------------------------------------------------- Get one connection (CRD 2644-2650)

pub async fn get_channel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_id(&raw_id)?;
    let row = store::find_by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)?;
    // Same-team callers and admins (any team) may read (CRD 2647).
    if !user.is_admin() && user.primary_team_id != Some(row.team_id) {
        return Err(AppError::Forbidden(
            "Channel integration belongs to another team".into(),
        ));
    }
    Ok(envelope::ok(store::view(&row)))
}

// --------------------------------------------------- Update a connection (CRD 2652-2659)

pub async fn update_channel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    Json(body): Json<Value>,
) -> Result {
    let id = parse_id(&raw_id)?;
    user.primary_team_id
        .ok_or_else(|| AppError::BadRequest("Team context required".into()))?;
    admin_gate(&user)?;
    let row = store::find_by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)?;
    own_team_gate(&user, &row)?;

    let mut config: Map<String, Value> = row
        .config
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let mut credentials: Map<String, Value> = row
        .credentials
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // Only the config block matching the connection's own platform is applied
    // (CRD 2656).
    let (config_key, plain_fields, optional_plain, secret_fields) = platform_fields(&row.platform);
    let mut secrets_changed = false;
    if let Some(patch) = body.get(config_key).and_then(Value::as_object) {
        for field in plain_fields.iter().chain(optional_plain.iter()) {
            if let Some(v) = patch.get(*field) {
                // An optional field provided as blank must not erase the stored
                // value (required plain fields keep their existing behavior).
                if optional_plain.contains(field) && v.as_str().is_some_and(|s| s.trim().is_empty())
                {
                    continue;
                }
                config.insert((*field).to_string(), v.clone());
            }
        }
        let key = state.config.encryption_key.as_deref();
        for field in secret_fields {
            if let Some(v) = patch.get(*field).and_then(Value::as_str) {
                credentials.insert(
                    (*field).to_string(),
                    Value::String(crypto::protect(key, v)?),
                );
                secrets_changed = true;
            }
        }
    }

    let mut is_active = row.is_active != 0;
    if let Some(flag) = body.get("isActive").and_then(Value::as_bool) {
        // Re-enabling remains subject to the uniqueness invariant (CRD 2712).
        if flag
            && row.is_active == 0
            && store::active_exists(&state.db, row.team_id, &row.platform, Some(row.id)).await?
        {
            return Err(AppError::BadRequest(format!(
                "An active {} integration already exists for this team",
                row.platform
            )));
        }
        is_active = flag;
    }

    let metadata = match body.get("metadata") {
        Some(m) if m.is_object() => Some(m.to_string()),
        _ => row.metadata.clone(),
    };

    // Any secret change clears the verified status (CRD 2656, 2714).
    let (is_verified, verified_at) = if secrets_changed {
        (0i64, None::<String>)
    } else {
        (row.is_verified, row.verified_at.clone())
    };

    sqlx::query(
        "UPDATE channel_integrations
         SET config = $1, credentials = $2, is_active = $3, is_verified = $4, verified_at = $5,
             metadata = $6, updated_at = $7
         WHERE id = $8",
    )
    .bind(Value::Object(config).to_string())
    .bind(Value::Object(credentials).to_string())
    .bind(is_active as i64)
    .bind(is_verified)
    .bind(verified_at)
    .bind(metadata)
    .bind(now_iso())
    .bind(row.id)
    .execute(&state.db)
    .await?;

    let updated = store::find_by_id(&state.db, row.id)
        .await?
        .ok_or_else(not_found)?;
    Ok(envelope::ok(store::view(&updated)))
}

// --------------------------------------- Disable a connection (soft delete, CRD 2661-2668)

pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_id(&raw_id)?;
    user.primary_team_id
        .ok_or_else(|| AppError::BadRequest("Team context required".into()))?;
    admin_gate(&user)?;
    let row = store::find_by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)?;
    own_team_gate(&user, &row)?;

    sqlx::query("UPDATE channel_integrations SET is_active = 0, updated_at = $1 WHERE id = $2")
        .bind(now_iso())
        .bind(row.id)
        .execute(&state.db)
        .await?;

    Ok(envelope::message_only(
        "Channel integration disabled successfully",
    ))
}

// --------------------------------------------------- Verify a connection (CRD 2669-2680)

fn verification_failure(message: &str, details: Option<Value>) -> Response {
    let mut body = json!({
        "success": false,
        "verified": false,
        "message": message,
        "timestamp": now_iso(),
    });
    if let Some(d) = details {
        body["details"] = d;
    }
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

fn platform_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client")
    })
}

#[derive(Debug, thiserror::Error)]
enum PlatformVerifyError {
    #[error("Invalid platform verification URL: {0}")]
    Url(String),
    #[error("Platform API request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Platform API returned status {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
}

async fn platform_get_json(
    url: reqwest::Url,
    token: &str,
) -> std::result::Result<Value, PlatformVerifyError> {
    let resp = platform_http_client()
        .get(url)
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(PlatformVerifyError::Status { status, body });
    }
    Ok(resp.json::<Value>().await?)
}

async fn verify_line_credentials(
    state: &AppState,
    token: &str,
    channel_id: String,
    webhook_url: Option<String>,
) -> std::result::Result<Value, PlatformVerifyError> {
    let url = reqwest::Url::parse(&state.config.line_bot_info_url)
        .map_err(|error| PlatformVerifyError::Url(error.to_string()))?;
    let details = platform_get_json(url, token).await?;
    Ok(json!({
        "channelId": channel_id,
        "webhookUrl": webhook_url,
        "botUserId": details.get("userId").and_then(Value::as_str),
        "basicId": details.get("basicId").and_then(Value::as_str),
        "displayName": details.get("displayName").and_then(Value::as_str),
    }))
}

async fn verify_meta_node(
    state: &AppState,
    node_id: &str,
    token: &str,
    fields: &[&str],
) -> std::result::Result<Value, PlatformVerifyError> {
    let url = reqwest::Url::parse_with_params(
        &format!(
            "{}/{}",
            state.config.meta_graph_url.trim_end_matches('/'),
            node_id
        ),
        &[("fields", fields.join(","))],
    )
    .map_err(|error| PlatformVerifyError::Url(error.to_string()))?;
    platform_get_json(url, token).await
}

pub async fn verify_channel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    // Optional test-message body: absent or invalid bodies are tolerated and
    // treated as empty (CRD 2671).
    _body: Option<Json<Value>>,
) -> Result {
    let id = parse_id(&raw_id)?;
    user.primary_team_id
        .ok_or_else(|| AppError::BadRequest("Team context required".into()))?;
    let row = store::find_by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)?;
    own_team_gate(&user, &row)?;

    if row.is_active == 0 {
        return Ok(verification_failure(
            "Channel integration is not active",
            None,
        ));
    }

    let key = state.config.encryption_key.as_deref();
    let creds = store::decrypt_credentials(key, &row.credentials)?;
    let config: Map<String, Value> = row
        .config
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let cred = |name: &str| {
        creds
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let conf = |name: &str| {
        config
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let webhook_url = row
        .webhook_config
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| {
            v.get("webhookUrl")
                .and_then(Value::as_str)
                .map(str::to_string)
        });

    // Per-platform credential/identifier presence + live check (CRD 2673-2676).
    let outcome: std::result::Result<Value, PlatformVerifyError> = match row.platform.as_str() {
        "line" => {
            let token = cred("channelAccessToken");
            if token.is_empty() {
                return Ok(verification_failure("Missing channel access token", None));
            }
            verify_line_credentials(&state, &token, conf("channelId"), webhook_url).await
        }
        "facebook" => {
            let token = cred("accessToken");
            let page_id = conf("pageId");
            if token.is_empty() {
                return Ok(verification_failure("Missing access token", None));
            }
            if page_id.is_empty() {
                return Ok(verification_failure("Missing page ID", None));
            }
            verify_meta_node(&state, &page_id, &token, &["id", "name"])
                .await
                .map(|details| {
                    json!({
                        "pageId": details.get("id").and_then(Value::as_str).unwrap_or(&page_id),
                        "pageName": details.get("name").and_then(Value::as_str),
                    })
                })
        }
        "whatsapp" => {
            let token = cred("accessToken");
            let business_id = conf("businessAccountId");
            let phone_number = conf("phoneNumber");
            if token.is_empty() {
                return Ok(verification_failure("Missing access token", None));
            }
            if business_id.is_empty() {
                return Ok(verification_failure("Missing business account ID", None));
            }
            verify_meta_node(
                &state,
                &business_id,
                &token,
                &["id", "display_phone_number", "verified_name"],
            )
            .await
            .map(|details| {
                json!({
                    "phoneNumberId": details.get("id").and_then(Value::as_str).unwrap_or(&business_id),
                    "displayPhoneNumber": details
                        .get("display_phone_number")
                        .and_then(Value::as_str)
                        .unwrap_or(phone_number.as_str()),
                    "verifiedName": details.get("verified_name").and_then(Value::as_str),
                })
            })
        }
        other => {
            return Ok(verification_failure(
                &format!("Verification is not supported for platform '{other}'"),
                None,
            ));
        }
    };

    let now = now_iso();
    match outcome {
        Ok(mut details) => {
            // Verified: timestamp set, error counter and last error cleared
            // (CRD 2676, 2715).
            sqlx::query(
                "UPDATE channel_integrations
                 SET is_verified = 1, verified_at = $1, error_count = 0, last_error = NULL,
                     updated_at = $2
                 WHERE id = $3",
            )
            .bind(&now)
            .bind(&now)
            .bind(row.id)
            .execute(&state.db)
            .await?;
            details["lastVerifiedAt"] = json!(now);
            Ok((
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "verified": true,
                    "message": format!("{} integration verified successfully", row.platform),
                    "details": details,
                    "timestamp": now,
                })),
            )
                .into_response())
        }
        Err(error) => {
            let message = error.to_string();
            // Failure: error counter incremented, structured last-error stored
            // (CRD 2676).
            let attempts = row.error_count + 1;
            let record = store::error_record(
                "verification_failed",
                &message,
                attempts,
                Some(json!({ "platform": row.platform })),
            );
            sqlx::query(
                "UPDATE channel_integrations
                 SET error_count = $1, last_error = $2, updated_at = $3
                 WHERE id = $4",
            )
            .bind(attempts)
            .bind(record.to_string())
            .bind(&now)
            .bind(row.id)
            .execute(&state.db)
            .await?;
            Ok(verification_failure(&message, None))
        }
    }
}

// --------------------------------------------- Connection statistics (CRD 2682-2687)

pub async fn channel_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_id(&raw_id)?;
    let row = store::find_by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)?;
    // Strict same-team ownership: admins are NOT granted cross-team access
    // here (CRD 2685).
    own_team_gate(&user, &row)?;

    let stats = store::stats_view(&row.stats);
    let days = chrono::DateTime::parse_from_rfc3339(&row.created_at)
        .map(|created| (chrono::Utc::now() - created.with_timezone(&chrono::Utc)).num_days())
        .unwrap_or(0)
        .max(0);

    Ok(envelope::ok(json!({
        "id": row.id,
        "platform": row.platform,
        "messagesSent": stats["messagesSent"],
        "messagesReceived": stats["messagesReceived"],
        "lastMessageAt": stats["lastMessageAt"],
        "isActive": row.is_active != 0,
        "isVerified": row.is_verified != 0,
        "errorCount": row.error_count,
        // Whole days since creation plus a fixed hours-in-last-day figure
        // (CRD 2687).
        "uptime": { "days": days, "hoursInLastDay": 24 },
    })))
}

// --------------------------------------------------- Connection health (CRD 2690-2696)

/// Error-count ceilings for the health classification (CRD 2694): healthy at
/// zero, degraded up to this threshold, down beyond it.
pub const DEGRADED_ERROR_THRESHOLD: i64 = 5;

pub async fn channel_health(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_id(&raw_id)?;
    let row = store::find_by_id(&state.db, id)
        .await?
        .ok_or_else(not_found)?;
    // Same strict ownership rule as statistics (CRD 2693).
    own_team_gate(&user, &row)?;

    let (status, recommendations): (&str, Vec<&str>) = if row.error_count == 0 {
        ("healthy", vec![])
    } else if row.error_count <= DEGRADED_ERROR_THRESHOLD {
        (
            "degraded",
            vec![
                "Monitor the connection for recurring errors",
                "Re-verify the connection to confirm credentials are still valid",
            ],
        )
    } else {
        (
            "down",
            vec![
                "Re-verify or rotate the platform credentials",
                "Check the platform's service status",
                "Review the most recent stored error for details",
            ],
        )
    };

    let last_error: Value = row
        .last_error
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);

    Ok(envelope::ok(json!({
        "id": row.id,
        "platform": row.platform,
        "status": status,
        "checkedAt": now_iso(),
        "consecutiveErrors": row.error_count,
        "lastError": last_error,
        "recommendations": recommendations,
    })))
}

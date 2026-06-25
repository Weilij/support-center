//! Auth & account-management handlers per CRD §1.1 (lines 126-293).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::{is_manager_or_admin, AuthUser};
use crate::middleware::cookies;
use crate::middleware::rate_limit::TrustedClientIp;
use crate::state::AppState;

use super::store::{self, AgentRow};
use super::tokens::{self, Claims, TeamClaim};

type Result<T = Response> = std::result::Result<T, AppError>;

const MONITORING_TOKEN_REFRESH_TTL_SECS: i64 = 604_800;
const MONITORING_TOKEN_MAX_LIFETIME_SECS: i64 = 2_592_000;
const USER_TOKEN_REFRESH_TTL_SECS: i64 = 3600;
const USER_TOKEN_MAX_LIFETIME_SECS: i64 = 86_400;

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn token_issued_before_valid_after(iat: i64, valid_after: &Option<String>) -> bool {
    valid_after
        .as_deref()
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .is_some_and(|valid_after| iat.saturating_mul(1000) < valid_after.timestamp_millis())
}

fn require_admin_access(user: &AuthUser) -> Result<()> {
    if user.is_admin() && user.token_type == "access" {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator access token required".into()))
    }
}

fn bounded_service_ttl(
    now: i64,
    root_iat: i64,
    refresh_ttl: i64,
    max_lifetime: i64,
) -> Option<i64> {
    let remaining = root_iat.saturating_add(max_lifetime).saturating_sub(now);
    (remaining > 0).then_some(refresh_ttl.min(remaining))
}

/// Compact agent view used by login/me (CRD line 140: createdAt as epoch milliseconds).
fn agent_view(agent: &AgentRow) -> Value {
    json!({
        "id": agent.id,
        "email": agent.email,
        "name": agent.display_name,
        "displayName": agent.display_name,
        "role": agent.role,
        "isActive": agent.is_active != 0,
        "createdAt": agent.created_at_millis(),
        "position": agent.position,
    })
}

async fn issue_token_pair(
    state: &AppState,
    agent: &AgentRow,
    teams: &[crate::state::TeamMembership],
) -> Result<(String, String)> {
    let team_claims: Vec<TeamClaim> = teams
        .iter()
        .map(|t| TeamClaim { team_id: t.team_id, role: t.role.clone(), is_primary: t.is_primary })
        .collect();
    let primary = teams.iter().find(|t| t.is_primary).map(|t| t.team_id);

    let mut access = Claims::new(&agent.id, &agent.role, "access", tokens::ACCESS_TTL_SECS);
    access.email = Some(agent.email.clone());
    access.name = Some(agent.display_name.clone());
    access.primary_team_id = primary;
    access.teams = Some(team_claims);

    let refresh = Claims::new(&agent.id, &agent.role, "refresh", tokens::REFRESH_TTL_SECS);
    store::record_refresh_token(&state.db, &refresh.jti, &agent.id, refresh.exp).await?;

    let secret = &state.config.jwt_secret;
    let access_token = tokens::sign(&access, secret)
        .map_err(|e| AppError::Internal(format!("token signing failed: {e}")))?;
    let refresh_token = tokens::sign(&refresh, secret)
        .map_err(|e| AppError::Internal(format!("token signing failed: {e}")))?;
    Ok((access_token, refresh_token))
}

// ---------------------------------------------------------------- Sign In (CRD 135-143)

#[derive(Deserialize)]
pub struct LoginBody {
    pub email: Option<String>,
    pub password: Option<String>,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Extension(client_ip): Extension<TrustedClientIp>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Result {
    let ip = client_ip.0;
    let email = body.email.as_deref().unwrap_or("").trim().to_string();
    let password = body.password.as_deref().unwrap_or("").trim().to_string();
    if email.is_empty() || password.is_empty() {
        return Err(AppError::BadRequest("Email and password are required".into()));
    }

    let generic = || AppError::Unauthorized("Invalid email or password".into());

    let agent = match store::find_active_agent_by_email(&state.db, &email).await? {
        Some(a) => a,
        None => {
            store::dummy_verify(&password); // comparable timing across failure modes
            return Err(generic());
        }
    };
    if !store::verify_password(&password, &agent.password_hash) {
        return Err(generic());
    }
    if agent.is_active == 0 {
        return Err(generic());
    }

    // Forced password change: not fully signed in (CRD 139).
    if agent.password_policy == "must_change" {
        let mut temp = Claims::new(&agent.id, &agent.role, "temp_change", tokens::TEMP_CHANGE_TTL_SECS);
        temp.email = Some(agent.email.clone());
        let temp_token = tokens::sign(&temp, &state.config.jwt_secret)
            .map_err(|e| AppError::Internal(format!("token signing failed: {e}")))?;
        return Ok(envelope::ok_msg(
            json!({
                "mustChangePassword": true,
                "tempToken": temp_token,
                "agent": agent_view(&agent),
            }),
            "Password must be changed before signing in",
        ));
    }

    // Best-effort last-login update (CRD 139: failure does not block sign-in).
    let _ = sqlx::query("UPDATE agents SET last_login_at = $1 WHERE id = $2")
        .bind(crate::db::now_iso())
        .bind(&agent.id)
        .execute(&state.db)
        .await;

    let teams = store::memberships(&state.db, &agent.id).await?;
    let (access_token, refresh_token) = issue_token_pair(&state, &agent, &teams).await?;
    let session_id = store::create_session(&state.db, &agent).await?;

    store::log_activity(
        &state.db,
        &agent.id,
        &agent.display_name,
        &agent.role,
        "login",
        "auth",
        None,
        Some(json!({"method": "password"})),
        ip.as_deref(),
        user_agent(&headers).as_deref(),
    )
    .await;

    let csrf_token = uuid::Uuid::new_v4().simple().to_string();
    let secure = state.config.is_production();
    let mut response = envelope::ok(json!({
        "token": access_token,
        "refreshToken": refresh_token,
        "sessionId": session_id,
        "expiresIn": tokens::ACCESS_TTL_SECS,
        "agent": agent_view(&agent),
    }));
    for cookie_str in cookies::auth_cookies(&access_token, &refresh_token, &csrf_token, secure) {
        if let Ok(hv) = HeaderValue::from_str(&cookie_str) {
            response.headers_mut().append(axum::http::header::SET_COOKIE, hv);
        }
    }
    Ok(response)
}

// ------------------------------------------------------- Create Account (CRD 145-153)

#[derive(Deserialize)]
pub struct RegisterBody {
    pub email: Option<String>,
    pub password: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub role: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<AuthUser>,
    Extension(client_ip): Extension<TrustedClientIp>,
    headers: HeaderMap,
    Json(body): Json<RegisterBody>,
) -> Result {
    let ip = client_ip.0;
    if !caller.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let email = body.email.as_deref().unwrap_or("").trim().to_string();
    let password = body.password.as_deref().unwrap_or("").to_string();
    let display_name = body.display_name.as_deref().unwrap_or("").trim().to_string();
    let role = body.role.as_deref().unwrap_or("").to_string();
    if email.is_empty() || password.is_empty() || display_name.is_empty() || role.is_empty() {
        return Err(AppError::BadRequest(
            "email, password, displayName and role are required".into(),
        ));
    }
    if role != "admin" && role != "agent" {
        return Err(AppError::BadRequest("role must be one of: admin, agent".into()));
    }
    if store::find_active_agent_by_email(&state.db, &email).await?.is_some() {
        return Err(AppError::Conflict("An account with this email already exists".into()));
    }

    let hash = store::hash_password(&password)
        .map_err(|e| AppError::Internal(format!("password hashing failed: {e}")))?;
    let now = crate::db::now_iso();

    // Soft-deleted same-email account is reactivated in place (CRD 149).
    let user_id = if let Some(old) = store::find_deleted_agent_by_email(&state.db, &email).await? {
        sqlx::query("DELETE FROM team_members WHERE agent_id = $1")
            .bind(&old.id)
            .execute(&state.db)
            .await?;
        sqlx::query(
            "UPDATE agents SET password_hash = $1, display_name = $2, role = $3, is_active = 1,
             password_policy = 'changeable', deleted_at = NULL, updated_at = $4 WHERE id = $5",
        )
        .bind(&hash)
        .bind(&display_name)
        .bind(&role)
        .bind(&now)
        .bind(&old.id)
        .execute(&state.db)
        .await?;
        old.id
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO agents (id, email, password_hash, display_name, role, is_active, created_at)
             VALUES ($1, $2, $3, $4, $5, 1, $6)",
        )
        .bind(&id)
        .bind(&email)
        .bind(&hash)
        .bind(&display_name)
        .bind(&role)
        .bind(&now)
        .execute(&state.db)
        .await?;
        id
    };

    let mut team_name: Option<String> = None;
    if let Some(team_id) = body.team_id {
        team_name = store::team_name(&state.db, team_id).await?;
        if team_name.is_none() {
            return Err(AppError::BadRequest(format!("Team {team_id} not found")));
        }
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at) VALUES ($1, $2, 'member', 1, $3)",
        )
        .bind(&user_id)
        .bind(team_id)
        .bind(&now)
        .execute(&state.db)
        .await?;
    }
    state.team_cache.invalidate(&user_id);

    store::log_activity(
        &state.db,
        &caller.id,
        &caller.display_name,
        &caller.role,
        "create_user",
        "agent",
        Some(&user_id),
        Some(json!({
            "email": email, "displayName": display_name, "role": role,
            "teamId": body.team_id, "reversible": true,
        })),
        ip.as_deref(),
        user_agent(&headers).as_deref(),
    )
    .await;

    Ok(envelope::ok(json!({
        "user": {
            "id": user_id,
            "email": email,
            "displayName": display_name,
            "role": role,
            "teamId": body.team_id,
            "teamName": team_name,
        }
    })))
}

// ---------------------------------------------------------------- Sign Out (CRD 155-163)

#[derive(Deserialize, Default)]
pub struct LogoutBody {
    #[serde(rename = "refreshToken")]
    pub refresh_token: Option<String>,
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    Extension(client_ip): Extension<TrustedClientIp>,
    headers: HeaderMap,
    body: Option<Json<LogoutBody>>,
) -> Result {
    let ip = client_ip.0;
    let session_id = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("Session ID required".into()))?;
    let agent_id = store::lookup_session(&state.db, session_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("Invalid or expired session".into()))?;
    let agent = store::find_agent_by_id(&state.db, &agent_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("Invalid or expired session".into()))?;

    store::delete_session(&state.db, session_id).await?;

    // Best-effort revocations (CRD 159).
    if let Some(token) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        if let Ok(claims) = tokens::verify(token, &state.config.jwt_secret) {
            let _ = store::revoke_jti(&state.db, &claims.jti, Some(&claims.sub), Some(claims.exp)).await;
        }
    }
    // Cookie-auth clients: revoke the access token carried by the HttpOnly cookie.
    if let Some(at) = cookies::cookie_value(&headers, "mcss_access") {
        if let Ok(claims) = tokens::verify(&at, &state.config.jwt_secret) {
            let _ = store::revoke_jti(&state.db, &claims.jti, Some(&claims.sub), Some(claims.exp)).await;
        }
    }
    let rt_opt = body
        .and_then(|Json(b)| b.refresh_token)
        .or_else(|| cookies::cookie_value(&headers, "mcss_refresh"));
    if let Some(rt) = rt_opt {
        if let Ok(claims) = tokens::verify(&rt, &state.config.jwt_secret) {
            // Only revoked when it belongs to the same signed-in user (CRD 159).
            if claims.token_type == "refresh" && claims.sub == agent_id {
                let _ = store::revoke_refresh_token(&state.db, &claims.jti).await;
                let _ = store::revoke_jti(&state.db, &claims.jti, Some(&claims.sub), Some(claims.exp)).await;
            }
        }
    }

    store::log_activity(
        &state.db, &agent.id, &agent.display_name, &agent.role,
        "logout", "auth", None, None,
        ip.as_deref(), user_agent(&headers).as_deref(),
    )
    .await;

    let secure = state.config.is_production();
    let mut response = envelope::message_only("Logged out successfully");
    for cookie_str in cookies::clear_auth_cookies(secure) {
        if let Ok(hv) = HeaderValue::from_str(&cookie_str) {
            response.headers_mut().append(axum::http::header::SET_COOKIE, hv);
        }
    }
    Ok(response)
}

// ---------------------------------------------------------- Renew Credentials (CRD 165-172)

#[derive(Deserialize)]
pub struct RefreshBody {
    #[serde(rename = "refreshToken")]
    pub refresh_token: Option<String>,
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Extension(client_ip): Extension<TrustedClientIp>,
    headers: HeaderMap,
    Json(body): Json<RefreshBody>,
) -> Result {
    let ip = client_ip.0;
    let raw = body
        .refresh_token
        .filter(|t| !t.is_empty())
        .or_else(|| cookies::cookie_value(&headers, "mcss_refresh"))
        .ok_or_else(|| AppError::BadRequest("refreshToken is required".into()))?;

    let claims = tokens::verify(&raw, &state.config.jwt_secret)
        .map_err(|_| AppError::Unauthorized("Invalid or expired refresh token".into()))?;
    if claims.token_type != "refresh" {
        return Err(AppError::Unauthorized("Invalid token type".into()));
    }

    // Replay/validation state read fails closed (CRD 171).
    let row = store::get_refresh_token(&state.db, &claims.jti)
        .await
        .map_err(|_| AppError::Unauthorized("Unable to validate refresh token".into()))?
        .ok_or_else(|| AppError::Unauthorized("Invalid or expired refresh token".into()))?;

    if row.consumed_at.is_some() || row.revoked_at.is_some() {
        // Reuse detected: revoke and force a fresh sign-in (CRD 169).
        let _ = store::revoke_refresh_token(&state.db, &claims.jti).await;
        let _ = store::revoke_jti(&state.db, &claims.jti, Some(&claims.sub), Some(claims.exp)).await;
        tracing::warn!(
            user = %claims.sub,
            ip = ?ip,
            user_agent = ?user_agent(&headers),
            "refresh token reuse detected"
        );
        return Err(AppError::Unauthorized(
            "Refresh token reuse detected - please log in again".into(),
        ));
    }

    let agent = store::find_agent_by_id(&state.db, &claims.sub)
        .await?
        .filter(|a| a.is_active != 0)
        .ok_or_else(|| AppError::Unauthorized("Account not found or inactive".into()))?;
    if token_issued_before_valid_after(claims.iat, &agent.tokens_valid_after) {
        return Err(AppError::Unauthorized("Refresh token is no longer valid".into()));
    }

    // Team data re-derived from authoritative storage (CRD 169).
    let teams = store::memberships(&state.db, &agent.id).await?;
    store::consume_refresh_token(&state.db, &claims.jti).await?;
    let (access_token, refresh_token) = issue_token_pair(&state, &agent, &teams).await?;

    let csrf_token = uuid::Uuid::new_v4().simple().to_string();
    let secure = state.config.is_production();
    let mut response = envelope::ok(json!({
        "token": access_token,
        "refreshToken": refresh_token,
    }));
    for cookie_str in cookies::auth_cookies(&access_token, &refresh_token, &csrf_token, secure) {
        if let Ok(hv) = HeaderValue::from_str(&cookie_str) {
            response.headers_mut().append(axum::http::header::SET_COOKIE, hv);
        }
    }
    Ok(response)
}

// ------------------------------------------------ Profile & current user (CRD 174-197)

pub async fn profile(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    let agent = store::find_agent_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("Account not found".into()))?;
    let team_name = match user.primary_team_id {
        Some(id) => store::team_name(&state.db, id).await?,
        None => None,
    };
    Ok(envelope::ok(json!({
        "user": {
            "id": agent.id,
            "email": agent.email,
            "displayName": agent.display_name,
            "role": agent.role,
            "teamId": user.primary_team_id,
            "teamName": team_name,
            "isActive": agent.is_active != 0,
            "createdAt": agent.created_at,
            "updatedAt": agent.updated_at,
        }
    })))
}

pub async fn me(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    let agent = store::find_agent_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("Account not found".into()))?;
    Ok(envelope::ok(agent_view(&agent)))
}

#[derive(Deserialize)]
pub struct UpdateMeBody {
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

pub async fn update_me(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(client_ip): Extension<TrustedClientIp>,
    headers: HeaderMap,
    Json(body): Json<UpdateMeBody>,
) -> Result {
    let ip = client_ip.0;
    // Strict allowlist: only displayName is self-service editable (CRD 192).
    let Some(raw) = body.display_name else {
        return Err(AppError::BadRequest("No updatable field provided".into()));
    };
    let name = raw.trim().to_string();
    if name.is_empty() || name.chars().count() > 50 {
        return Err(AppError::BadRequest(
            "displayName must be between 1 and 50 characters".into(),
        ));
    }
    let agent = store::find_agent_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".into()))?;

    if agent.display_name == name {
        return Ok(envelope::ok_msg(agent_view(&agent), "No changes"));
    }

    sqlx::query("UPDATE agents SET display_name = $1, updated_at = $2 WHERE id = $3")
        .bind(&name)
        .bind(crate::db::now_iso())
        .bind(&user.id)
        .execute(&state.db)
        .await?;

    store::log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "update_profile", "agent", Some(&user.id),
        Some(json!({
            "selfService": true, "reversible": true,
            "old": {"displayName": agent.display_name},
            "new": {"displayName": name},
        })),
        ip.as_deref(), user_agent(&headers).as_deref(),
    )
    .await;

    let updated = store::find_agent_by_id(&state.db, &user.id).await?.unwrap_or(agent);
    Ok(envelope::ok_msg(agent_view(&updated), "Profile updated"))
}

// ------------------------------------------------- Change Own Password (CRD 199-206)

#[derive(Deserialize)]
pub struct ChangePasswordBody {
    #[serde(rename = "currentPassword")]
    pub current_password: Option<String>,
    #[serde(rename = "newPassword")]
    pub new_password: Option<String>,
}

pub async fn change_password(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(client_ip): Extension<TrustedClientIp>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> Result {
    let ip = client_ip.0;
    let current = body.current_password.unwrap_or_default();
    let new = body.new_password.unwrap_or_default();
    if current.is_empty() || new.is_empty() {
        return Err(AppError::BadRequest(
            "currentPassword and newPassword are required".into(),
        ));
    }
    let agent = store::find_agent_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".into()))?;

    if !store::verify_password(&current, &agent.password_hash) {
        store::log_activity(
            &state.db, &user.id, &user.display_name, &user.role,
            "change_password_failed", "agent", Some(&user.id),
            Some(json!({"reason": "wrong current password", "security": true})),
            ip.as_deref(), user_agent(&headers).as_deref(),
        )
        .await;
        return Err(AppError::Unauthorized("Current password is incorrect".into()));
    }

    let hash = store::hash_password(&new)
        .map_err(|e| AppError::Internal(format!("password hashing failed: {e}")))?;
    let now = crate::db::now_iso();
    sqlx::query(
        "UPDATE agents SET password_hash = $1, password_policy = 'changeable',
         tokens_valid_after = $2, updated_at = $2 WHERE id = $3",
    )
    .bind(&hash)
    .bind(&now)
    .bind(&user.id)
    .execute(&state.db)
    .await?;
    store::revoke_user_credentials(&state.db, &user.id, &now).await?;

    store::log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "change_password", "agent", Some(&user.id), None,
        ip.as_deref(), user_agent(&headers).as_deref(),
    )
    .await;

    Ok(envelope::message_only("Password changed successfully"))
}

// --------------------------------------------- Reset Member Password (CRD 208-215)

#[derive(Deserialize)]
pub struct ResetPasswordBody {
    #[serde(rename = "newPassword")]
    pub new_password: Option<String>,
    pub policy: Option<String>,
}

pub async fn reset_member_password(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(member_id): Path<String>,
    Json(body): Json<ResetPasswordBody>,
) -> Result {
    if !is_manager_or_admin(&user) {
        return Err(AppError::Forbidden("Manager or administrator role required".into()));
    }
    let new = body.new_password.unwrap_or_default();
    if new.is_empty() {
        return Err(AppError::BadRequest("newPassword is required".into()));
    }
    if member_id == user.id {
        return Err(AppError::Forbidden(
            "Use the self-service change-password endpoint to change your own password".into(),
        ));
    }
    if let Some(p) = body.policy.as_deref() {
        if !["changeable", "unchangeable", "must_change"].contains(&p) {
            return Err(AppError::BadRequest(
                "policy must be one of: changeable, unchangeable, must_change".into(),
            ));
        }
    }
    let target = store::find_agent_by_id(&state.db, &member_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    let hash = store::hash_password(&new)
        .map_err(|e| AppError::Internal(format!("password hashing failed: {e}")))?;
    let policy = body.policy.unwrap_or_else(|| target.password_policy.clone());
    let now = crate::db::now_iso();
    sqlx::query(
        "UPDATE agents SET password_hash = $1, password_policy = $2,
         tokens_valid_after = $3, updated_at = $3 WHERE id = $4",
    )
    .bind(&hash)
    .bind(&policy)
    .bind(&now)
    .bind(&member_id)
    .execute(&state.db)
    .await?;
    store::revoke_user_credentials(&state.db, &member_id, &now).await?;

    Ok(envelope::ok_msg(
        json!({"passwordPolicy": policy}),
        "Password reset successfully",
    ))
}

// ------------------------------------------------------ /phase2-auth (CRD 217-263)

#[derive(Deserialize)]
pub struct ExpiresQuery {
    #[serde(rename = "expiresIn")]
    pub expires_in: Option<i64>,
}

pub async fn monitoring_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ExpiresQuery>,
) -> Result {
    require_admin_access(&user)?;
    let expires_in = q.expires_in.unwrap_or(604_800);
    if !(3600..=2_592_000).contains(&expires_in) {
        return Err(AppError::BadRequest(
            "expiresIn must be between 3600 and 2592000 seconds".into(),
        ));
    }
    let mut claims = Claims::new("system-monitoring", "admin", "monitoring", expires_in);
    claims.monitoring = Some(true);
    claims.service_root_iat = Some(claims.iat);
    let token = tokens::sign(&claims, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(format!("token signing failed: {e}")))?;
    Ok(envelope::ok(json!({
        "token": token,
        "type": "monitoring",
        "expiresIn": expires_in,
        "expiresAt": chrono::DateTime::from_timestamp(claims.exp, 0).map(|t| t.to_rfc3339()),
        "issuedBy": user.id,
    })))
}

#[derive(Deserialize)]
pub struct UserTokenBody {
    #[serde(rename = "targetUserId")]
    pub target_user_id: Option<String>,
    #[serde(rename = "expiresIn")]
    pub expires_in: Option<i64>,
}

pub async fn user_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<UserTokenBody>,
) -> Result {
    require_admin_access(&user)?;
    let target_id = body
        .target_user_id
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::BadRequest("targetUserId is required".into()))?;
    let expires_in = body.expires_in.unwrap_or(3600);
    if !(300..=86_400).contains(&expires_in) {
        return Err(AppError::BadRequest(
            "expiresIn must be between 300 and 86400 seconds".into(),
        ));
    }
    let target = store::find_agent_by_id(&state.db, &target_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Target user not found".into()))?;
    let teams = store::memberships(&state.db, &target.id).await?;
    let primary = teams.iter().find(|t| t.is_primary).map(|t| t.team_id);

    let mut claims = Claims::new(&target.id, &target.role, "user", expires_in);
    claims.email = Some(target.email.clone());
    claims.name = Some(target.display_name.clone());
    claims.primary_team_id = primary;
    claims.service_root_iat = Some(claims.iat);
    let token = tokens::sign(&claims, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(format!("token signing failed: {e}")))?;
    Ok(envelope::ok(json!({
        "token": token,
        "type": "user",
        "user": {
            "id": target.id,
            "displayName": target.display_name,
            "role": target.role,
            "teamId": primary,
        },
        "expiresIn": expires_in,
        "expiresAt": chrono::DateTime::from_timestamp(claims.exp, 0).map(|t| t.to_rfc3339()),
        "issuedBy": user.id,
    })))
}

#[derive(Deserialize)]
pub struct BatchTokensBody {
    pub users: Option<serde_json::Value>,
    #[serde(rename = "expiresIn")]
    pub expires_in: Option<i64>,
}

pub async fn batch_tokens(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<BatchTokensBody>,
) -> Result {
    require_admin_access(&user)?;
    if state.config.is_production() {
        return Err(AppError::Forbidden(
            "Batch token issuance is not permitted in production".into(),
        ));
    }
    let users = body
        .users
        .as_ref()
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::BadRequest("users must be a non-empty array".into()))?
        .clone();
    if users.is_empty() {
        return Err(AppError::BadRequest("users must be a non-empty array".into()));
    }
    if users.len() > 10 {
        return Err(AppError::BadRequest("users may contain at most 10 entries".into()));
    }
    let expires_in = body.expires_in.unwrap_or(3600);

    let mut issued = Vec::new();
    for u in &users {
        let id = u
            .get("id")
            .or_else(|| u.get("userId"))
            .and_then(|v| v.as_str())
            .unwrap_or("test-user")
            .to_string();
        let role = u.get("role").and_then(|v| v.as_str()).unwrap_or("agent").to_string();
        let mut claims = Claims::new(&id, &role, "user", expires_in);
        claims.name = u.get("displayName").and_then(|v| v.as_str()).map(|s| s.to_string());
        let token = tokens::sign(&claims, &state.config.jwt_secret)
            .map_err(|e| AppError::Internal(format!("token signing failed: {e}")))?;
        issued.push(json!({"userId": id, "role": role, "token": token}));
    }

    Ok(envelope::ok_msg(
        json!({
            "tokens": issued,
            "count": issued.len(),
            "expiresIn": expires_in,
            "issuedBy": user.id,
            "warning": "Development-only credentials. Do not use in production.",
        }),
        "Batch tokens issued",
    ))
}

#[derive(Deserialize)]
pub struct VerifyTokenBody {
    pub token: Option<String>,
}

pub async fn verify_token(
    State(state): State<Arc<AppState>>,
    Json(body): Json<VerifyTokenBody>,
) -> Result {
    let raw = body
        .token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::BadRequest("token is required".into()))?;
    // Does not consult revocation state (CRD 245).
    match tokens::verify(&raw, &state.config.jwt_secret) {
        Ok(claims) => {
            let remaining = (claims.exp - chrono::Utc::now().timestamp()).max(0);
            Ok(envelope::ok(json!({
                "valid": true,
                "payload": {
                    "userId": claims.sub,
                    "displayName": claims.name,
                    "role": claims.role,
                    "teamId": claims.primary_team_id,
                    "systemToken": claims.monitoring.unwrap_or(false),
                },
                "expiresAt": chrono::DateTime::from_timestamp(claims.exp, 0).map(|t| t.to_rfc3339()),
                "remainingSeconds": remaining,
                "expiringSoon": remaining < 3600,
            })))
        }
        Err(e) => Ok(envelope::ok(json!({
            "valid": false,
            "error": format!("Invalid token: {e}"),
        }))),
    }
}

#[derive(Deserialize)]
pub struct RefreshServiceTokenBody {
    pub token: Option<String>,
}

pub async fn refresh_service_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<RefreshServiceTokenBody>,
) -> Result {
    require_admin_access(&user)?;
    let raw = body
        .token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::BadRequest("token is required".into()))?;
    let claims = tokens::verify(&raw, &state.config.jwt_secret)
        .map_err(|_| AppError::BadRequest("Invalid or expired token".into()))?;
    if store::is_jti_revoked(&state.db, &claims.jti)
        .await
        .map_err(|_| {
            AppError::ServiceUnavailable(
                "Unable to verify token revocation state".into(),
                "REVOCATION_CHECK_FAILED",
            )
        })?
    {
        return Err(AppError::Unauthorized("Token has been revoked".into()));
    }

    let root_iat = claims.service_root_iat.unwrap_or(claims.iat);
    let now = chrono::Utc::now().timestamp();
    let (new_claims, kind) = if claims.token_type == "monitoring" && claims.monitoring.unwrap_or(false) {
        let ttl = bounded_service_ttl(
            now,
            root_iat,
            MONITORING_TOKEN_REFRESH_TTL_SECS,
            MONITORING_TOKEN_MAX_LIFETIME_SECS,
        )
        .ok_or_else(|| AppError::Unauthorized("Token maximum lifetime exceeded".into()))?;
        let mut c = Claims::new(&claims.sub, &claims.role, "monitoring", ttl);
        c.monitoring = Some(true);
        c.service_root_iat = Some(root_iat);
        (c, "monitoring")
    } else if claims.token_type == "user" {
        let ttl = bounded_service_ttl(
            now,
            root_iat,
            USER_TOKEN_REFRESH_TTL_SECS,
            USER_TOKEN_MAX_LIFETIME_SECS,
        )
        .ok_or_else(|| AppError::Unauthorized("Token maximum lifetime exceeded".into()))?;
        let mut c = Claims::new(&claims.sub, &claims.role, "user", ttl);
        c.email = claims.email.clone();
        c.name = claims.name.clone();
        c.primary_team_id = claims.primary_team_id;
        c.service_root_iat = Some(root_iat);
        (c, "user")
    } else {
        return Err(AppError::BadRequest("Only monitoring or user tokens can be refreshed".into()));
    };
    store::revoke_jti(&state.db, &claims.jti, Some(&claims.sub), Some(claims.exp)).await?;
    let token = tokens::sign(&new_claims, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(format!("token signing failed: {e}")))?;
    Ok(envelope::ok_msg(
        json!({"token": token, "type": kind}),
        "Token refreshed",
    ))
}

pub async fn auth_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    let team_name = match user.primary_team_id {
        Some(id) => store::team_name(&state.db, id).await?,
        None => None,
    };
    let admin = user.is_admin();
    Ok(envelope::ok(json!({
        "authenticated": true,
        "user": {
            "id": user.id,
            "displayName": user.display_name,
            "role": user.role,
            "teamId": user.primary_team_id,
            "teamName": team_name,
        },
        "permissions": {
            "canIssueMonitoringTokens": admin,
            "canIssueUserTokens": admin,
            "canAccessAnalytics": admin,
            "canTriggerAlerts": admin,
        },
    })))
}

#[cfg(test)]
mod tests {
    use super::bounded_service_ttl;

    #[test]
    fn bounded_service_ttl_caps_refresh_to_absolute_lifetime() {
        assert_eq!(bounded_service_ttl(90, 0, 60, 100), Some(10));
    }

    #[test]
    fn bounded_service_ttl_allows_full_refresh_inside_lifetime() {
        assert_eq!(bounded_service_ttl(10, 0, 60, 100), Some(60));
    }

    #[test]
    fn bounded_service_ttl_rejects_after_absolute_lifetime() {
        assert_eq!(bounded_service_ttl(100, 0, 60, 100), None);
        assert_eq!(bounded_service_ttl(101, 0, 60, 100), None);
    }
}

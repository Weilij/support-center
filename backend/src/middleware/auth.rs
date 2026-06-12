//! Bearer-token request gate per CRD §1.3 (lines 492-515) and §1.1 (lines 265-273).

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use std::time::Duration;

use crate::domain::auth::{store, tokens};
use crate::error::AppError;
use crate::state::{AppState, TeamMembership};

pub const TEAM_CACHE_TTL: Duration = Duration::from_secs(60);
const LAST_ACTIVE_INTERVAL: Duration = Duration::from_secs(60);

/// Authenticated caller context inserted as a request extension by `require_auth`.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub primary_team_id: Option<i64>,
    pub teams: Vec<TeamMembership>,
    pub jti: Option<String>,
    pub token_type: String,
    pub context_team_id: Option<i64>,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    pub fn team_role(&self, team_id: i64) -> Option<&str> {
        self.teams
            .iter()
            .find(|t| t.team_id == team_id)
            .map(|t| t.role.as_str())
    }

    pub fn can_access_team(&self, team_id: i64) -> bool {
        self.is_admin() || self.team_role(team_id).is_some()
    }
}

pub fn team_role_level(role: &str) -> u8 {
    match role {
        "supervisor" => 3,
        "lead" => 2,
        "member" => 1,
        _ => 0,
    }
}

/// Manager-or-administrator level (used e.g. by the member password reset, CRD line 211):
/// system admin, or lead/supervisor in at least one team.
pub fn is_manager_or_admin(user: &AuthUser) -> bool {
    user.is_admin() || user.teams.iter().any(|t| team_role_level(&t.role) >= 2)
}

pub async fn authenticate(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<AuthUser, AppError> {
    let header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("Authentication required".into()))?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or_else(|| AppError::Unauthorized("Authentication required".into()))?;

    let claims = tokens::verify(token, &state.config.jwt_secret)
        .map_err(|_| AppError::Unauthorized("Invalid or expired token".into()))?;

    // A renewal credential cannot directly access protected resources (CRD line 268).
    if claims.token_type == "refresh" {
        return Err(AppError::Unauthorized("Invalid token type".into()));
    }

    // Revocation check fails closed (CRD line 269).
    match store::is_jti_revoked(&state.db, &claims.jti).await {
        Ok(true) => return Err(AppError::Unauthorized("Token has been revoked".into())),
        Ok(false) => {}
        Err(_) => {
            return Err(AppError::ServiceUnavailable(
                "Unable to verify token revocation state".into(),
                "REVOCATION_CHECK_FAILED",
            ))
        }
    }

    let agent = store::find_agent_by_id(&state.db, &claims.sub)
        .await?
        .filter(|a| a.is_active != 0)
        .ok_or_else(|| AppError::Unauthorized("Account is inactive or not found".into()))?;

    // Memberships re-derived from authoritative storage with a brief cache (CRD line 270).
    let teams = match state.team_cache.get(&agent.id, TEAM_CACHE_TTL) {
        Some(t) => t,
        None => {
            let t = store::memberships(&state.db, &agent.id).await?;
            state.team_cache.put(&agent.id, t.clone());
            t
        }
    };

    // Optional team-context header (CRD line 271, 400 on non-numeric, 403 on non-member).
    let context_team_id = match headers
        .get("x-context-team-id")
        .and_then(|v| v.to_str().ok())
    {
        None => None,
        Some(raw) => {
            let id: i64 = raw
                .parse()
                .map_err(|_| AppError::BadRequest("Invalid team ID: must be numeric".into()))?;
            let allowed = agent.role == "admin" || teams.iter().any(|t| t.team_id == id);
            if !allowed {
                return Err(AppError::Forbidden(
                    "You do not have access to the requested team".into(),
                ));
            }
            Some(id)
        }
    };

    // Debounced last-active persistence (CRD line 273).
    if state.last_active.should_persist(&agent.id, LAST_ACTIVE_INTERVAL) {
        let db = state.db.clone();
        let id = agent.id.clone();
        tokio::spawn(async move {
            let _ = sqlx::query("UPDATE agents SET last_active_at = $1 WHERE id = $2")
                .bind(crate::db::now_iso())
                .bind(id)
                .execute(&db)
                .await;
        });
    }

    let primary_team_id = teams.iter().find(|t| t.is_primary).map(|t| t.team_id);
    Ok(AuthUser {
        id: agent.id,
        email: agent.email,
        display_name: agent.display_name,
        role: agent.role,
        primary_team_id,
        teams,
        jti: Some(claims.jti),
        token_type: claims.token_type,
        context_team_id,
    })
}

/// Middleware: require a valid access credential; inserts `AuthUser` extension.
pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let headers = req.headers().clone();
    match authenticate(&state, &headers).await {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

/// Optional authentication gate (CRD lines 523-529): attaches `AuthUser` when a valid
/// credential is present; anonymous requests proceed without one. A *malformed* credential
/// is still rejected rather than silently downgraded.
pub async fn optional_auth(
    State(state): State<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    if req.headers().get("authorization").is_none() {
        return next.run(req).await;
    }
    let headers = req.headers().clone();
    match authenticate(&state, &headers).await {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

/// System-to-system key gate (CRD lines 530-536): requires a matching X-System-Key header.
/// The expected key is the SYSTEM_API_KEY environment setting; absent configuration denies all.
pub async fn require_system_key(req: Request<Body>, next: Next) -> Response {
    let expected = std::env::var("SYSTEM_API_KEY").ok().filter(|k| !k.is_empty());
    let presented = req
        .headers()
        .get("x-system-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    match (expected, presented) {
        (Some(e), Some(p)) if e == p => next.run(req).await,
        _ => AppError::Unauthorized("Valid system key required".into()).into_response(),
    }
}

/// Middleware: require a valid access credential AND the `admin` system role.
pub async fn require_admin(
    State(state): State<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let headers = req.headers().clone();
    match authenticate(&state, &headers).await {
        Ok(user) if user.is_admin() => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Ok(_) => AppError::Forbidden("Administrator role required".into()).into_response(),
        Err(e) => e.into_response(),
    }
}

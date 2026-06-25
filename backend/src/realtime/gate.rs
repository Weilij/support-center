//! Real-time connection gate (WebSocket upgrade) — CRD §1.3 lines 596-646 and
//! §5.1 lines 3230-3258. The credential travels as a `token` query parameter;
//! every rejection carries a JSON body with `error` label, numeric `code`,
//! a timestamp, a suggested next `action`, and `X-Error-Code` /
//! `X-WebSocket-Close-Code` headers.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::domain::auth::{
    store,
    tokens::{Claims, AUDIENCE, ISSUER},
};
use crate::middleware::auth::TEAM_CACHE_TTL;
use crate::state::AppState;

use super::hub::ConnIdentity;

/// Tokens expiring within this margin are rejected outright (CRD 605).
const EXPIRY_REJECT_MARGIN_SECS: i64 = 30;
/// Tokens expiring within this margin are allowed but flagged (CRD 605).
const EXPIRY_WARN_MARGIN_SECS: i64 = 300;

pub struct GateOutcome {
    pub identity: ConnIdentity,
    /// Credential expiry (epoch seconds) — drives the scheduled forced close.
    pub exp: i64,
}

/// Build one structured gate rejection (CRD 612-622).
pub fn gate_error(
    status: StatusCode,
    code: u16,
    label: &str,
    message: &str,
    action: &str,
    extra: Option<Map<String, Value>>,
) -> Response {
    let mut body = json!({
        "success": false,
        "error": label,
        "message": message,
        "code": code,
        "action": action,
        "timestamp": crate::db::now_iso(),
    });
    if let Some(extra) = extra {
        for (k, v) in extra {
            body[k] = v;
        }
    }
    let mut resp = (status, Json(body)).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = code.to_string().parse() {
        headers.insert("X-Error-Code", v);
    }
    // The suggested websocket close code mirrors the gate code (CRD 644).
    if let Ok(v) = code.to_string().parse() {
        headers.insert("X-WebSocket-Close-Code", v);
    }
    resp
}

fn unexpected_failure(state: &Arc<AppState>) -> Response {
    // Best-effort error analytics record (CRD 645) — never alters the result.
    let db = state.db.clone();
    tokio::spawn(async move {
        let _ = sqlx::query(
            "INSERT INTO realtime_error_events (id, timestamp, error_code, error_type, details, created_at)
             VALUES ($1, $2, '4500', 'AUTH_SYSTEM_ERROR', NULL, $3)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(crate::db::now_iso())
        .bind(crate::db::now_iso())
        .execute(&db)
        .await;
    });
    gate_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        4500,
        "AUTH_SYSTEM_ERROR",
        "Authentication system error",
        "retry_with_new_token",
        None,
    )
}

/// Validate the handshake credential and (for agents) conversation access.
/// Observable order per CRD 600-610.
pub async fn authorize(
    state: &Arc<AppState>,
    token: Option<&str>,
    conversation_id: Option<&str>,
) -> Result<GateOutcome, Box<Response>> {
    // 1. Missing token (CRD 613).
    let token = match token.filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => {
            return Err(Box::new(gate_error(
                StatusCode::UNAUTHORIZED,
                4401,
                "NO_TOKEN",
                "Authentication token is required",
                "provide_token",
                None,
            )))
        }
    };

    // 2. Malformed shape: must be three dot-separated segments (CRD 614).
    if token.split('.').count() != 3 {
        return Err(Box::new(gate_error(
            StatusCode::UNAUTHORIZED,
            4402,
            "INVALID_TOKEN_FORMAT",
            "Token must be a three-segment signed credential",
            "provide_token",
            None,
        )));
    }

    // 3. Cryptographic verification, expiry checked separately so the expired
    //    case can echo expiry details (CRD 615-616).
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[AUDIENCE]);
    let claims = match decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &validation,
    ) {
        Ok(d) => d.claims,
        Err(_) => {
            return Err(Box::new(gate_error(
                StatusCode::UNAUTHORIZED,
                4403,
                "INVALID_TOKEN",
                "Token verification failed",
                "retry_with_new_token",
                None,
            )))
        }
    };

    // 4. Already expired (CRD 616): echoes expiry and current time.
    let now = chrono::Utc::now().timestamp();
    if claims.exp <= now {
        let mut extra = Map::new();
        extra.insert("expiredAt".into(), json!(claims.exp));
        extra.insert("currentTime".into(), json!(now));
        return Err(Box::new(gate_error(
            StatusCode::UNAUTHORIZED,
            4404,
            "TOKEN_EXPIRED",
            "Token has expired",
            "refresh_token",
            Some(extra),
        )));
    }

    // 5. Expiring within the safety margin (CRD 617); within five minutes is
    //    allowed but flagged (CRD 605).
    let remaining = claims.exp - now;
    if remaining < EXPIRY_REJECT_MARGIN_SECS {
        let mut extra = Map::new();
        extra.insert("secondsRemaining".into(), json!(remaining));
        return Err(Box::new(gate_error(
            StatusCode::UNAUTHORIZED,
            4405,
            "TOKEN_EXPIRING_SOON",
            "Token expires too soon; refresh before connecting",
            "refresh_token",
            Some(extra),
        )));
    }
    if remaining < EXPIRY_WARN_MARGIN_SECS {
        tracing::warn!(user = %claims.sub, seconds = remaining, "websocket credential expiring soon");
    }

    // 6. Account identifier must be present and non-zero (CRD 618).
    if claims.sub.trim().is_empty() || claims.sub == "0" {
        return Err(Box::new(gate_error(
            StatusCode::UNAUTHORIZED,
            4406,
            "INVALID_USER_DATA",
            "Token does not carry a usable account identifier",
            "contact_admin",
            None,
        )));
    }

    // 7. System role: admin or agent only; a missing role defaults to agent
    //    (CRD 607, 619).
    let role = if claims.role.trim().is_empty() {
        "agent".to_string()
    } else {
        claims.role.clone()
    };
    if role != "admin" && role != "agent" {
        let mut extra = Map::new();
        extra.insert("allowedRoles".into(), json!(["admin", "agent"]));
        return Err(Box::new(gate_error(
            StatusCode::UNAUTHORIZED,
            4407,
            "INVALID_ROLE",
            "Role is not permitted to open a realtime connection",
            "contact_admin",
            Some(extra),
        )));
    }

    // 8. Attach the verified identity (CRD 608). Inactive accounts are denied
    //    at every gate (CRD 626); mapped to INVALID_USER_DATA here since the
    //    gate's code taxonomy has no dedicated inactive-account code.
    let agent = match store::find_agent_by_id(&state.db, &claims.sub).await {
        Ok(a) => a,
        Err(_) => return Err(Box::new(unexpected_failure(state))),
    };
    let Some(agent) = agent.filter(|a| a.is_active != 0) else {
        return Err(Box::new(gate_error(
            StatusCode::UNAUTHORIZED,
            4406,
            "INVALID_USER_DATA",
            "Account is inactive or not found",
            "contact_admin",
            None,
        )));
    };
    let teams = match state.team_cache.get(&agent.id, TEAM_CACHE_TTL) {
        Some(t) => t,
        None => match store::memberships(&state.db, &agent.id).await {
            Ok(t) => {
                state.team_cache.put(&agent.id, t.clone());
                t
            }
            Err(_) => return Err(Box::new(unexpected_failure(state))),
        },
    };
    let identity = ConnIdentity {
        user_id: agent.id.clone(),
        email: agent.email.clone(),
        display_name: agent.display_name.clone(),
        role: agent.role.clone(),
        team_ids: teams.iter().map(|t| t.team_id).collect(),
    };

    // 9. Conversation access for agents (CRD 609): team-assigned conversations
    //    plus the unassigned shared pool, cached ~5 minutes. Admins bypass.
    if let Some(cid) = conversation_id.filter(|c| !c.is_empty()) {
        if identity.role != "admin" {
            let allowed = match state.realtime.cached_access(&identity.user_id, cid) {
                Some(v) => v,
                None => {
                    let team: Result<Option<Option<i64>>, sqlx::Error> =
                        sqlx::query_scalar("SELECT team_id FROM conversations WHERE id = $1")
                            .bind(cid)
                            .fetch_optional(&state.db)
                            .await;
                    let allowed = match team {
                        Ok(Some(None)) => true, // unassigned shared pool
                        Ok(Some(Some(team_id))) => identity.team_ids.contains(&team_id),
                        Ok(None) => false,
                        Err(_) => return Err(Box::new(unexpected_failure(state))),
                    };
                    state.realtime.cache_access(&identity.user_id, cid, allowed);
                    allowed
                }
            };
            if !allowed {
                return Err(Box::new(gate_error(
                    StatusCode::FORBIDDEN,
                    4403,
                    "CONVERSATION_ACCESS_DENIED",
                    "You do not have access to this conversation",
                    "contact_admin",
                    None,
                )));
            }
        }
    }

    Ok(GateOutcome {
        exp: claims.exp,
        identity,
    })
}

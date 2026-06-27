use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::domain::teams::store::{self, QrRow};
use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::{
    parse_team_id, require_admin, require_team_access, require_team_rank, team_exists, JsonBody,
};

pub async fn generate_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
    body: JsonBody<Value>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "supervisor")?;
    if !team_exists(&state, id).await? {
        return Err(AppError::NotFound("Team not found".into()));
    }
    // Missing/invalid body is tolerated (CRD 2079).
    let body = body.map(|Json(b)| b).unwrap_or(Value::Null);
    let campaign = body.get("campaignName").and_then(Value::as_str);
    let description = body.get("description").and_then(Value::as_str);
    let expires_at = body.get("expiresAt").and_then(Value::as_str);
    let max_uses = body.get("maxUses").and_then(Value::as_i64);

    let qr = store::create_join_qr(
        &state.db,
        &state.config,
        id,
        campaign,
        description,
        expires_at,
        max_uses,
    )
    .await?;
    Ok(envelope::with_status(
        StatusCode::CREATED,
        Some(store::qr_view(&qr)),
        Some("QR code generated"),
    ))
}

pub async fn list_qr_codes(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let rows: Vec<QrRow> = sqlx::query_as(
        "SELECT id, team_id, token, url, image_url, campaign, description, scan_count,
                max_scans, is_active, expires_at, created_at
         FROM qr_codes WHERE team_id = $1 ORDER BY created_at DESC, id",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(
        rows.iter().map(store::qr_view).collect::<Vec<_>>(),
    ))
}

/// Cached image (team record) or the latest active QR record; optionally caches back.
async fn resolve_team_qr(
    state: &Arc<AppState>,
    team_id: i64,
) -> Result<Option<(String, Option<String>, bool)>> {
    let cached: Option<Option<String>> =
        sqlx::query_scalar("SELECT qr_code_image FROM teams WHERE id = $1 AND deleted_at IS NULL")
            .bind(team_id)
            .fetch_optional(&state.db)
            .await?;
    let cached = cached.ok_or_else(|| AppError::NotFound("Team not found".into()))?;

    let latest: Option<QrRow> = sqlx::query_as(
        "SELECT id, team_id, token, url, image_url, campaign, description, scan_count,
                max_scans, is_active, expires_at, created_at
         FROM qr_codes WHERE team_id = $1 AND is_active = 1
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(team_id)
    .fetch_optional(&state.db)
    .await?;

    if let Some(image) = cached {
        return Ok(Some((image, latest.and_then(|qr| qr.url), true)));
    }
    let Some(qr) = latest else { return Ok(None) };
    let Some(image) = qr.image_url.clone() else {
        return Ok(None);
    };
    // Asynchronously cache the image back onto the team record (CRD 2092, 2098).
    let db = state.db.clone();
    let img = image.clone();
    tokio::spawn(async move {
        if let Err(error) = sqlx::query("UPDATE teams SET qr_code_image = $1 WHERE id = $2")
            .bind(&img)
            .bind(team_id)
            .execute(&db)
            .await
        {
            tracing::warn!(error = %error, team_id, "team QR image async cache update failed");
        }
    });
    Ok(Some((image, qr.url, false)))
}

pub async fn latest_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let Some((image, join_url, from_cache)) = resolve_team_qr(&state, id).await? else {
        return Err(AppError::NotFound("No QR code found for this team".into()));
    };
    Ok(envelope::ok(json!({
        "qrCodeImage": image,
        "joinUrl": join_url,
        "fromCache": from_cache,
    })))
}

pub async fn fast_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let Some((image, join_url, from_cache)) = resolve_team_qr(&state, id).await? else {
        return Err(AppError::NotFound("No QR code found for this team".into()));
    };
    Ok(envelope::ok(json!({
        "qrCodeImage": image,
        "joinUrl": join_url,
        "source": if from_cache { "cache" } else { "database" },
        "performance": if from_cache { "fast" } else { "fallback" },
    })))
}

pub async fn deactivate_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((raw_id, raw_qr_id)): Path<(String, String)>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_rank(&user, id, "supervisor")?;
    let qr_id = raw_qr_id.trim().to_string();
    if qr_id.is_empty() {
        return Err(AppError::BadRequest("QR code id is required".into()));
    }
    let res = sqlx::query(
        "UPDATE qr_codes SET is_active = 0, updated_at = $1 WHERE id = $2 AND team_id = $3",
    )
    .bind(now_iso())
    .bind(&qr_id)
    .bind(id)
    .execute(&state.db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("QR code not found".into()));
    }
    Ok(envelope::message_only("QR code deactivated"))
}

fn liff_view(liff: &store::LiffRow) -> Value {
    json!({
        "id": liff.id,
        "teamId": liff.team_id,
        "url": liff.url,
        "imageUrl": liff.image_url,
        "scanCount": liff.scan_count,
        "isActive": liff.is_active != 0,
        "createdAt": liff.created_at,
        "updatedAt": liff.updated_at,
    })
}

pub async fn get_liff_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let liff = store::find_liff(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("No LIFF QR code found for this team".into()))?;
    Ok(envelope::ok(liff_view(&liff)))
}

pub async fn generate_liff_qr(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    require_admin(&user)?;
    let id = parse_team_id(&raw_id)?;
    if !team_exists(&state, id).await? {
        return Err(AppError::NotFound("Team not found".into()));
    }
    let liff = store::upsert_liff(&state.db, id).await?;
    Ok(envelope::ok_msg(liff_view(&liff), "LIFF QR code generated"))
}

pub async fn liff_qr_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id = parse_team_id(&raw_id)?;
    require_team_access(&user, id)?;
    let liff = store::find_liff(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("No LIFF QR code found for this team".into()))?;
    let assignments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM customer_team_assignments WHERE liff_link_id = $1",
    )
    .bind(&liff.id)
    .fetch_one(&state.db)
    .await?;
    Ok(envelope::ok(json!({
        "scanCount": liff.scan_count,
        "customerAssignments": assignments,
        "createdAt": liff.created_at,
        "lastScanAt": null,
        "isActive": liff.is_active != 0,
    })))
}

/// Unauthenticated diagnostics endpoint; nothing is persisted (CRD 2126-2129).
pub async fn qr_code_test(Path(raw_id): Path<String>) -> Result {
    let id = parse_team_id(&raw_id)?;
    let token = uuid::Uuid::new_v4().to_string();
    Ok(envelope::ok(json!({
        "test": true,
        "teamId": id,
        "token": token,
        "imageUrl": store::qr_image_url(&token),
        "generatedAt": now_iso(),
    })))
}

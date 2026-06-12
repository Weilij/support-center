//! LIFF public + admin handlers (CRD §4.3).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

const AUTO_CLOSE_DELAY_MS: i64 = 2000;
const VERSION: &str = "1.0.0";
const DEFAULT_BOT_HANDLE: &str = "@support";

// ---------------------------------------------------------------- public ops

pub async fn health() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "status": "healthy",
            "module": "liff",
            "version": VERSION,
            "timestamp": now_iso(),
        })),
    )
        .into_response()
}

pub async fn config(State(state): State<Arc<AppState>>) -> Result {
    let Some(liff_id) = state.config.liff_id.clone() else {
        return Err(AppError::Internal(
            "LIFF application identifier is not configured".into(),
        ));
    };
    let bot = state
        .config
        .line_bot_id
        .clone()
        .unwrap_or_else(|| DEFAULT_BOT_HANDLE.into());
    Ok(envelope::ok(json!({
        "liffId": liff_id,
        "lineBotId": bot,
        "lineOaId": bot.trim_start_matches('@'),
        "apiEndpoint": state.config.backend_url.clone().unwrap_or_default(),
        "autoCloseDelay": AUTO_CLOSE_DELAY_MS,
        "version": VERSION,
    })))
}

pub async fn team_info(
    State(state): State<Arc<AppState>>,
    Path(team_id): Path<String>,
) -> Result {
    let team_id: i64 = team_id
        .parse()
        .map_err(|_| AppError::BadRequest("無效的團隊編號".into()))?;
    let row: Option<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, name, description FROM teams WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(team_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((id, name, description)) = row else {
        return Err(AppError::NotFound("找不到團隊".into()));
    };
    Ok(envelope::ok(json!({ "id": id, "name": name, "description": description })))
}

#[derive(Deserialize)]
pub struct AssignTeamBody {
    #[serde(rename = "lineUserId")]
    pub line_user_id: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub timestamp: Option<String>,
}

pub async fn assign_team(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AssignTeamBody>,
) -> Result {
    let user_id = body.line_user_id.as_deref().unwrap_or("");
    let Some(team_id) = body.team_id else {
        return Err(AppError::BadRequest("缺少必要欄位 lineUserId 或 teamId".into()));
    };
    if user_id.is_empty() {
        return Err(AppError::BadRequest("缺少必要欄位 lineUserId 或 teamId".into()));
    }
    let team_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM teams WHERE id = ? AND deleted_at IS NULL")
            .bind(team_id)
            .fetch_optional(&state.db)
            .await?;
    let Some(team_name) = team_name else {
        return Err(AppError::NotFound("找不到團隊".into()));
    };

    // Idempotent per (platform user, team): an existing record is returned
    // unchanged and the scan counter is NOT re-incremented (CRD 2897, 2901).
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT id FROM customer_team_assignments WHERE platform_user_id = ? AND team_id = ?",
    )
    .bind(user_id)
    .bind(team_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some(assignment_id) = existing {
        return Ok(envelope::ok(json!({
            "assignmentId": assignment_id,
            "teamName": team_name,
            "message": "已記錄過此團隊指派",
        })));
    }

    let assignment_id = uuid::Uuid::new_v4().to_string();
    let now = body.timestamp.clone().unwrap_or_else(now_iso);
    let metadata = json!({
        "userAgent": headers.get("user-agent").and_then(|v| v.to_str().ok()),
        "recordedAt": now_iso(),
    });
    sqlx::query(
        "INSERT INTO customer_team_assignments
            (id, platform_user_id, team_id, source, display_name, assigned_at, metadata)
         VALUES (?, ?, ?, 'scan', ?, ?, ?)",
    )
    .bind(&assignment_id)
    .bind(user_id)
    .bind(team_id)
    .bind(&body.display_name)
    .bind(&now)
    .bind(metadata.to_string())
    .execute(&state.db)
    .await?;

    // Scan counter on the team's front-end code, when one exists (CRD 2897).
    let _ = sqlx::query(
        "UPDATE team_liff_links SET scan_count = scan_count + 1, updated_at = ? WHERE team_id = ?",
    )
    .bind(now_iso())
    .bind(team_id)
    .execute(&state.db)
    .await;

    // Synthetic pending-conversation broadcast to the destination team,
    // best-effort (CRD 2991).
    let display = body.display_name.clone().unwrap_or_else(|| "LINE 用戶".into());
    state.realtime.to_team(
        team_id,
        "conversation_transferred",
        json!({
            "fromTeamId": null,
            "toTeamId": team_id,
            "teamName": team_name,
            "conversation": {
                "id": format!("pending-{assignment_id}"),
                "customerName": display,
                "platform": "line",
                "status": "pending",
                "lastMessage": "（等待用戶加入好友）",
                "lastMessageAt": now_iso(),
                "unreadCount": 0,
                "teamId": team_id,
            },
            "metadata": {
                "pending": true,
                "platformUserId": user_id,
                "assignmentId": assignment_id,
                "scannedAt": now,
            },
            "actor": { "id": "system", "label": "qr-scan" },
            "reason": "LIFF QR code pre-assignment",
        }),
    );

    Ok(envelope::ok(json!({
        "assignmentId": assignment_id,
        "teamName": team_name,
        "message": "已成功記錄團隊指派",
    })))
}

#[derive(Deserialize)]
pub struct WelcomeBody {
    #[serde(rename = "lineUserId")]
    pub line_user_id: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
}

pub async fn welcome(
    State(state): State<Arc<AppState>>,
    Json(body): Json<WelcomeBody>,
) -> Result {
    let user_id = body.line_user_id.as_deref().unwrap_or("");
    let Some(team_id) = body.team_id else {
        return Err(AppError::BadRequest("缺少必要欄位 lineUserId 或 teamId".into()));
    };
    if user_id.is_empty() {
        return Err(AppError::BadRequest("缺少必要欄位 lineUserId 或 teamId".into()));
    }
    let team_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM teams WHERE id = ? AND deleted_at IS NULL")
            .bind(team_id)
            .fetch_optional(&state.db)
            .await?;
    let Some(team_name) = team_name else {
        return Err(AppError::NotFound("找不到團隊".into()));
    };
    if state.config.line_channel_access_token.is_none() {
        return Err(AppError::Internal("推播憑證尚未設定".into()));
    }

    // Best-effort, non-blocking reconciliation (CRD 2907): failures are
    // logged but never fail the welcome push.
    if let Err(e) = reconcile(&state, user_id, team_id, &team_name).await {
        tracing::warn!(error = %e, "LIFF welcome reconciliation skipped");
    }

    // TODO(channels): real LINE push API call; the credential is validated
    // above and the localized text is fixed (CRD 2907, 2994).
    Ok(envelope::ok(json!({ "message": "歡迎訊息已送出" })))
}

/// Conversation reconciliation for an existing friend (CRD 2907, 2987).
async fn reconcile(
    state: &AppState,
    user_id: &str,
    team_id: i64,
    team_name: &str,
) -> std::result::Result<(), String> {
    let customer: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT id, display_name FROM customers
         WHERE platform = 'line' AND platform_user_id = ? AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| e.to_string())?;
    let Some((customer_id, customer_name)) = customer else {
        return Err("customer record not found; reconciliation skipped".into());
    };

    let conversation: Option<(String, Option<i64>, String)> = sqlx::query_as(
        "SELECT id, team_id, status FROM conversations
         WHERE customer_id = ? AND status != 'closed' AND deleted_at IS NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(customer_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let customer_view = json!({
        "id": customer_id,
        "displayName": customer_name,
        "platform": "line",
    });
    match conversation {
        Some((_, Some(current), _)) if current == team_id => {} // already correct
        Some((conversation_id, prior_team, status)) => {
            sqlx::query("UPDATE conversations SET team_id = ?, updated_at = ? WHERE id = ?")
                .bind(team_id)
                .bind(now_iso())
                .bind(&conversation_id)
                .execute(&state.db)
                .await
                .map_err(|e| e.to_string())?;
            let mut audience = vec![team_id];
            if let Some(p) = prior_team {
                audience.push(p);
            }
            state.realtime.to_teams_and_admins(
                &audience,
                "conversation_transferred",
                json!({
                    "conversationId": conversation_id,
                    "fromTeamId": prior_team,
                    "toTeamId": team_id,
                    "teamName": team_name,
                    "customer": customer_view,
                    "status": status,
                    "actor": { "id": "system", "label": "qr-scan" },
                    "reason": "existing-friend reassignment",
                }),
            );
        }
        None => {
            let conversation_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO conversations (id, customer_id, team_id, status, priority, created_at)
                 VALUES (?, ?, ?, 'active', 'normal', ?)",
            )
            .bind(&conversation_id)
            .bind(customer_id)
            .bind(team_id)
            .bind(now_iso())
            .execute(&state.db)
            .await
            .map_err(|e| e.to_string())?;
            state.realtime.to_team(
                team_id,
                "conversation_transferred",
                json!({
                    "conversationId": conversation_id,
                    "fromTeamId": null,
                    "toTeamId": team_id,
                    "teamName": team_name,
                    "customer": customer_view,
                    "status": "active",
                    "actor": { "id": "system", "label": "qr-scan" },
                    "reason": "new conversation for an existing friend",
                }),
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- /join page

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub async fn join_page(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(team_ref) = params.get("team").filter(|t| !t.is_empty()) else {
        return Html("<html><body><h1>無效的連結</h1><p>此邀請連結缺少必要的參數。</p></body></html>")
            .into_response();
    };
    let team: Option<(i64, String, Option<String>)> = match team_ref.parse::<i64>() {
        Ok(id) => sqlx::query_as(
            "SELECT id, name, description FROM teams WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None),
        Err(_) => sqlx::query_as(
            "SELECT t.id, t.name, t.description FROM teams t
             JOIN qr_codes q ON q.team_id = t.id
             WHERE q.token = ? AND t.deleted_at IS NULL",
        )
        .bind(team_ref)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None),
    };
    match team {
        Some((id, name, description)) => Html(format!(
            "<html><body><h1>加入 {}</h1><p>{}</p><p>團隊編號: {}</p></body></html>",
            html_escape(&name),
            html_escape(description.as_deref().unwrap_or("歡迎加入我們的客服團隊")),
            id
        ))
        .into_response(),
        None => Html("<html><body><h1>連結已失效</h1><p>此邀請連結對應的團隊不存在。</p></body></html>")
            .into_response(),
    }
}

// ---------------------------------------------------------------- admin ops

pub async fn batch_generate(
    State(state): State<Arc<AppState>>,
    Extension(_admin): Extension<AuthUser>,
) -> Result {
    let teams: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM teams
         WHERE deleted_at IS NULL AND is_active = 1
           AND id NOT IN (SELECT team_id FROM team_liff_links)",
    )
    .fetch_all(&state.db)
    .await?;
    if teams.is_empty() {
        return Ok(envelope::ok_msg(
            json!({ "total": 0, "success": 0, "failed": 0, "errors": [] }),
            "所有團隊皆已擁有 LIFF QR Code",
        ));
    }
    let mut success = 0usize;
    let mut errors: Vec<Value> = Vec::new();
    for (team_id, team_name) in &teams {
        match crate::domain::teams::store::upsert_liff(&state.db, *team_id).await {
            Ok(_) => success += 1,
            Err(e) => errors.push(json!({
                "teamId": team_id, "teamName": team_name, "error": e.to_string(),
            })),
        }
    }
    Ok(envelope::ok(json!({
        "total": teams.len(),
        "success": success,
        "failed": errors.len(),
        "errors": errors,
    })))
}

pub async fn coverage_status(
    State(state): State<Arc<AppState>>,
    Extension(_admin): Extension<AuthUser>,
) -> Result {
    let teams: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT t.id, t.name,
                EXISTS(SELECT 1 FROM team_liff_links l WHERE l.team_id = t.id) AS has_liff
         FROM teams t WHERE t.deleted_at IS NULL AND t.is_active = 1 ORDER BY t.id",
    )
    .fetch_all(&state.db)
    .await?;
    let total = teams.len() as i64;
    let with = teams.iter().filter(|(_, _, has)| *has != 0).count() as i64;
    let coverage = if total > 0 { with as f64 * 100.0 / total as f64 } else { 0.0 };
    Ok(envelope::ok(json!({
        "totalTeams": total,
        "teamsWithLiffQR": with,
        "teamsWithoutLiffQR": total - with,
        "coverage": format!("{coverage:.2}%"),
        "teams": teams.iter().map(|(id, name, has)| json!({
            "id": id, "name": name, "hasLiffQR": *has != 0,
        })).collect::<Vec<_>>(),
    })))
}

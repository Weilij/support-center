use axum::extract::State;
use axum::Extension;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::sessions::store;

use super::{
    bad, parse_json, require_admin, require_enum, require_iso_date, require_uuid, validate_tags,
    JsonBody, PRIORITIES,
};

const BATCH_ACTIONS: &[&str] = &[
    "close",
    "reopen",
    "update_priority",
    "add_tags",
    "remove_tags",
    "delete",
];

pub async fn batch(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    require_admin(&user, "Administrator access required")?;
    let body = parse_json(body)?;
    let ids = body
        .get("sessionIds")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| bad("sessionIds must be a non-empty array"))?;
    if ids.len() > 100 {
        return Err(bad("sessionIds can contain at most 100 items"));
    }
    let mut session_ids: Vec<String> = Vec::with_capacity(ids.len());
    for v in ids {
        let raw = v
            .as_str()
            .ok_or_else(|| bad("Invalid session ID format: must be a UUID"))?;
        session_ids.push(require_uuid(raw, "session ID")?);
    }
    let action = body.get("action").and_then(Value::as_str).unwrap_or("");
    require_enum(action, BATCH_ACTIONS, "action")?;
    let data = body.get("data").cloned().unwrap_or(Value::Null);

    let priority = if action == "update_priority" {
        let p = data
            .get("priority")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("data.priority is required for update_priority"))?;
        Some(require_enum(p, PRIORITIES, "priority")?)
    } else {
        None
    };
    let tags = if action == "add_tags" || action == "remove_tags" {
        let v = data
            .get("tags")
            .ok_or_else(|| bad(format!("data.tags is required for {action}")))?;
        Some(validate_tags(v)?)
    } else {
        None
    };
    let end_time = match data.get("endTime").and_then(Value::as_str) {
        Some(t) => Some(require_iso_date(t, "endTime")?),
        None => None,
    };

    let now = crate::db::now_iso();
    let mut results: Vec<Value> = Vec::with_capacity(session_ids.len());
    let mut succeeded = 0usize;
    for sid in &session_ids {
        let outcome: std::result::Result<bool, AppError> = match action {
            "close" => {
                let ended = end_time.clone().unwrap_or_else(|| now.clone());
                Ok(sqlx::query(
                    "UPDATE conversation_sessions SET is_active = 0, ended_at = $1, updated_at = $2
                     WHERE id = $3 AND is_active = 1",
                )
                .bind(&ended)
                .bind(&now)
                .bind(sid)
                .execute(&state.db)
                .await?
                .rows_affected()
                    > 0)
            }
            "reopen" => Ok(sqlx::query(
                "UPDATE conversation_sessions
                    SET is_active = 1, ended_at = NULL, last_activity_at = $1, updated_at = $2
                  WHERE id = $3 AND is_active = 0",
            )
            .bind(&now)
            .bind(&now)
            .bind(sid)
            .execute(&state.db)
            .await?
            .rows_affected()
                > 0),
            "update_priority" => Ok(sqlx::query(
                "UPDATE conversation_sessions SET priority = $1, updated_at = $2 WHERE id = $3",
            )
            .bind(priority.as_deref())
            .bind(&now)
            .bind(sid)
            .execute(&state.db)
            .await?
            .rows_affected()
                > 0),
            "add_tags" | "remove_tags" => match store::find(&state.db, sid).await? {
                None => Ok(false),
                Some(s) => {
                    let mut current: Vec<String> = s
                        .tags
                        .as_deref()
                        .and_then(|t| serde_json::from_str(t).ok())
                        .unwrap_or_default();
                    let change = tags.clone().unwrap_or_default();
                    if action == "add_tags" {
                        for t in change {
                            if !current.contains(&t) {
                                current.push(t);
                            }
                        }
                    } else {
                        current.retain(|t| !change.contains(t));
                    }
                    sqlx::query(
                        "UPDATE conversation_sessions SET tags = $1, updated_at = $2 WHERE id = $3",
                    )
                    .bind(json!(current).to_string())
                    .bind(&now)
                    .bind(sid)
                    .execute(&state.db)
                    .await?;
                    Ok(true)
                }
            },
            "delete" => Ok(
                sqlx::query("DELETE FROM conversation_sessions WHERE id = $1")
                    .bind(sid)
                    .execute(&state.db)
                    .await?
                    .rows_affected()
                    > 0,
            ),
            _ => unreachable!("validated above"),
        };
        match outcome {
            Ok(true) => {
                succeeded += 1;
                results.push(json!({ "sessionId": sid, "success": true }));
            }
            Ok(false) => results.push(json!({
                "sessionId": sid,
                "success": false,
                "error": "Session not found or not applicable",
            })),
            Err(e) => results.push(json!({
                "sessionId": sid,
                "success": false,
                "error": e.to_string(),
            })),
        }
    }

    Ok(envelope::ok_msg(
        json!({
            "total": session_ids.len(),
            "succeeded": succeeded,
            "failed": session_ids.len() - succeeded,
            "results": results,
        }),
        &format!("Batch {action} completed"),
    ))
}

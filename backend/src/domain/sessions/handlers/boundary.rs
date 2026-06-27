use axum::extract::State;
use axum::Extension;
use serde_json::Value;
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::sessions::store::{self, NewSession};
use crate::domain::sessions::topics;

use super::{
    bad, continue_session_or_error, has_conversation_access, parse_json, require_enum,
    require_uuid, session_not_found, JsonBody, SENDER_TYPES,
};

pub async fn get_or_create(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    let body = parse_json(body)?;
    let conversation_id = body.get("conversation_id").and_then(Value::as_str);
    let message_content = body.get("messageContent").and_then(Value::as_str);
    let sender_type = body.get("senderType").and_then(Value::as_str);
    let (Some(conversation_id), Some(message_content), Some(sender_type)) =
        (conversation_id, message_content, sender_type)
    else {
        return Err(bad(
            "conversation_id, messageContent and senderType are required",
        ));
    };
    let conversation_id = require_uuid(conversation_id, "conversation_id")?;
    if !has_conversation_access(&state, &user, &conversation_id).await? {
        return Err(AppError::Forbidden(
            "You do not have access to this conversation".into(),
        ));
    }
    require_enum(sender_type, SENDER_TYPES, "senderType")?;

    let current = store::latest_active(&state.db, &conversation_id).await?;
    let detection = topics::detect_boundary(
        current.as_ref(),
        message_content,
        sender_type,
        chrono::Utc::now(),
    );

    if detection.should_create_new {
        let now = crate::db::now_iso();
        if let Some(prior) = &current {
            sqlx::query(
                "UPDATE conversation_sessions SET is_active = 0, ended_at = $1, updated_at = $2
                 WHERE id = $3 AND is_active = 1",
            )
            .bind(&now)
            .bind(&now)
            .bind(&prior.id)
            .execute(&state.db)
            .await?;
        }
        let session = store::create(
            &state.db,
            NewSession {
                conversation_id: &conversation_id,
                session_type: "continuous",
                topic: detection.suggested_topic.clone(),
                priority: None,
                tags: None,
                metadata: None,
            },
        )
        .await?;
        return Ok(envelope::ok(store::session_view(&session)));
    }

    let existing = continue_session_or_error(current)?;
    let now = crate::db::now_iso();
    sqlx::query(
        "UPDATE conversation_sessions SET last_activity_at = $1, updated_at = $2 WHERE id = $3",
    )
    .bind(&now)
    .bind(&now)
    .bind(&existing.id)
    .execute(&state.db)
    .await?;
    let refreshed = store::find(&state.db, &existing.id)
        .await?
        .ok_or_else(session_not_found)?;
    Ok(envelope::ok(store::session_view(&refreshed)))
}

pub async fn detect_boundary(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    let body = parse_json(body)?;
    let message_content = body
        .get("messageContent")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("messageContent is required"))?;
    let sender_type = body
        .get("senderType")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("senderType is required"))?;
    let current = match body.get("currentSessionId").and_then(Value::as_str) {
        Some(raw) => {
            let id = require_uuid(raw, "currentSessionId")?;
            store::find(&state.db, &id).await?
        }
        None => None,
    };
    let detection = topics::detect_boundary(
        current.as_ref(),
        message_content,
        sender_type,
        chrono::Utc::now(),
    );
    Ok(envelope::ok(detection.to_json()))
}

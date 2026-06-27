use axum::extract::{Path, State};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::messaging::store;

use super::{message_not_found, parse_json, JsonBody};

const FORWARD_CAP: usize = 20;

#[derive(Deserialize)]
pub struct ForwardBody {
    #[serde(rename = "targetConversationIds")]
    pub target_conversation_ids: Option<Value>,
    pub comment: Option<String>,
}

pub async fn forward_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<ForwardBody>,
) -> Result {
    let body = parse_json(body)?;
    let targets: Vec<String> = body
        .target_conversation_ids
        .as_ref()
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest("targetConversationIds must be a non-empty array".into())
        })?;
    if targets.len() > FORWARD_CAP {
        return Err(AppError::BadRequest(format!(
            "Cannot forward to more than {FORWARD_CAP} conversations"
        )));
    }
    let m = store::find_message(&state.db, &id)
        .await?
        .ok_or_else(message_not_found)?;

    let placeholders = vec!["?"; targets.len()].join(", ");
    let sql =
        format!("SELECT id FROM conversations WHERE id IN ({placeholders}) AND deleted_at IS NULL");
    let sql = crate::db::pg_params(&sql);
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for t in &targets {
        q = q.bind(t);
    }
    let existing: HashSet<String> = q.fetch_all(&state.db).await?.into_iter().collect();

    let mut content = format!("[Forwarded] {}", m.content.as_deref().unwrap_or_default());
    if let Some(comment) = body
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        content.push_str(&format!("\n\n{comment}"));
    }
    let metadata = json!({
        "forwardedFrom": {
            "messageId": m.id,
            "conversationId": m.conversation_id,
            "senderType": m.sender_type,
        },
        "forwardedBy": user.id,
        "forwardedAt": crate::db::now_iso(),
    })
    .to_string();

    let now = crate::db::now_iso();
    let mut results: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let mut tx = state.db.begin().await?;
    for target in &targets {
        if !existing.contains(target) {
            errors.push(json!({ "conversationId": target, "error": "Conversation not found" }));
            continue;
        }
        let message_id = store::new_message_id();
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content,
                                   content_type, is_sent, sent_at, delivery_status, metadata,
                                   sender_name, created_at)
             VALUES ($1, $2, 'agent', $3, $4, $5, 1, $6, 'sent', $7, $8, $9)",
        )
        .bind(&message_id)
        .bind(target)
        .bind(&user.id)
        .bind(&content)
        .bind(&m.content_type)
        .bind(&now)
        .bind(&metadata)
        .bind(&user.display_name)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        results.push(json!({
            "conversationId": target,
            "messageId": message_id,
            "status": "forwarded",
        }));
    }
    let bumped: Vec<&str> = results
        .iter()
        .filter_map(|r| r["conversationId"].as_str())
        .collect();
    if !bumped.is_empty() {
        let placeholders = vec!["?"; bumped.len()].join(", ");
        let sql = format!(
            "UPDATE conversations SET last_message_at = $1, updated_at = $2
             WHERE id IN ({placeholders})"
        );
        let sql = crate::db::pg_params(&sql);
        let mut q = sqlx::query(&sql).bind(&now).bind(&now);
        for cid in &bumped {
            q = q.bind(*cid);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;

    crate::domain::auth::store::log_activity(
        &state.db,
        &user.id,
        &user.display_name,
        &user.role,
        "message forward",
        "message",
        Some(&id),
        Some(json!({ "targetCount": targets.len() })),
        None,
        None,
    )
    .await;

    let mut data = json!({
        "originalMessageId": id,
        "totalTargets": targets.len(),
        "successCount": results.len(),
        "failureCount": errors.len(),
        "results": results,
    });
    if !errors.is_empty() {
        data["errors"] = json!(errors);
    }
    Ok(envelope::created(data))
}

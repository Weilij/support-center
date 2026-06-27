use axum::extract::State;
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::messaging::store::{self, RECALL_PLACEHOLDER};

use super::{parse_json, JsonBody};

const BULK_CAP: usize = 100;

#[derive(Deserialize)]
pub struct BulkCreateBody {
    pub messages: Option<Value>,
}

pub async fn bulk_create(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<BulkCreateBody>,
) -> Result {
    let body = parse_json(body)?;
    let entries = body
        .messages
        .as_ref()
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest("messages must be a non-empty array".into()))?
        .clone();
    if entries.len() > BULK_CAP {
        return Err(AppError::BadRequest(format!(
            "Cannot create more than {BULK_CAP} messages per batch"
        )));
    }

    let referenced: HashSet<String> = entries
        .iter()
        .filter_map(|e| e.get("conversationId").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let mut existing: HashSet<String> = HashSet::new();
    if !referenced.is_empty() {
        let ids: Vec<&String> = referenced.iter().collect();
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!(
            "SELECT id FROM conversations WHERE id IN ({placeholders}) AND deleted_at IS NULL"
        );
        let sql = crate::db::pg_params(&sql);
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for id in &ids {
            q = q.bind(id.as_str());
        }
        existing = q.fetch_all(&state.db).await?.into_iter().collect();
    }

    let now = crate::db::now_iso();
    let mut results: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let mut touched: HashSet<String> = HashSet::new();
    let mut tx = state.db.begin().await?;
    for (index, entry) in entries.iter().enumerate() {
        let conversation_id = entry
            .get("conversationId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let content = entry
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let (Some(conversation_id), Some(content)) = (conversation_id, content) else {
            errors.push(
                json!({ "index": index, "error": "conversationId and content are required" }),
            );
            continue;
        };
        if !existing.contains(conversation_id) {
            errors.push(json!({ "index": index, "error": "Conversation not found" }));
            continue;
        }
        let message_type = entry
            .get("messageType")
            .and_then(Value::as_str)
            .unwrap_or("text");
        let metadata = entry.get("metadata").map(|m| m.to_string());
        let message_id = store::new_message_id();
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, sender_type, agent_id, content,
                                   content_type, is_sent, sent_at, delivery_status, metadata,
                                   sender_name, created_at)
             VALUES ($1, $2, 'agent', $3, $4, $5, 1, $6, 'sent', $7, $8, $9)",
        )
        .bind(&message_id)
        .bind(conversation_id)
        .bind(&user.id)
        .bind(content)
        .bind(message_type)
        .bind(&now)
        .bind(&metadata)
        .bind(&user.display_name)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        touched.insert(conversation_id.to_string());
        results.push(json!({
            "index": index,
            "messageId": message_id,
            "conversationId": conversation_id,
            "status": "created",
        }));
    }
    for conversation_id in &touched {
        sqlx::query("UPDATE conversations SET last_message_at = $1, updated_at = $2 WHERE id = $3")
            .bind(&now)
            .bind(&now)
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let mut data = json!({
        "totalRequested": entries.len(),
        "successCount": results.len(),
        "failureCount": errors.len(),
        "results": results,
    });
    if !errors.is_empty() {
        data["errors"] = json!(errors);
    }
    Ok(envelope::created(data))
}

#[derive(Deserialize)]
pub struct BulkDeleteBody {
    #[serde(rename = "messageIds")]
    pub message_ids: Option<Value>,
}

pub async fn bulk_delete(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: JsonBody<BulkDeleteBody>,
) -> Result {
    let body = parse_json(body)?;
    let ids: Vec<String> = body
        .message_ids
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
        .ok_or_else(|| AppError::BadRequest("messageIds must be a non-empty array".into()))?;
    if ids.len() > BULK_CAP {
        return Err(AppError::BadRequest(format!(
            "Cannot recall more than {BULK_CAP} messages per batch"
        )));
    }

    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT id, conversation_id, sender_type, agent_id, is_recalled, recall_deadline
         FROM messages WHERE id IN ({placeholders}) AND deleted_at IS NULL"
    );
    let sql = crate::db::pg_params(&sql);
    type RecallRow = (String, String, Option<String>, i64, Option<String>);
    let mut q =
        sqlx::query_as::<_, (String, String, String, Option<String>, i64, Option<String>)>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    let found: HashMap<String, RecallRow> = q
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|(id, cid, st, aid, rec, dl)| (id, (cid, st, aid, rec, dl)))
        .collect();

    let now = crate::db::now_iso();
    let mut eligible: Vec<(String, String)> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    for id in &ids {
        match found.get(id) {
            None => errors.push(json!({ "messageId": id, "error": "Message not found" })),
            Some((cid, sender_type, agent_id, is_recalled, deadline)) => {
                let permitted = sender_type == "agent"
                    && (user.is_admin() || agent_id.as_deref() == Some(user.id.as_str()));
                if !permitted {
                    errors.push(json!({ "messageId": id, "error": "Permission denied" }));
                } else if *is_recalled != 0 {
                    errors.push(
                        json!({ "messageId": id, "error": "Message has already been recalled" }),
                    );
                } else if deadline.as_deref().is_some_and(|d| now.as_str() > d) {
                    errors.push(json!({ "messageId": id, "error": "Recall deadline has passed" }));
                } else {
                    eligible.push((id.clone(), cid.clone()));
                }
            }
        }
    }

    if !eligible.is_empty() {
        let placeholders = vec!["?"; eligible.len()].join(", ");
        let sql = format!(
            "UPDATE messages
                SET is_recalled = 1, recalled_at = $1, content = $2, delivery_status = 'recalled',
                    updated_at = $3
              WHERE id IN ({placeholders})"
        );
        let sql = crate::db::pg_params(&sql);
        let mut q = sqlx::query(&sql)
            .bind(&now)
            .bind(RECALL_PLACEHOLDER)
            .bind(&now);
        for (id, _) in &eligible {
            q = q.bind(id);
        }
        q.execute(&state.db).await?;
    }

    let results: Vec<Value> = eligible
        .iter()
        .map(|(id, cid)| {
            json!({
                "messageId": id,
                "conversationId": cid,
                "recalledAt": now,
                "status": "recalled",
            })
        })
        .collect();
    let mut data = json!({
        "totalRequested": ids.len(),
        "successCount": results.len(),
        "failureCount": errors.len(),
        "results": results,
    });
    if !errors.is_empty() {
        data["errors"] = json!(errors);
    }
    Ok(envelope::ok(data))
}

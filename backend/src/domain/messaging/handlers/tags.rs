use axum::extract::{Path, State};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::messaging::store;

use super::{message_not_found, parse_json, JsonBody};

const TAG_CAP: usize = 10;

pub async fn list_tags(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT metadata FROM messages
         WHERE deleted_at IS NULL AND is_recalled = 0 AND metadata IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for (raw,) in &rows {
        if let Some(tags) = store::parse_metadata(raw)
            .get("tags")
            .and_then(Value::as_array)
        {
            for tag in tags.iter().filter_map(Value::as_str) {
                *counts.entry(tag.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut tags: Vec<(String, i64)> = counts.into_iter().collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total = tags.len();
    let tags: Vec<Value> = tags
        .into_iter()
        .map(|(name, count)| json!({ "name": name, "count": count }))
        .collect();
    Ok(envelope::ok(json!({ "tags": tags, "total": total })))
}

#[derive(Deserialize)]
pub struct TagsBody {
    pub tags: Option<Value>,
}

pub async fn set_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: JsonBody<TagsBody>,
) -> Result {
    let body = parse_json(body)?;
    let raw = body
        .tags
        .as_ref()
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("tags must be an array".into()))?;
    if raw.len() > TAG_CAP {
        return Err(AppError::BadRequest(format!(
            "Cannot set more than {TAG_CAP} tags"
        )));
    }
    let mut tags: Vec<String> = Vec::new();
    for entry in raw {
        let tag = entry
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::BadRequest("Every tag must be a non-empty string".into()))?;
        tags.push(tag.to_string());
    }

    let m = store::find_message(&state.db, &id)
        .await?
        .ok_or_else(message_not_found)?;
    let mut metadata = store::metadata_map(&m.metadata);
    let previous = metadata.get("tags").cloned().unwrap_or_else(|| json!([]));
    let now = crate::db::now_iso();
    metadata.insert("tags".into(), json!(tags));
    metadata.insert("tagsUpdatedAt".into(), json!(now));
    metadata.insert("tagsUpdatedBy".into(), json!(user.id));
    sqlx::query("UPDATE messages SET metadata = $1, updated_at = $2 WHERE id = $3")
        .bind(Value::Object(metadata).to_string())
        .bind(&now)
        .bind(&id)
        .execute(&state.db)
        .await?;

    Ok(envelope::ok(json!({
        "messageId": id,
        "conversationId": m.conversation_id,
        "tags": tags,
        "previousTags": previous,
        "updatedAt": now,
        "updatedBy": user.id,
    })))
}

pub async fn remove_tags(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let m = store::find_message(&state.db, &id)
        .await?
        .ok_or_else(message_not_found)?;
    let mut metadata = store::metadata_map(&m.metadata);
    let removed = metadata.remove("tags").unwrap_or_else(|| json!([]));
    let now = crate::db::now_iso();
    metadata.insert("tagsRemovedAt".into(), json!(now));
    metadata.insert("tagsRemovedBy".into(), json!(user.id));
    sqlx::query("UPDATE messages SET metadata = $1, updated_at = $2 WHERE id = $3")
        .bind(Value::Object(metadata).to_string())
        .bind(&now)
        .bind(&id)
        .execute(&state.db)
        .await?;

    Ok(envelope::ok(json!({
        "messageId": id,
        "conversationId": m.conversation_id,
        "removedTags": removed,
        "removedAt": now,
    })))
}

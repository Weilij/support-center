use axum::extract::{Query, State};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::envelope;
use crate::error::HandlerResult as Result;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::sessions::{store, topics};

use super::{bad, ok_count, parse_json, require_uuid, JsonBody};

#[derive(Deserialize)]
pub struct TopicStatsQuery {
    pub conversation_id: Option<String>,
}

pub async fn topic_stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<TopicStatsQuery>,
) -> Result {
    let cid = match q.conversation_id.as_deref() {
        Some(c) => Some(require_uuid(c, "conversation_id")?),
        None => None,
    };
    let stats = store::statistics(&state.db, cid.as_deref()).await?;
    Ok(envelope::ok(json!({
        "total": stats["total"],
        "topics": stats["topicDistribution"],
    })))
}

pub async fn analyze_topic(Extension(_user): Extension<AuthUser>, body: JsonBody<Value>) -> Result {
    let body = parse_json(body)?;
    let content = body
        .get("messageContent")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("messageContent is required"))?;
    let result = topics::derive_topic(content);
    Ok(envelope::ok(json!({
        "topic": result.topic,
        "confidence": result.confidence,
        "source": result.source,
    })))
}

pub async fn suggest_topics(
    Extension(_user): Extension<AuthUser>,
    body: JsonBody<Value>,
) -> Result {
    let body = parse_json(body)?;
    let content = body
        .get("messageContent")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("messageContent is required"))?;
    let limit = match body.get("limit") {
        None => 3,
        Some(v) => v
            .as_i64()
            .filter(|n| (1..=10).contains(n))
            .ok_or_else(|| bad("limit must be between 1 and 10"))? as usize,
    };
    let suggestions = topics::suggest_topics(content, limit);
    let count = suggestions.len();
    Ok(ok_count(json!(suggestions), count, None))
}

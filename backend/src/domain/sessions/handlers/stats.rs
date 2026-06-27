use axum::extract::{Path, Query, State};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::envelope;
use crate::error::HandlerResult as Result;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::sessions::store;

use super::{bad, require_admin, require_uuid};

#[derive(Deserialize)]
pub struct StatsQuery {
    pub conversation_id: Option<String>,
}

pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<StatsQuery>,
) -> Result {
    require_admin(&user, "Administrator access required")?;
    let cid = match q.conversation_id.as_deref() {
        Some(c) => Some(require_uuid(c, "conversation_id")?),
        None => None,
    };
    let stats = store::statistics(&state.db, cid.as_deref()).await?;
    Ok(envelope::ok(stats))
}

pub async fn stats_for_conversation(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    require_admin(&user, "Administrator access required")?;
    let cid = require_uuid(&raw_id, "conversation_id")?;
    let mut stats = store::statistics(&state.db, Some(&cid)).await?;
    stats["conversationId"] = json!(cid);
    Ok(envelope::ok(stats))
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    pub conversation_id: Option<String>,
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
}

pub async fn activity_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ActivityQuery>,
) -> Result {
    require_admin(&user, "Administrator access required")?;
    let cid = match q.conversation_id.as_deref() {
        Some(c) => Some(require_uuid(c, "conversation_id")?),
        None => None,
    };
    let range = q.time_range.as_deref().unwrap_or("week");
    let (days, bucket_len) = match range {
        "day" => (1, 13),
        "week" => (7, 10),
        "month" => (30, 10),
        "year" => (365, 7),
        _ => return Err(bad("timeRange must be one of: day, week, month, year")),
    };
    let since = (chrono::Utc::now() - chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    #[derive(Default, Clone)]
    struct Bucket {
        created: i64,
        ended: i64,
        messages: i64,
        active_minutes: f64,
    }
    let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
    let conv_clause = if cid.is_some() {
        " AND conversation_id = ?"
    } else {
        ""
    };

    let created_sql = format!(
        "SELECT substr(created_at, 1, {bucket_len}), COUNT(*),
                COALESCE(SUM(EXTRACT(EPOCH FROM (COALESCE(ended_at, last_activity_at, created_at)::timestamptz
                              - COALESCE(started_at, created_at)::timestamptz)) / 60.0)::float8, 0)
         FROM conversation_sessions WHERE created_at >= $1{conv_clause} GROUP BY 1"
    );
    let created_sql = crate::db::pg_params(&created_sql);
    let mut query = sqlx::query_as::<_, (String, i64, f64)>(&created_sql).bind(&since);
    if let Some(c) = &cid {
        query = query.bind(c.clone());
    }
    for (k, n, mins) in query.fetch_all(&state.db).await? {
        let b = buckets.entry(k).or_default();
        b.created = n;
        b.active_minutes = (mins * 100.0).round() / 100.0;
    }

    let ended_sql = format!(
        "SELECT substr(ended_at, 1, {bucket_len}), COUNT(*) FROM conversation_sessions
         WHERE ended_at IS NOT NULL AND ended_at >= $1{conv_clause} GROUP BY 1"
    );
    let ended_sql = crate::db::pg_params(&ended_sql);
    let mut query = sqlx::query_as::<_, (String, i64)>(&ended_sql).bind(&since);
    if let Some(c) = &cid {
        query = query.bind(c.clone());
    }
    for (k, n) in query.fetch_all(&state.db).await? {
        buckets.entry(k).or_default().ended = n;
    }

    let messages_sql = format!(
        "SELECT substr(created_at, 1, {bucket_len}), COUNT(*) FROM messages
         WHERE session_id IS NOT NULL AND deleted_at IS NULL AND created_at >= $1{conv_clause}
         GROUP BY 1"
    );
    let messages_sql = crate::db::pg_params(&messages_sql);
    let mut query = sqlx::query_as::<_, (String, i64)>(&messages_sql).bind(&since);
    if let Some(c) = &cid {
        query = query.bind(c.clone());
    }
    for (k, n) in query.fetch_all(&state.db).await? {
        buckets.entry(k).or_default().messages = n;
    }

    let hours_sql = format!(
        "SELECT substr(created_at, 12, 2), COUNT(*) FROM conversation_sessions
         WHERE created_at >= $1{conv_clause} GROUP BY 1"
    );
    let hours_sql = crate::db::pg_params(&hours_sql);
    let mut query = sqlx::query_as::<_, (String, i64)>(&hours_sql).bind(&since);
    if let Some(c) = &cid {
        query = query.bind(c.clone());
    }
    let hours = query.fetch_all(&state.db).await?;
    let peak = hours.iter().max_by_key(|(_, n)| *n).map(|(h, _)| h.clone());
    let least = hours.iter().min_by_key(|(_, n)| *n).map(|(h, _)| h.clone());

    let total_created: i64 = buckets.values().map(|b| b.created).sum();
    let total_ended: i64 = buckets.values().map(|b| b.ended).sum();
    let total_messages: i64 = buckets.values().map(|b| b.messages).sum();
    let bucket_count = buckets.len().max(1) as f64;
    let items: Vec<Value> = buckets
        .iter()
        .map(|(k, b)| {
            json!({
                "bucket": k,
                "sessionsCreated": b.created,
                "sessionsEnded": b.ended,
                "messagesSent": b.messages,
                "activeMinutes": b.active_minutes,
            })
        })
        .collect();

    Ok(envelope::ok(json!({
        "timeRange": range,
        "buckets": items,
        "summary": {
            "totalSessionsCreated": total_created,
            "totalSessionsEnded": total_ended,
            "totalMessages": total_messages,
            "avgSessionsPerBucket": ((total_created as f64 / bucket_count) * 100.0).round() / 100.0,
            "peakActivityHour": peak,
            "leastActivityHour": least,
        },
    })))
}

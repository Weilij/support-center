//! Conversation-session persistence (CRD §1.2B, lines 329-483).

use serde_json::{json, Map, Value};
use sqlx::SqlitePool;

use crate::error::AppError;

#[derive(sqlx::FromRow, Clone)]
pub struct SessionRow {
    pub id: String,
    pub conversation_id: String,
    pub session_type: Option<String>,
    pub topic: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub last_activity_at: Option<String>,
    pub message_count: i64,
    pub is_active: i64,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub priority: Option<String>,
    pub sentiment: Option<String>,
    pub tags: Option<String>,
    pub metadata: Option<String>,
}

pub const SELECT: &str =
    "SELECT id, conversation_id, session_type, topic, started_at, ended_at, last_activity_at,
            message_count, is_active, created_at, updated_at, priority, sentiment, tags, metadata
     FROM conversation_sessions";

fn parse_json_text(raw: &Option<String>) -> Value {
    raw.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null)
}

/// Wire view of one conversation session (CRD 473-474).
pub fn session_view(s: &SessionRow) -> Value {
    json!({
        "id": s.id,
        "conversationId": s.conversation_id,
        "sessionType": s.session_type,
        "topic": s.topic,
        "startTime": s.started_at,
        "endTime": s.ended_at,
        "lastActivityTime": s.last_activity_at,
        "messageCount": s.message_count,
        "isActive": s.is_active != 0,
        "createdAt": s.created_at,
        "updatedAt": s.updated_at,
        "priority": s.priority,
        "sentiment": s.sentiment,
        "tags": parse_json_text(&s.tags),
        "metadata": parse_json_text(&s.metadata),
    })
}

pub async fn find(db: &SqlitePool, id: &str) -> Result<Option<SessionRow>, AppError> {
    let sql = format!("{SELECT} WHERE id = ?");
    Ok(sqlx::query_as(&sql).bind(id).fetch_optional(db).await?)
}

/// Team of the session's underlying conversation (None when unassigned).
pub async fn conversation_team(
    db: &SqlitePool,
    session: &SessionRow,
) -> Result<Option<i64>, AppError> {
    Ok(sqlx::query_scalar("SELECT team_id FROM conversations WHERE id = ?")
        .bind(&session.conversation_id)
        .fetch_optional(db)
        .await?
        .flatten())
}

/// The conversation's most-recently-active session (CRD 450).
pub async fn latest_active(
    db: &SqlitePool,
    conversation_id: &str,
) -> Result<Option<SessionRow>, AppError> {
    let sql = format!(
        "{SELECT} WHERE conversation_id = ? AND is_active = 1
         ORDER BY COALESCE(last_activity_at, created_at) DESC, created_at DESC LIMIT 1"
    );
    Ok(sqlx::query_as(&sql).bind(conversation_id).fetch_optional(db).await?)
}

pub struct NewSession<'a> {
    pub conversation_id: &'a str,
    pub session_type: &'a str,
    pub topic: Option<String>,
    pub priority: Option<String>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<Value>,
}

/// Insert a new active session with start/last-activity set to now and a zero
/// message count (CRD 346).
pub async fn create(db: &SqlitePool, s: NewSession<'_>) -> Result<SessionRow, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::db::now_iso();
    sqlx::query(
        "INSERT INTO conversation_sessions
             (id, conversation_id, session_type, topic, started_at, ended_at, last_activity_at,
              message_count, is_active, created_at, priority, sentiment, tags, metadata)
         VALUES (?, ?, ?, ?, ?, NULL, ?, 0, 1, ?, ?, NULL, ?, ?)",
    )
    .bind(&id)
    .bind(s.conversation_id)
    .bind(s.session_type)
    .bind(&s.topic)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(&s.priority)
    .bind(s.tags.as_ref().map(|t| json!(t).to_string()))
    .bind(s.metadata.as_ref().map(|m| m.to_string()))
    .execute(db)
    .await?;
    find(db, &id)
        .await?
        .ok_or_else(|| AppError::Internal("Failed to reload created session".into()))
}

/// Aggregate summary over a filtered session set (CRD 353).
pub async fn summarize(
    db: &SqlitePool,
    where_clause: &str,
    binds: &[String],
) -> Result<Value, AppError> {
    let sql = format!(
        "SELECT COUNT(*) AS total,
                COALESCE(SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END), 0) AS active,
                COALESCE(SUM(CASE WHEN is_active = 0 THEN 1 ELSE 0 END), 0) AS inactive
         FROM conversation_sessions {where_clause}"
    );
    let mut q = sqlx::query_as::<_, (i64, i64, i64)>(&sql);
    for b in binds {
        q = q.bind(b.clone());
    }
    let (total, active, inactive) = q.fetch_one(db).await?;

    let by = |col: &str| {
        format!(
            "SELECT COALESCE({col}, 'unspecified') AS k, COUNT(*) FROM conversation_sessions
             {where_clause} GROUP BY k"
        )
    };
    let mut by_type = Map::new();
    let type_sql = by("session_type");
    let mut q = sqlx::query_as::<_, (String, i64)>(&type_sql);
    for b in binds {
        q = q.bind(b.clone());
    }
    for (k, n) in q.fetch_all(db).await? {
        by_type.insert(k, json!(n));
    }
    let mut by_priority = Map::new();
    let priority_sql = by("priority");
    let mut q = sqlx::query_as::<_, (String, i64)>(&priority_sql);
    for b in binds {
        q = q.bind(b.clone());
    }
    for (k, n) in q.fetch_all(db).await? {
        by_priority.insert(k, json!(n));
    }

    Ok(json!({
        "total": total,
        "active": active,
        "inactive": inactive,
        "byType": by_type,
        "byPriority": by_priority,
    }))
}

fn group_map(rows: Vec<(String, i64)>) -> Value {
    let mut m = Map::new();
    for (k, n) in rows {
        m.insert(k, json!(n));
    }
    Value::Object(m)
}

/// Aggregate statistics, optionally scoped to one conversation (CRD 420-431).
pub async fn statistics(
    db: &SqlitePool,
    conversation_id: Option<&str>,
) -> Result<Value, AppError> {
    let (clause, binds): (&str, Vec<String>) = match conversation_id {
        Some(cid) => ("WHERE conversation_id = ?", vec![cid.to_string()]),
        None => ("", Vec::new()),
    };
    let sql = format!(
        "SELECT COUNT(*) ,
                COALESCE(SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN is_active = 0 THEN 1 ELSE 0 END), 0),
                COALESCE(AVG(message_count), 0),
                COALESCE(AVG((julianday(COALESCE(ended_at, last_activity_at, created_at))
                              - julianday(COALESCE(started_at, created_at))) * 1440.0), 0)
         FROM conversation_sessions {clause}"
    );
    let mut q = sqlx::query_as::<_, (i64, i64, i64, f64, f64)>(&sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    let (total, active, inactive, avg_messages, avg_duration) = q.fetch_one(db).await?;

    let grouped = |col: &str| {
        format!(
            "SELECT COALESCE({col}, 'unspecified'), COUNT(*) FROM conversation_sessions {clause}
             GROUP BY 1 ORDER BY 2 DESC"
        )
    };
    let mut maps: Vec<Value> = Vec::new();
    for col in ["session_type", "priority", "sentiment"] {
        let sql = grouped(col);
        let mut q = sqlx::query_as::<_, (String, i64)>(&sql);
        for b in &binds {
            q = q.bind(b.clone());
        }
        maps.push(group_map(q.fetch_all(db).await?));
    }

    let topic_sql = format!(
        "SELECT COALESCE(topic, 'unspecified'), COUNT(*) FROM conversation_sessions {clause}
         GROUP BY 1 ORDER BY 2 DESC"
    );
    let mut q = sqlx::query_as::<_, (String, i64)>(&topic_sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    let topics: Vec<Value> = q
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|(topic, count)| {
            let pct = if total == 0 { 0.0 } else { (count as f64 / total as f64) * 100.0 };
            json!({ "topic": topic, "count": count, "percentage": (pct * 100.0).round() / 100.0 })
        })
        .collect();

    let per_day_sql = format!(
        "SELECT substr(created_at, 1, 10), COUNT(*),
                COALESCE(SUM(CASE WHEN is_active = 0 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(message_count), 0)
         FROM conversation_sessions {clause} GROUP BY 1 ORDER BY 1 DESC LIMIT 30"
    );
    let mut q = sqlx::query_as::<_, (String, i64, i64, i64)>(&per_day_sql);
    for b in &binds {
        q = q.bind(b.clone());
    }
    let per_day: Vec<Value> = q
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|(date, sessions, ended, messages)| {
            json!({ "date": date, "sessions": sessions, "ended": ended, "messages": messages })
        })
        .collect();

    Ok(json!({
        "total": total,
        "active": active,
        "inactive": inactive,
        "avgMessagesPerSession": (avg_messages * 100.0).round() / 100.0,
        "avgDurationMinutes": (avg_duration * 100.0).round() / 100.0,
        "byType": maps[0],
        "byPriority": maps[1],
        "bySentiment": maps[2],
        "topicDistribution": topics,
        "perDay": per_day,
    }))
}

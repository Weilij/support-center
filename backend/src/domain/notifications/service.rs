//! Notification creation + real-time push shared by HTTP handlers, internal
//! triggers, and reminder firing (CRD 4894, 5094).

use serde_json::{json, Map, Value};
use sqlx::PgPool;

use crate::db::now_iso;
use crate::state::AppState;

pub const TYPES: &[&str] = &[
    "new_message",
    "conversation_assigned",
    "conversation_transferred",
    "mention",
    "system",
    "priority_changed",
    "customer_responded",
    "task_reminder",
    "agent_removed",
    "customer_followed",
    "new_conversation",
];
pub const PRIORITIES: &[&str] = &["low", "normal", "high", "urgent"];
pub const CHANNELS: &[&str] = &["database", "realtime", "email", "push", "webhook", "sms"];

/// Strip HTML/script-like markup from string values in the data bag (CRD 4911).
pub fn sanitize_data(value: &Value) -> Value {
    match value {
        Value::String(s) => {
            let mut out = String::with_capacity(s.len());
            let mut in_tag = false;
            for c in s.chars() {
                match c {
                    '<' => in_tag = true,
                    '>' => in_tag = false,
                    _ if !in_tag => out.push(c),
                    _ => {}
                }
            }
            Value::String(out)
        }
        Value::Object(map) => {
            let mut clean = Map::new();
            for (k, v) in map {
                clean.insert(k.clone(), sanitize_data(v));
            }
            Value::Object(clean)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_data).collect()),
        other => other.clone(),
    }
}

pub struct NewNotification<'a> {
    pub recipient: &'a str,
    pub kind: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub data: Option<Value>,
    pub priority: &'a str,
    pub expires_at: Option<String>,
}

/// Persist one notification and push it in real time (best-effort).
pub async fn create(state: &AppState, n: NewNotification<'_>) -> sqlx::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let data = n.data.map(|d| sanitize_data(&d).to_string());
    sqlx::query(
        "INSERT INTO notifications
            (id, agent_id, type, title, content, data, priority, is_read, expires_at, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 0, $8, $9, $10)",
    )
    .bind(&id)
    .bind(n.recipient)
    .bind(n.kind)
    .bind(n.title)
    .bind(n.content)
    .bind(&data)
    .bind(n.priority)
    .bind(&n.expires_at)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    // Real-time delivery is best-effort; the persisted record is the source
    // of truth on next inbox fetch (CRD 5094).
    state.realtime.to_user(
        n.recipient,
        "notification",
        json!({
            "id": id,
            "type": n.kind,
            "title": n.title,
            "content": n.content,
            "priority": n.priority,
            "data": data.as_deref().and_then(|d| serde_json::from_str::<Value>(d).ok()),
            "createdAt": now,
        }),
    );
    Ok(id)
}

pub fn expiry(hours: i64) -> Option<String> {
    Some(
        (chrono::Utc::now() + chrono::Duration::hours(hours))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )
}

/// All active (non-deleted, enabled) staff ids.
pub async fn active_staff(db: &PgPool) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT id FROM agents WHERE deleted_at IS NULL AND is_active = 1")
        .fetch_all(db)
        .await
}

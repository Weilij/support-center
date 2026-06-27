use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::messaging::store::{self, FullMessage};

const EXPORT_FILTER_CAP: i64 = 100;
const EXPORT_MAX: i64 = 1000;
const EXPORT_DEFAULT: i64 = 100;

pub async fn export_customers(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let rows: Vec<(i64, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, display_name, platform, platform_user_id FROM customers
         WHERE deleted_at IS NULL ORDER BY display_name LIMIT $1",
    )
    .bind(EXPORT_FILTER_CAP)
    .fetch_all(&state.db)
    .await?;
    let data: Vec<Value> = rows
        .iter()
        .map(|(id, name, platform, puid)| {
            json!({ "id": id, "displayName": name, "platform": platform, "platformUserId": puid })
        })
        .collect();
    Ok(envelope::ok(data))
}

pub async fn export_agents(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, display_name, role FROM agents
         WHERE deleted_at IS NULL AND is_active = 1 ORDER BY display_name LIMIT $1",
    )
    .bind(EXPORT_FILTER_CAP)
    .fetch_all(&state.db)
    .await?;
    let data: Vec<Value> = rows
        .iter()
        .map(|(id, name, role)| json!({ "id": id, "displayName": name, "role": role }))
        .collect();
    Ok(envelope::ok(data))
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub format: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "dateFrom")]
    pub date_from: Option<String>,
    #[serde(rename = "dateTo")]
    pub date_to: Option<String>,
    #[serde(rename = "customerId")]
    pub customer_id: Option<String>,
    #[serde(rename = "agentId")]
    pub agent_id: Option<String>,
    pub limit: Option<String>,
}

/// Recalled messages are always excluded from exports (CRD 922, 928).
fn export_clause(q: &ExportQuery) -> (String, Vec<String>) {
    let mut clause = String::from("m.deleted_at IS NULL AND m.is_recalled = 0");
    let mut binds = Vec::new();
    if let Some(cid) = q.conversation_id.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.conversation_id = ?");
        binds.push(cid.to_string());
    }
    if let Some(f) = q.date_from.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.created_at >= ?");
        binds.push(f.to_string());
    }
    if let Some(t) = q.date_to.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.created_at <= ?");
        binds.push(t.to_string());
    }
    if let Some(c) = q.customer_id.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.customer_id = ?");
        binds.push(c.to_string());
    }
    if let Some(a) = q.agent_id.as_deref().filter(|s| !s.is_empty()) {
        clause.push_str(" AND m.agent_id = ?");
        binds.push(a.to_string());
    }
    (clause, binds)
}

pub async fn export_count(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ExportQuery>,
) -> Result {
    let (clause, binds) = export_clause(&q);
    let sql = format!("SELECT COUNT(*) FROM messages m WHERE {clause}");
    let sql = crate::db::pg_params(&sql);
    let mut cq = sqlx::query_scalar::<_, i64>(&sql);
    for b in &binds {
        cq = cq.bind(b);
    }
    let count = cq.fetch_one(&state.db).await?;
    Ok(envelope::ok(json!({
        "count": count,
        "limit": EXPORT_MAX,
        "willBeTruncated": count > EXPORT_MAX,
    })))
}

fn csv_escape(field: &str) -> String {
    if field.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Localized-time rendering for the TXT transcript (CRD 929).
fn localized_time(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

pub async fn export_messages(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ExportQuery>,
) -> Result {
    let format = q.format.as_deref().unwrap_or("json");
    if !["json", "csv", "txt"].contains(&format) {
        return Err(AppError::BadRequest(
            "Invalid format. Valid formats are: json, csv, txt".into(),
        ));
    }
    let limit = q
        .limit
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(EXPORT_DEFAULT)
        .clamp(1, EXPORT_MAX);

    let (clause, binds) = export_clause(&q);
    let sql = format!(
        "{} WHERE {clause} ORDER BY m.created_at DESC, m.id DESC LIMIT $1",
        store::MESSAGE_SELECT
    );
    let sql = crate::db::pg_params(&sql);
    let mut mq = sqlx::query_as::<_, FullMessage>(&sql);
    for b in &binds {
        mq = mq.bind(b);
    }
    let rows = mq.bind(limit).fetch_all(&state.db).await?;
    let now = crate::db::now_iso();

    match format {
        "json" => {
            let messages: Vec<Value> = rows
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "conversationId": m.conversation_id,
                        "senderType": m.sender_type,
                        "senderName": store::resolved_sender_name(m),
                        "content": m.content,
                        "messageType": m.content_type,
                        "createdAt": m.created_at,
                    })
                })
                .collect();
            Ok(envelope::ok(json!({
                "messages": messages,
                "exportInfo": {
                    "format": "json",
                    "totalRecords": rows.len(),
                    "exportedAt": now,
                    "exportedBy": user.id,
                    "filters": {
                        "conversationId": q.conversation_id,
                        "dateFrom": q.date_from,
                        "dateTo": q.date_to,
                        "customerId": q.customer_id,
                        "agentId": q.agent_id,
                    },
                },
            })))
        }
        "csv" => {
            let mut out = String::from(
                "id,conversationId,senderType,senderName,content,messageType,createdAt\n",
            );
            for m in &rows {
                out.push_str(&format!(
                    "{},{},{},{},{},{},{}\n",
                    csv_escape(&m.id),
                    csv_escape(&m.conversation_id),
                    csv_escape(&m.sender_type),
                    csv_escape(&store::resolved_sender_name(m).unwrap_or_default()),
                    csv_escape(m.content.as_deref().unwrap_or_default()),
                    csv_escape(&m.content_type),
                    csv_escape(&m.created_at),
                ));
            }
            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!(
                            "attachment; filename=\"messages_export_{}.csv\"",
                            chrono::Utc::now().timestamp()
                        ),
                    ),
                ],
                out,
            )
                .into_response())
        }
        _ => {
            let mut groups: BTreeMap<String, Vec<&FullMessage>> = BTreeMap::new();
            for m in &rows {
                groups.entry(m.conversation_id.clone()).or_default().push(m);
            }
            let mut out = String::new();
            for (conversation_id, mut group) in groups {
                group.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
                out.push_str(&format!("Conversation: {conversation_id}\n"));
                for m in group {
                    out.push_str(&format!(
                        "[{}] {}: {}\n",
                        localized_time(&m.created_at),
                        store::resolved_sender_name(m).unwrap_or_else(|| "Unknown".into()),
                        m.content.as_deref().unwrap_or_default(),
                    ));
                }
                out.push('\n');
            }
            Ok((
                StatusCode::OK,
                [
                    (
                        header::CONTENT_TYPE,
                        "text/plain; charset=utf-8".to_string(),
                    ),
                    (
                        header::CONTENT_DISPOSITION,
                        format!(
                            "attachment; filename=\"messages_export_{}.txt\"",
                            chrono::Utc::now().timestamp()
                        ),
                    ),
                ],
                out,
            )
                .into_response())
        }
    }
}

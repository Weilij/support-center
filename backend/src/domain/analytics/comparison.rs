//! Period comparison family (CRD 4294-4325).

use axum::extract::{Query, State};
use axum::response::Response;
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

#[derive(Deserialize)]
pub struct ComparisonQuery {
    pub metric: Option<String>,
    pub metrics: Option<String>,
    #[serde(rename = "currentStart")]
    pub current_start: Option<String>,
    #[serde(rename = "currentEnd")]
    pub current_end: Option<String>,
    #[serde(rename = "previousStart")]
    pub previous_start: Option<String>,
    #[serde(rename = "previousEnd")]
    pub previous_end: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
}

struct Periods {
    cur_s: chrono::DateTime<chrono::Utc>,
    cur_e: chrono::DateTime<chrono::Utc>,
    prev_s: chrono::DateTime<chrono::Utc>,
    prev_e: chrono::DateTime<chrono::Utc>,
}

fn parse(q: &ComparisonQuery) -> Result<Periods> {
    let cur_s = q
        .current_start
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .ok_or_else(|| AppError::BadRequest("Missing required parameters".into()))?
        .with_timezone(&chrono::Utc);
    let cur_e = q
        .current_end
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .ok_or_else(|| AppError::BadRequest("Missing required parameters".into()))?
        .with_timezone(&chrono::Utc);
    // Auto previous period: equal length ending one second before (CRD 4297).
    let (prev_s, prev_e) = match (
        q.previous_start.as_deref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()),
        q.previous_end.as_deref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()),
    ) {
        (Some(s), Some(e)) => (s.with_timezone(&chrono::Utc), e.with_timezone(&chrono::Utc)),
        _ => {
            let len = cur_e - cur_s;
            let prev_e = cur_s - chrono::Duration::seconds(1);
            (prev_e - len, prev_e)
        }
    };
    Ok(Periods { cur_s, cur_e, prev_s, prev_e })
}

async fn metric_value(
    state: &AppState,
    metric: &str,
    start: &chrono::DateTime<chrono::Utc>,
    end: &chrono::DateTime<chrono::Utc>,
    team: Option<i64>,
) -> f64 {
    let s = start.to_rfc3339();
    let e = end.to_rfc3339();
    let sql = match metric {
        "total_conversations" => "SELECT COUNT(*) FROM conversations WHERE deleted_at IS NULL AND created_at >= $1 AND created_at <= $2 AND ($3 IS NULL OR team_id = $3)",
        "active_conversations" => "SELECT COUNT(*) FROM conversations WHERE deleted_at IS NULL AND status != 'closed' AND created_at >= $1 AND created_at <= $2 AND ($3 IS NULL OR team_id = $3)",
        "closed_conversations" => "SELECT COUNT(*) FROM conversations WHERE deleted_at IS NULL AND status = 'closed' AND created_at >= $1 AND created_at <= $2 AND ($3 IS NULL OR team_id = $3)",
        "total_messages" => "SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL AND created_at >= $1 AND created_at <= $2 AND ($3 IS NULL OR $3 = $3)",
        "customer_messages" => "SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL AND sender_type = 'customer' AND created_at >= $1 AND created_at <= $2 AND ($3 IS NULL OR $3 = $3)",
        "agent_messages" => "SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL AND sender_type = 'agent' AND created_at >= $1 AND created_at <= $2 AND ($3 IS NULL OR $3 = $3)",
        "active_users" => "SELECT COUNT(*) FROM agents WHERE deleted_at IS NULL AND is_active = 1 AND last_active_at >= $1 AND last_active_at <= $2 AND ($3 IS NULL OR $3 = $3)",
        "total_activities" => "SELECT COUNT(*) FROM activity_logs WHERE created_at >= $1 AND created_at <= $2 AND ($3 IS NULL OR $3 = $3)",
        // Unknown / unavailable metrics report 0 (CRD 4298, 4300).
        _ => return 0.0,
    };
    sqlx::query_scalar::<_, i64>(sql)
        .bind(&s)
        .bind(&e)
        .bind(team)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0) as f64
}

fn label(s: &chrono::DateTime<chrono::Utc>, e: &chrono::DateTime<chrono::Utc>) -> String {
    format!("{} ~ {}", s.format("%Y-%m-%d"), e.format("%Y-%m-%d"))
}

fn compare(current: f64, previous: f64, p: &Periods) -> Value {
    let change = current - previous;
    // Zero prior value: 100 when current positive, else 0 (CRD 4298).
    let pct = if previous == 0.0 {
        if current > 0.0 { 100.0 } else { 0.0 }
    } else {
        change / previous * 100.0
    };
    let trend = if pct.abs() < 5.0 {
        "stable"
    } else if change > 0.0 {
        "up"
    } else {
        "down"
    };
    json!({
        "current": current,
        "previous": previous,
        "change": change,
        "changePercent": (pct * 100.0).round() / 100.0,
        "trend": trend,
        "currentPeriod": {"start": p.cur_s.to_rfc3339(), "end": p.cur_e.to_rfc3339(), "label": label(&p.cur_s, &p.cur_e)},
        "previousPeriod": {"start": p.prev_s.to_rfc3339(), "end": p.prev_e.to_rfc3339(), "label": label(&p.prev_s, &p.prev_e)},
    })
}

pub async fn single(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ComparisonQuery>,
) -> Result {
    let metric = q
        .metric
        .clone()
        .filter(|m| !m.is_empty())
        .ok_or_else(|| AppError::BadRequest("Missing required parameters".into()))?;
    let p = parse(&q)?;
    let current = metric_value(&state, &metric, &p.cur_s, &p.cur_e, q.team_id).await;
    let previous = metric_value(&state, &metric, &p.prev_s, &p.prev_e, q.team_id).await;
    Ok(envelope::ok(json!({
        "comparison": compare(current, previous, &p),
        "metadata": { "metric": metric, "computedAt": crate::db::now_iso() },
    })))
}

async fn multi_compare(
    state: &AppState,
    metrics: &[String],
    p: &Periods,
    team: Option<i64>,
) -> Value {
    let mut per_metric = Map::new();
    let (mut improved, mut declined, mut stable) = (0usize, 0usize, 0usize);
    for metric in metrics {
        let current = metric_value(state, metric, &p.cur_s, &p.cur_e, team).await;
        let previous = metric_value(state, metric, &p.prev_s, &p.prev_e, team).await;
        let entry = compare(current, previous, p);
        match entry["trend"].as_str().unwrap_or("stable") {
            "up" => improved += 1,
            "down" => declined += 1,
            _ => stable += 1,
        }
        per_metric.insert(metric.clone(), entry);
    }
    // Overall verdict (CRD 4305): majority + at least half the set.
    let half = metrics.len().div_ceil(2);
    let overall = if improved > declined && improved >= half {
        "positive"
    } else if declined > improved && declined >= half {
        "negative"
    } else {
        "neutral"
    };
    json!({
        "metrics": per_metric,
        "summary": {
            "totalMetrics": metrics.len(),
            "improved": improved,
            "declined": declined,
            "stable": stable,
            "overallTrend": overall,
        },
    })
}

pub async fn multi(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ComparisonQuery>,
) -> Result {
    let metrics: Vec<String> = q
        .metrics
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    if metrics.is_empty() {
        return Err(AppError::BadRequest("Missing required parameters".into()));
    }
    let p = parse(&q)?;
    let comparison = multi_compare(&state, &metrics, &p, q.team_id).await;
    Ok(envelope::ok(json!({
        "comparison": comparison,
        "metadata": {
            "metricCount": metrics.len(),
            "currentPeriod": {"start": p.cur_s.to_rfc3339(), "end": p.cur_e.to_rfc3339()},
            "previousPeriod": {"start": p.prev_s.to_rfc3339(), "end": p.prev_e.to_rfc3339()},
        },
    })))
}

async fn preset(
    state: Arc<AppState>,
    q: ComparisonQuery,
    name: &str,
    metrics: &[&str],
) -> Result {
    let p = parse(&q)?;
    let metric_names: Vec<String> = metrics.iter().map(|m| m.to_string()).collect();
    let comparison = multi_compare(&state, &metric_names, &p, q.team_id).await;
    Ok(envelope::ok(json!({
        "comparison": comparison,
        "metadata": { "preset": name },
    })))
}

pub async fn preset_conversation(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ComparisonQuery>,
) -> Result {
    preset(state, q, "conversation", &[
        "total_conversations", "active_conversations", "closed_conversations",
        "avg_resolution_time", "customer_satisfaction",
    ])
    .await
}

pub async fn preset_message(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ComparisonQuery>,
) -> Result {
    preset(state, q, "message", &[
        "total_messages", "customer_messages", "agent_messages",
        "avg_response_time", "messages_per_conversation",
    ])
    .await
}

pub async fn preset_user_activity(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<ComparisonQuery>,
) -> Result {
    preset(state, q, "user-activity", &[
        "active_users", "total_activities", "avg_session_duration", "user_engagement_rate",
    ])
    .await
}

pub async fn cache_stats(Extension(_user): Extension<AuthUser>) -> Result {
    Ok(envelope::ok(json!({
        "entries": 0,
        "hits": 0,
        "misses": 0,
        "hitRate": 0.0,
        "ttlSeconds": 300,
    })))
}

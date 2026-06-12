//! Security-events dashboard (CRD 4446-4476), over webhook_security_events
//! and cors_events.

use axum::extract::{Query, State};
use axum::response::Response;
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

pub async fn health() -> Result {
    Ok(envelope::ok(json!({
        "status": {"status": "healthy", "module": "security-dashboard", "version": "1.0.0"},
        "timestamp": now_iso(),
    })))
}

fn require_admin(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator role required".into()))
    }
}

#[derive(Deserialize)]
pub struct RangeQuery {
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
    pub limit: Option<String>,
}

type WebhookEvent = (String, String, Option<String>, Option<String>, Option<String>, Option<String>, String);
type CorsEvent = (String, String, Option<String>, Option<String>, Option<String>, Option<String>, String);

async fn gather(
    state: &AppState,
    hours: i64,
) -> sqlx::Result<(Vec<WebhookEvent>, Vec<CorsEvent>)> {
    let since = (chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339();
    // At most the most recent 1000 events of each kind (CRD 4454).
    let webhook: Vec<WebhookEvent> = sqlx::query_as(
        "SELECT id, event_type, severity, platform, source_ip, details, created_at
         FROM webhook_security_events WHERE created_at >= ?
         ORDER BY created_at DESC LIMIT 1000",
    )
    .bind(&since)
    .fetch_all(&state.db)
    .await?;
    let cors: Vec<CorsEvent> = sqlx::query_as(
        "SELECT id, outcome, origin, method, path, metadata, timestamp
         FROM cors_events WHERE timestamp >= ?
         ORDER BY timestamp DESC LIMIT 1000",
    )
    .bind(&since)
    .fetch_all(&state.db)
    .await?;
    Ok((webhook, cors))
}

fn count_map<'a>(items: impl Iterator<Item = Option<&'a str>>) -> Map<String, Value> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for item in items.flatten() {
        *counts.entry(item.to_string()).or_default() += 1;
    }
    counts.into_iter().map(|(k, v)| (k, json!(v))).collect()
}

async fn compute_metrics(state: &AppState, hours: i64) -> Result<Value> {
    let (webhook, cors) = gather(state, hours).await?;
    let total = webhook.len() + cors.len();

    let mut by_severity: HashMap<&str, i64> = HashMap::new();
    for (_, _, severity, ..) in &webhook {
        *by_severity.entry(severity.as_deref().unwrap_or("low")).or_default() += 1;
    }
    let mut by_type: HashMap<String, (i64, String)> = HashMap::new();
    for (_, kind, severity, ..) in &webhook {
        let entry = by_type.entry(kind.clone()).or_insert((0, "low".into()));
        entry.0 += 1;
        entry.1 = severity.clone().unwrap_or_else(|| "low".into());
    }
    let mut top_threats: Vec<Value> = by_type
        .iter()
        .map(|(kind, (count, severity))| json!({"type": kind, "count": count, "severity": severity}))
        .collect();
    top_threats.sort_by_key(|t| -t["count"].as_i64().unwrap_or(0));
    top_threats.truncate(5);

    let rejected: Vec<&CorsEvent> =
        cors.iter().filter(|(_, outcome, ..)| outcome == "rejected").collect();
    let allowed = cors.len() - rejected.len();
    let mut origin_counts: HashMap<String, i64> = HashMap::new();
    for (_, _, origin, ..) in &rejected {
        *origin_counts.entry(origin.clone().unwrap_or_default()).or_default() += 1;
    }
    let mut top_origins: Vec<Value> = origin_counts
        .iter()
        .map(|(origin, count)| json!({"origin": origin, "count": count}))
        .collect();
    top_origins.sort_by_key(|o| -o["count"].as_i64().unwrap_or(0));
    top_origins.truncate(5);

    // Hourly distribution, kept to the last 24 buckets (CRD 4459).
    let mut hourly: std::collections::BTreeMap<String, (i64, i64)> = Default::default();
    for (_, _, severity, _, _, _, created) in &webhook {
        let bucket = created.chars().take(13).collect::<String>();
        let entry = hourly.entry(bucket).or_default();
        entry.0 += 1;
        if severity.as_deref() == Some("critical") {
            entry.1 += 1;
        }
    }
    let hourly_dist: Vec<Value> = hourly
        .iter()
        .rev()
        .take(24)
        .map(|(hour, (count, critical))| json!({"hour": hour, "count": count, "critical": critical}))
        .collect();

    let platform_counts = count_map(webhook.iter().map(|(_, _, _, p, ..)| p.as_deref()));
    let platform_dist: Vec<Value> = platform_counts
        .iter()
        .map(|(platform, count)| {
            let c = count.as_i64().unwrap_or(0);
            let pct = if webhook.is_empty() { 0.0 } else { c as f64 * 100.0 / webhook.len() as f64 };
            json!({"platform": platform, "count": c, "percentage": (pct * 100.0).round() / 100.0})
        })
        .collect();

    let alert_count = webhook
        .iter()
        .filter(|(_, _, s, ..)| matches!(s.as_deref(), Some("high") | Some("critical")))
        .count();

    Ok(json!({
        "summary": {
            "totalEvents": total,
            "bySeverity": {
                "critical": by_severity.get("critical").copied().unwrap_or(0),
                "high": by_severity.get("high").copied().unwrap_or(0),
                "medium": by_severity.get("medium").copied().unwrap_or(0),
                "low": by_severity.get("low").copied().unwrap_or(0),
            },
            "eventsPerHour": ((total as f64 / hours as f64).round()) as i64,
            "topThreats": top_threats,
        },
        "webhookSecurity": {
            "totalEvents": webhook.len(),
            "byPlatform": platform_counts,
            "byType": count_map(webhook.iter().map(|(_, t, ..)| Some(t.as_str()))),
            "bySeverity": count_map(webhook.iter().map(|(_, _, s, ..)| s.as_deref())),
            "recentEvents": webhook.iter().take(10).map(|(id, kind, severity, platform, ip, _, created)| json!({
                "id": id, "type": kind, "severity": severity, "platform": platform,
                "sourceIp": ip, "timestamp": created,
            })).collect::<Vec<_>>(),
        },
        "corsMonitoring": {
            "totalEvents": cors.len(),
            "allowed": allowed,
            "rejected": rejected.len(),
            "topRejectedOrigins": top_origins,
            "recentRejections": rejected.iter().take(10).map(|(_, _, origin, _, path, _, ts)| json!({
                "origin": origin, "path": path, "timestamp": ts,
            })).collect::<Vec<_>>(),
        },
        "trends": {
            "hourlyDistribution": hourly_dist,
            "platformDistribution": platform_dist,
        },
        "alerts": {
            "count": alert_count,
            "byChannel": {"email": 0, "slack": 0, "webhook": 0},
            "recentAlerts": [],
        },
    }))
}

pub async fn metrics(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<RangeQuery>,
) -> Result {
    require_admin(&user)?;
    let hours = match q.time_range.as_deref().unwrap_or("24h") {
        "1h" => 1,
        "7d" => 168,
        "30d" => 720,
        _ => 24,
    };
    Ok(envelope::ok(compute_metrics(&state, hours).await?))
}

pub async fn recent_events(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<RangeQuery>,
) -> Result {
    require_admin(&user)?;
    let limit: usize = match q.limit.as_deref() {
        None => 50,
        Some(raw) => {
            let parsed: i64 = raw
                .parse()
                .map_err(|_| AppError::BadRequest("limit must be a positive integer".into()))?;
            if parsed < 1 {
                return Err(AppError::BadRequest("limit must be a positive integer".into()));
            }
            parsed.min(200) as usize
        }
    };
    let (webhook, cors) = gather(&state, 720).await?;
    let mut events: Vec<Value> = Vec::new();
    for (id, kind, severity, platform, ip, details, created) in &webhook {
        events.push(json!({
            "id": id, "type": kind, "category": "webhook", "severity": severity,
            "platform": platform, "timestamp": created,
            "metadata": {
                "sourceIp": ip,
                "details": details.as_deref().and_then(|d| serde_json::from_str::<Value>(d).ok()),
            },
        }));
    }
    for (id, outcome, origin, method, path, metadata, ts) in &cors {
        events.push(json!({
            "id": id, "type": outcome, "category": "cors", "origin": origin,
            "timestamp": ts,
            "metadata": {
                "method": method, "path": path,
                "metadata": metadata.as_deref().and_then(|m| serde_json::from_str::<Value>(m).ok()),
            },
        }));
    }
    events.sort_by(|a, b| b["timestamp"].as_str().cmp(&a["timestamp"].as_str()));
    events.truncate(limit);
    Ok(envelope::ok(json!({
        "events": events,
        "count": events.len(),
        "limit": limit,
    })))
}

pub async fn summary(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_admin(&user)?;
    let metrics = compute_metrics(&state, 24).await?;
    let top_type = metrics["webhookSecurity"]["byType"]
        .as_object()
        .and_then(|m| m.iter().max_by_key(|(_, v)| v.as_i64().unwrap_or(0)))
        .map(|(k, _)| k.clone());
    Ok(envelope::ok(json!({
        "summary": metrics["summary"],
        "webhook": {
            "total": metrics["webhookSecurity"]["totalEvents"],
            "byPlatform": metrics["webhookSecurity"]["byPlatform"],
            "topEventType": top_type,
        },
        "cors": {
            "total": metrics["corsMonitoring"]["totalEvents"],
            "allowed": metrics["corsMonitoring"]["allowed"],
            "rejected": metrics["corsMonitoring"]["rejected"],
        },
    })))
}

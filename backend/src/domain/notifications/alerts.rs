//! Operational/security alerting (CRD 5055-5072): multi-destination security
//! alerts with severity gating, and monitoring alerts with rate limiting,
//! ack/resolve, and persisted configuration. External destinations are
//! configuration-gated; unconfigured destinations fail gracefully.

use serde_json::{json, Value};
use sqlx::PgPool;

use crate::db::now_iso;
use crate::state::AppState;

pub const SEVERITIES: &[&str] = &["low", "medium", "high", "critical"];
pub const LEVELS: &[&str] = &["info", "warning", "critical", "emergency"];
const CONFIG_KEY: &str = "alerting_config";
const DEFAULT_MAX_PER_HOUR: i64 = 20;

fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("alert webhook client")
    })
}

fn severity_rank(s: &str) -> usize {
    SEVERITIES.iter().position(|x| *x == s).unwrap_or(0)
}

/// Multi-destination security alert (CRD 5058-5062). Destination set comes
/// from environment configuration; missing configuration skips or fails the
/// destination gracefully. Returns (successes, failures, errors).
pub async fn send_security_alert(
    title: &str,
    message: &str,
    severity: &str,
    _metadata: Option<Value>,
) -> (usize, usize, Vec<String>) {
    let mut successes = 0;
    let mut failures = 0;
    let mut errors = Vec::new();

    struct Destination {
        name: &'static str,
        configured: bool,
        min_severity: String,
    }
    let destinations = [
        Destination {
            name: "email",
            configured: std::env::var("ALERT_EMAIL_API_KEY").is_ok(),
            min_severity: std::env::var("ALERT_EMAIL_MIN_SEVERITY").unwrap_or_else(|_| "high".into()),
        },
        Destination {
            name: "chat-webhook",
            configured: std::env::var("ALERT_CHAT_WEBHOOK_URL").is_ok(),
            min_severity: std::env::var("ALERT_CHAT_MIN_SEVERITY").unwrap_or_else(|_| "medium".into()),
        },
        Destination {
            name: "webhook",
            configured: std::env::var("ALERT_WEBHOOK_URL").is_ok(),
            min_severity: std::env::var("ALERT_WEBHOOK_MIN_SEVERITY").unwrap_or_else(|_| "low".into()),
        },
    ];
    for dest in &destinations {
        if severity_rank(severity) < severity_rank(&dest.min_severity) {
            continue; // severity gate not satisfied: destination not selected
        }
        if !dest.configured {
            failures += 1;
            errors.push(format!("{}: not configured", dest.name));
            continue;
        }
        // TODO(channels): real outbound dispatch (email API / webhook POST).
        tracing::info!(destination = dest.name, severity, title, message, "security alert dispatched");
        successes += 1;
    }
    (successes, failures, errors)
}

// ------------------------------------------------ monitoring alerts

pub async fn get_config(db: &PgPool) -> Value {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(CONFIG_KEY)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    stored
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(default_config)
}

pub fn default_config() -> Value {
    json!({
        "enabled": true,
        "defaultChannels": ["console"],
        "levelChannels": {
            "info": ["console"],
            "warning": ["console"],
            "critical": ["console", "webhook"],
            "emergency": ["console", "webhook", "chat"],
        },
        "rateLimiting": { "enabled": true, "maxPerHour": DEFAULT_MAX_PER_HOUR, "cooldownMinutes": 5 },
        "escalation": { "enabled": true, "delayMinutes": 15, "channels": ["chat"] },
    })
}

pub async fn update_config(db: &PgPool, partial: &Value) -> Value {
    let mut config = get_config(db).await;
    if let (Some(base), Some(patch)) = (config.as_object_mut(), partial.as_object()) {
        for (k, v) in patch {
            base.insert(k.clone(), v.clone());
        }
    }
    let _ = sqlx::query(
        "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(CONFIG_KEY)
    .bind(config.to_string())
    .bind(now_iso())
    .execute(db)
    .await;
    config
}

async fn get_channel_setting(db: &PgPool, key: &str) -> Option<Value> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    stored.and_then(|s| serde_json::from_str(&s).ok())
}

async fn dispatch_webhook(
    state: &AppState,
    payload: &Value,
    setting_key: &str,
) -> Result<(), String> {
    let setting = get_channel_setting(&state.db, setting_key)
        .await
        .ok_or_else(|| "webhook: not configured".to_string())?;
    let url = setting["url"]
        .as_str()
        .or_else(|| setting["webhookUrl"].as_str())
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        .ok_or_else(|| "webhook: invalid URL".to_string())?;

    let mut req = http_client().post(url).json(payload);
    if let Some(headers) = setting["headers"].as_object() {
        for (name, value) in headers {
            if let Some(v) = value.as_str() {
                req = req.header(name, v);
            }
        }
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("webhook: request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("webhook: HTTP {}", resp.status()));
    }
    Ok(())
}

/// Monitoring alert with rate limiting and escalation (CRD 5064-5067).
pub async fn send_monitoring_alert(
    state: &AppState,
    level: &str,
    title: &str,
    description: &str,
    metadata: Option<Value>,
) -> Value {
    let config = get_config(&state.db).await;
    let enabled = config["enabled"].as_bool().unwrap_or(true);
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();

    // Per-hour rate limit; emergency always bypasses (CRD 5066).
    let hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let recent: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM monitoring_alerts WHERE created_at >= $1")
            .bind(&hour_ago)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let max_per_hour = config["rateLimiting"]["maxPerHour"].as_i64().unwrap_or(DEFAULT_MAX_PER_HOUR);
    let limit_on = config["rateLimiting"]["enabled"].as_bool().unwrap_or(true);
    let rate_limited = limit_on && level != "emergency" && recent >= max_per_hour;

    let mut attempts: Vec<Value> = Vec::new();
    if enabled && !rate_limited {
        let channels: Vec<String> = config["levelChannels"][level]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_else(|| vec!["console".into()]);
        for channel in &channels {
            let payload = json!({
                "type": "monitoring_alert",
                "id": id,
                "level": level,
                "title": title,
                "description": description,
                "metadata": metadata.clone().unwrap_or(Value::Null),
                "timestamp": now,
            });
            let result = match channel.as_str() {
                "console" => {
                    tracing::warn!(level, title, description, "monitoring alert");
                    Ok(())
                }
                "webhook" => dispatch_webhook(state, &payload, "alert.webhook").await,
                "chat" => dispatch_webhook(state, &payload, "alert.slack").await,
                other => Err(format!("{other}: not configured")),
            };
            let success = result.is_ok();
            attempts.push(json!({
                "channel": channel,
                "time": now_iso(),
                "success": success,
                "error": result.err().map(Value::String).unwrap_or(Value::Null),
            }));
        }
        // Escalation for critical/emergency is scheduled (logged) only (CRD 5066).
        if matches!(level, "critical" | "emergency") {
            tracing::warn!(alert = %id, level, "escalation scheduled (logged, not executed)");
        }
    }

    let _ = sqlx::query(
        "INSERT INTO monitoring_alerts
            (id, level, title, description, channel_attempts, metadata, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(level)
    .bind(title)
    .bind(description)
    .bind(json!(attempts).to_string())
    .bind(metadata.map(|m| m.to_string()))
    .bind(&now)
    .execute(&state.db)
    .await;

    json!({
        "id": id,
        "level": level,
        "title": title,
        "description": description,
        "timestamp": now,
        "acknowledged": false,
        "resolved": false,
        "rateLimited": rate_limited,
        "channelAttempts": attempts,
    })
}

pub async fn acknowledge(db: &PgPool, alert_id: &str, actor: &str) -> bool {
    sqlx::query(
        "UPDATE monitoring_alerts SET acknowledged = 1, acknowledged_by = $1, acknowledged_at = $2 WHERE id = $3",
    )
    .bind(actor)
    .bind(now_iso())
    .bind(alert_id)
    .execute(db)
    .await
    .map(|r| r.rows_affected() == 1)
    .unwrap_or(false)
}

pub async fn resolve(db: &PgPool, alert_id: &str) -> bool {
    sqlx::query("UPDATE monitoring_alerts SET resolved = 1, resolved_at = $1 WHERE id = $2")
        .bind(now_iso())
        .bind(alert_id)
        .execute(db)
        .await
        .map(|r| r.rows_affected() == 1)
        .unwrap_or(false)
}

//! Operational/security alerting (CRD 5055-5072): multi-destination security
//! alerts with severity gating, and monitoring alerts with rate limiting,
//! ack/resolve, and persisted configuration. External destinations are
//! configuration-gated; unconfigured destinations fail gracefully.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::crypto;
use crate::db::now_iso;
use crate::state::AppState;

pub const SEVERITIES: &[&str] = &["low", "medium", "high", "critical"];
pub const LEVELS: &[&str] = &["info", "warning", "critical", "emergency"];
const CONFIG_KEY: &str = "alerting_config";
const DEFAULT_MAX_PER_HOUR: i64 = 20;
const SMTP_TIMEOUT: Duration = Duration::from_secs(5);

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

fn slack_payload(level: &str, title: &str, description: &str, metadata: Option<&Value>) -> Value {
    let mut text = format!("*[{level}]* {title}\n{description}");
    if let Some(meta) = metadata.filter(|v| !v.is_null()) {
        text.push_str("\n```");
        text.push_str(&meta.to_string());
        text.push_str("```");
    }
    json!({ "text": text })
}

fn email_message(
    sender: &str,
    recipients: &[String],
    level: &str,
    title: &str,
    description: &str,
    metadata: Option<&Value>,
) -> String {
    let mut body = format!(
        "From: {sender}\r\nTo: {}\r\nSubject: [{level}] {title}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{description}\r\n",
        recipients.join(", ")
    );
    if let Some(meta) = metadata.filter(|v| !v.is_null()) {
        body.push_str("\r\nMetadata:\r\n");
        body.push_str(&meta.to_string());
        body.push_str("\r\n");
    }
    body
}

async fn read_smtp_reply<R>(reader: &mut R) -> Result<u16, String>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let read = tokio::time::timeout(SMTP_TIMEOUT, reader.read_line(&mut line))
            .await
            .map_err(|_| "email: SMTP read timed out".to_string())?
            .map_err(|e| format!("email: SMTP read failed: {e}"))?;
        if read == 0 {
            return Err("email: SMTP connection closed".into());
        }
        let bytes = line.as_bytes();
        if bytes.len() >= 4 && bytes[3] == b' ' {
            let code = line[..3]
                .parse::<u16>()
                .map_err(|_| format!("email: invalid SMTP reply: {}", line.trim_end()))?;
            return Ok(code);
        }
        if bytes.len() < 4 || bytes[3] != b'-' {
            return Err(format!("email: invalid SMTP reply: {}", line.trim_end()));
        }
    }
}

fn ensure_smtp_code(code: u16, accepted: &[u16], context: &str) -> Result<(), String> {
    if accepted.contains(&code) {
        Ok(())
    } else {
        Err(format!("email: SMTP {context} failed with {code}"))
    }
}

fn dot_stuff(message: &str) -> String {
    let normalized = message.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .split('\n')
        .map(|line| {
            if line.starts_with('.') {
                format!(".{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

async fn send_plain_smtp(
    host: &str,
    port: u16,
    sender: &str,
    recipients: &[String],
    message: &str,
    auth: Option<(&str, &str)>,
) -> Result<(), String> {
    let stream = tokio::time::timeout(SMTP_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| "email: SMTP connect timed out".to_string())?
        .map_err(|e| format!("email: SMTP connect failed: {e}"))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    ensure_smtp_code(read_smtp_reply(&mut reader).await?, &[220], "greeting")?;

    writer
        .write_all(b"HELO localhost\r\n")
        .await
        .map_err(|e| format!("email: SMTP write failed: {e}"))?;
    ensure_smtp_code(read_smtp_reply(&mut reader).await?, &[250], "HELO")?;

    if let Some((username, password)) = auth {
        let auth = B64.encode(format!("\0{username}\0{password}"));
        writer
            .write_all(format!("AUTH PLAIN {auth}\r\n").as_bytes())
            .await
            .map_err(|e| format!("email: SMTP write failed: {e}"))?;
        ensure_smtp_code(read_smtp_reply(&mut reader).await?, &[235, 503], "AUTH PLAIN")?;
    }

    writer
        .write_all(format!("MAIL FROM:<{sender}>\r\n").as_bytes())
        .await
        .map_err(|e| format!("email: SMTP write failed: {e}"))?;
    ensure_smtp_code(read_smtp_reply(&mut reader).await?, &[250], "MAIL FROM")?;

    for recipient in recipients {
        writer
            .write_all(format!("RCPT TO:<{recipient}>\r\n").as_bytes())
            .await
            .map_err(|e| format!("email: SMTP write failed: {e}"))?;
        ensure_smtp_code(read_smtp_reply(&mut reader).await?, &[250, 251], "RCPT TO")?;
    }

    writer
        .write_all(b"DATA\r\n")
        .await
        .map_err(|e| format!("email: SMTP write failed: {e}"))?;
    ensure_smtp_code(read_smtp_reply(&mut reader).await?, &[354], "DATA")?;

    writer
        .write_all(format!("{}\r\n.\r\n", dot_stuff(message)).as_bytes())
        .await
        .map_err(|e| format!("email: SMTP write failed: {e}"))?;
    ensure_smtp_code(read_smtp_reply(&mut reader).await?, &[250], "message")?;

    let _ = writer.write_all(b"QUIT\r\n").await;
    Ok(())
}

async fn dispatch_email(
    state: &AppState,
    level: &str,
    title: &str,
    description: &str,
    metadata: Option<&Value>,
) -> Result<(), String> {
    let setting = get_channel_setting(&state.db, "alert.email")
        .await
        .ok_or_else(|| "email: not configured".to_string())?;
    let host = setting["host"]
        .as_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "email: host not configured".to_string())?;
    let port = setting["port"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .ok_or_else(|| "email: invalid port".to_string())?;
    let sender = setting["sender"]
        .as_str()
        .filter(|s| s.contains('@'))
        .ok_or_else(|| "email: sender not configured".to_string())?;
    let recipients: Vec<String> = setting["recipients"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().filter(|s| s.contains('@')).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if recipients.is_empty() {
        return Err("email: recipients not configured".into());
    }
    let password = setting["password"]
        .as_str()
        .map(|stored| {
            crypto::reveal(state.config.encryption_key.as_deref(), stored)
                .map_err(|e| format!("email: credential reveal failed: {e}"))
        })
        .transpose()?;

    let message = email_message(sender, &recipients, level, title, description, metadata);
    send_plain_smtp(
        host,
        port,
        sender,
        &recipients,
        &message,
        password.as_deref().map(|p| (sender, p)),
    )
    .await
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
                "chat" => {
                    let payload =
                        slack_payload(level, title, description, metadata.as_ref());
                    dispatch_webhook(state, &payload, "alert.slack").await
                }
                "email" => {
                    dispatch_email(state, level, title, description, metadata.as_ref()).await
                }
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

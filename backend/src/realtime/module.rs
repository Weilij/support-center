//! Realtime Module — event dispatch, runtime configuration and the
//! management/monitoring surface (CRD §5.5 lines 3974-4127, 4192-4226).
//!
//! The HTTP routes mount under `/api/realtime` behind the bearer middleware
//! (CRD 3981); finer role tiers (CRD 4225-4226) are enforced per handler:
//! presence/typing/broadcast/health are open to any authenticated caller,
//! monitoring reads and assignment/notification publishing require an
//! elevated/team or administrator role, and configuration changes, statistics
//! reset and system broadcasts require the administrator role.
//!
//! The programmatic event-publishing operations (CRD 4068-4127) are not
//! standalone HTTP routes; they are the canonical publishing entry points
//! used by other parts of the system and are exposed here as functions whose
//! validation/authorization rules are part of this area's contract.

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::{is_manager_or_admin, AuthUser};
use crate::state::AppState;
use std::sync::Arc;

type Result<T = Response> = std::result::Result<T, AppError>;

/// Bounded metrics-history retention (monitoring/metrics, CRD 4034).
const METRICS_HISTORY_CAP: usize = 288;

// ----------------------------------------------------------- runtime state

/// Runtime configuration (CRD 4001-4004): per running instance, not durable
/// across restart (CRD 4010, 4223).
#[derive(Clone, Debug)]
pub struct RealtimeConfig {
    /// Delivery-version selector: automatic, legacy or current (CRD 4003).
    pub delivery_version: String,
    pub event_driven_processing: bool,
    pub queue_processing: bool,
    pub heartbeat_interval_ms: i64,
    pub connection_timeout_ms: i64,
    pub max_retries: i64,
    pub event_storage_ttl_secs: i64,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            delivery_version: "auto".into(),
            event_driven_processing: true,
            queue_processing: true,
            heartbeat_interval_ms: 30_000,
            connection_timeout_ms: 60_000,
            max_retries: 3,
            event_storage_ttl_secs: 86_400,
        }
    }
}

impl RealtimeConfig {
    pub fn to_json(&self) -> Value {
        json!({
            "deliveryVersion": self.delivery_version,
            "eventDrivenProcessing": self.event_driven_processing,
            "queueProcessing": self.queue_processing,
            "heartbeatInterval": self.heartbeat_interval_ms,
            "connectionTimeout": self.connection_timeout_ms,
            "maxRetries": self.max_retries,
            "eventStorageTtl": self.event_storage_ttl_secs,
        })
    }

    /// Merge a JSON subset over the current configuration (CRD 4008-4009).
    fn merge(&mut self, patch: &Value) {
        if let Some(v) = patch.get("deliveryVersion").and_then(Value::as_str) {
            self.delivery_version = v.to_string();
        }
        if let Some(v) = patch.get("eventDrivenProcessing").and_then(Value::as_bool) {
            self.event_driven_processing = v;
        }
        if let Some(v) = patch.get("queueProcessing").and_then(Value::as_bool) {
            self.queue_processing = v;
        }
        if let Some(v) = patch.get("heartbeatInterval").and_then(Value::as_i64) {
            self.heartbeat_interval_ms = v;
        }
        if let Some(v) = patch.get("connectionTimeout").and_then(Value::as_i64) {
            self.connection_timeout_ms = v;
        }
        if let Some(v) = patch.get("maxRetries").and_then(Value::as_i64) {
            self.max_retries = v;
        }
        if let Some(v) = patch.get("eventStorageTtl").and_then(Value::as_i64) {
            self.event_storage_ttl_secs = v;
        }
    }
}

/// Runtime event-processing statistics (CRD 4192-4193): reset on restart or
/// explicit reset, never persisted long-term.
#[derive(Default)]
struct EventStats {
    by_type: HashMap<String, u64>,
    by_priority: HashMap<String, u64>,
    by_source: HashMap<String, u64>,
    total: u64,
    succeeded: u64,
    failed: u64,
    total_processing_ms: f64,
}

/// Performance alert (CRD 4195).
#[derive(Clone)]
pub struct Alert {
    pub id: String,
    pub level: String,
    pub metric: String,
    pub threshold: f64,
    pub current_value: f64,
    pub message: String,
    pub timestamp: String,
    pub resolved: bool,
}

impl Alert {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "level": self.level,
            "metric": self.metric,
            "threshold": self.threshold,
            "currentValue": self.current_value,
            "message": self.message,
            "timestamp": self.timestamp,
            "resolved": self.resolved,
        })
    }
}

/// Per-instance management-layer state (CRD 4223: not synchronized across
/// instances).
#[derive(Default)]
pub struct ModuleState {
    config: Mutex<RealtimeConfig>,
    stats: Mutex<EventStats>,
    alerts: Mutex<Vec<Alert>>,
    metrics_history: Mutex<VecDeque<Value>>,
}

impl ModuleState {
    pub fn config(&self) -> RealtimeConfig {
        self.config.lock().clone()
    }

    pub fn merge_config(&self, patch: &Value) {
        self.config.lock().merge(patch);
    }

    /// Record one processed event into the runtime statistics.
    pub fn record_event(&self, event_type: &str, priority: &str, source: &str, ms: f64, ok: bool) {
        let mut s = self.stats.lock();
        *s.by_type.entry(event_type.to_string()).or_default() += 1;
        *s.by_priority.entry(priority.to_string()).or_default() += 1;
        *s.by_source.entry(source.to_string()).or_default() += 1;
        s.total += 1;
        if ok {
            s.succeeded += 1;
        } else {
            s.failed += 1;
        }
        s.total_processing_ms += ms;
    }

    /// Aggregated statistics view (CRD 4119-4121).
    pub fn stats_json(&self) -> Value {
        let s = self.stats.lock();
        let avg = if s.total == 0 {
            0.0
        } else {
            s.total_processing_ms / s.total as f64
        };
        let success_rate = if s.total == 0 {
            1.0
        } else {
            s.succeeded as f64 / s.total as f64
        };
        json!({
            "totalEvents": s.total,
            "byType": s.by_type,
            "byPriority": s.by_priority,
            "bySource": s.by_source,
            "averageProcessingTime": avg,
            "successRate": success_rate,
            "errorRate": if s.total == 0 { 0.0 } else { s.failed as f64 / s.total as f64 },
            "succeeded": s.succeeded,
            "failed": s.failed,
        })
    }

    pub fn reset_stats(&self) {
        *self.stats.lock() = EventStats::default();
    }

    /// Raise a performance alert (used by the monitoring layer and tests).
    pub fn raise_alert(
        &self,
        level: &str,
        metric: &str,
        threshold: f64,
        current_value: f64,
        message: &str,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.alerts.lock().push(Alert {
            id: id.clone(),
            level: level.to_string(),
            metric: metric.to_string(),
            threshold,
            current_value,
            message: message.to_string(),
            timestamp: crate::db::now_iso(),
            resolved: false,
        });
        id
    }

    /// Mark an alert resolved; false when missing or already resolved
    /// (CRD 4045-4046).
    pub fn resolve_alert(&self, alert_id: &str) -> bool {
        let mut alerts = self.alerts.lock();
        match alerts.iter_mut().find(|a| a.id == alert_id) {
            Some(a) if !a.resolved => {
                a.resolved = true;
                true
            }
            _ => false,
        }
    }

    fn alerts_view(&self, active_only: bool, limit: usize) -> (Vec<Value>, Value) {
        let alerts = self.alerts.lock();
        let mut by_level: HashMap<String, u64> = HashMap::new();
        let day_ago = chrono::Utc::now() - chrono::Duration::hours(24);
        let mut last24h = 0u64;
        for a in alerts.iter() {
            *by_level.entry(a.level.clone()).or_default() += 1;
            if chrono::DateTime::parse_from_rfc3339(&a.timestamp)
                .map(|t| t.with_timezone(&chrono::Utc) > day_ago)
                .unwrap_or(false)
            {
                last24h += 1;
            }
        }
        let summary = json!({
            "total": alerts.len(),
            "byLevel": by_level,
            "last24Hours": last24h,
        });
        let list: Vec<Value> = alerts
            .iter()
            .rev() // most recent first
            .filter(|a| !active_only || !a.resolved)
            .take(limit)
            .map(Alert::to_json)
            .collect();
        (list, summary)
    }

    fn push_metrics_point(&self, point: Value) {
        let mut h = self.metrics_history.lock();
        h.push_back(point);
        while h.len() > METRICS_HISTORY_CAP {
            h.pop_front();
        }
    }

    fn metrics_view(&self, limit: usize) -> (Value, Vec<Value>, usize) {
        let h = self.metrics_history.lock();
        let total = h.len();
        let latest = h.back().cloned().unwrap_or(Value::Null);
        let history: Vec<Value> = h.iter().rev().take(limit).cloned().collect();
        (latest, history, total)
    }
}

// ----------------------------------------------------- role helpers

/// Administrator-only operations answer "Admin access required" otherwise
/// (CRD 4001, 4226).
fn require_admin(user: &AuthUser) -> Result<()> {
    if !user.is_admin() {
        return Err(AppError::Unauthorized("Admin access required".into()));
    }
    Ok(())
}

/// Administrator or elevated/team role (CRD 4013, 4226); resolved here as
/// system administrator or a lead/supervisor team role.
fn require_elevated(user: &AuthUser) -> Result<()> {
    if !is_manager_or_admin(user) {
        return Err(AppError::Unauthorized("Insufficient permissions".into()));
    }
    Ok(())
}

fn body_value(body: Option<Json<Value>>) -> Value {
    body.map(|Json(v)| v).unwrap_or(Value::Null)
}

/// Conversation identifiers travel as numbers or strings (CRD 3986).
fn id_present(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Number(_)) => true,
        Some(Value::String(s)) => !s.is_empty(),
        _ => false,
    }
}

// --------------------------------------------------- lightweight endpoints

/// POST /api/realtime/typing — acknowledgement-only (CRD 3984-3991): actual
/// typing propagation travels over the persistent realtime channel.
pub async fn typing(Extension(_user): Extension<AuthUser>, body: Option<Json<Value>>) -> Result {
    let body = body_value(body);
    if !id_present(body.get("conversationId")) {
        return Err(AppError::BadRequest("Conversation ID is required".into()));
    }
    Ok(envelope::message_only("Typing status received"))
}

/// POST /api/realtime/broadcast — acknowledgement-only custom-event publish
/// (CRD 3993-3998).
pub async fn broadcast(Extension(_user): Extension<AuthUser>, body: Option<Json<Value>>) -> Result {
    let body = body_value(body);
    if !id_present(body.get("conversationId")) || body.get("event").is_none() {
        return Err(AppError::BadRequest(
            "Conversation ID and event are required".into(),
        ));
    }
    Ok(envelope::message_only("Broadcast received"))
}

/// GET /api/realtime/conversation/{id}/status — static informational response
/// (CRD 4000-4004 block "Get conversation real-time status").
pub async fn conversation_status(
    Extension(_user): Extension<AuthUser>,
    Path(_id): Path<String>,
) -> Result {
    Ok(envelope::ok_msg(
        json!({ "timestamp": crate::db::now_iso() }),
        "Use the persistent real-time channel for live conversation status",
    ))
}

/// POST /api/realtime/online-status — acknowledge and echo (CRD 3996-3999
/// block "Update presence / online status").
pub async fn online_status(
    Extension(_user): Extension<AuthUser>,
    body: Option<Json<Value>>,
) -> Result {
    let body = body_value(body);
    let is_online = body.get("isOnline").and_then(Value::as_bool);
    Ok(envelope::ok_msg(
        json!({ "isOnline": is_online }),
        "Online status updated",
    ))
}

// ------------------------------------------------- configuration endpoints

/// GET /api/realtime/config — administrator only (CRD 4000-4004).
pub async fn get_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_admin(&user)?;
    Ok(envelope::ok(state.realtime.module.config().to_json()))
}

/// PUT /api/realtime/config — merge a subset; runtime-scoped only
/// (CRD 4006-4011).
pub async fn put_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&user)?;
    state.realtime.module.merge_config(&body_value(body));
    Ok(envelope::message_only("Configuration updated"))
}

/// GET /api/realtime/stats — configuration snapshot plus timestamp
/// (CRD 4013-4017).
pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_elevated(&user)?;
    Ok(envelope::ok(json!({
        "currentConfig": state.realtime.module.config().to_json(),
        "timestamp": crate::db::now_iso(),
    })))
}

/// GET /api/realtime/health — configuration-derived health summary
/// (CRD 4019-4024).
pub async fn health(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let config = state.realtime.module.config();
    Ok(envelope::ok(json!({
        "status": "healthy",
        "deliveryVersion": config.delivery_version,
        "eventDrivenProcessing": config.event_driven_processing,
        "queueProcessing": config.queue_processing,
        "timestamp": crate::db::now_iso(),
    })))
}

// ----------------------------------------------------- monitoring endpoints

fn collect_metrics_point(state: &AppState) -> Value {
    let (total, rooms, personal) = state.realtime.connection_breakdown();
    let (attempted, delivered, failed) = state.realtime.broadcast_counters();
    let (fast_depth, slow_depth) = state.realtime.queue.depths();
    json!({
        "connections": {
            "total": total,
            "conversation": rooms,
            "personal": personal,
        },
        "eventProcessing": state.realtime.module.stats_json(),
        "queue": { "fastQueueDepth": fast_depth, "slowQueueDepth": slow_depth },
        "resources": {
            "broadcastsAttempted": attempted,
            "broadcastsDelivered": delivered,
            "sendFailures": failed,
            "uptimeSeconds": state.realtime.uptime_secs(),
        },
        "timestamp": crate::db::now_iso(),
        "collectionPeriod": 60,
    })
}

/// GET /api/realtime/monitoring/dashboard — aggregated overview
/// (CRD 4026-4030).
pub async fn monitoring_dashboard(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_elevated(&user)?;
    let events = state.realtime.module.stats_json();
    let latest = {
        let (latest, _, total) = state.realtime.module.metrics_view(1);
        if total == 0 {
            Value::Null
        } else {
            latest
        }
    };
    Ok(envelope::ok(json!({
        "service": {
            "status": "running",
            "uptime": state.realtime.uptime_secs(),
            "version": env!("CARGO_PKG_VERSION"),
        },
        "performance": {
            "averageProcessingTime": events["averageProcessingTime"],
            "successRate": events["successRate"],
            "errorRate": events["errorRate"],
        },
        // Aggregate connection counters are reported as zero here: counts are
        // reported by conversation-specific streams (CRD 4029).
        "connections": {
            "transport": "realtime-channel",
            "total": 0,
            "active": 0,
        },
        "events": {
            "totalEvents": events["totalEvents"],
            "successRate": events["successRate"],
            "averageProcessingTime": events["averageProcessingTime"],
            "byType": events["byType"],
        },
        "latestMetrics": latest,
        "capabilities": {
            "queue": true,
            "kv": true,
            "database": true,
            "realtimeChannel": true,
        },
        "timestamp": crate::db::now_iso(),
    })))
}

#[derive(serde::Deserialize)]
pub struct MetricsQuery {
    pub limit: Option<usize>,
}

/// GET /api/realtime/monitoring/metrics — latest point plus bounded history
/// (CRD 4032-4036). A fresh point is collected per read.
pub async fn monitoring_metrics(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<MetricsQuery>,
) -> Result {
    require_elevated(&user)?;
    let limit = q.limit.filter(|l| *l > 0).unwrap_or(50);
    state
        .realtime
        .module
        .push_metrics_point(collect_metrics_point(&state));
    let (latest, history, total) = state.realtime.module.metrics_view(limit);
    Ok(envelope::ok(json!({
        "latest": latest,
        "history": history,
        "totalPoints": total,
    })))
}

#[derive(serde::Deserialize)]
pub struct AlertsQuery {
    pub active: Option<String>,
    pub limit: Option<usize>,
}

/// GET /api/realtime/monitoring/alerts — alert list plus summary
/// (CRD 4038-4042).
pub async fn monitoring_alerts(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<AlertsQuery>,
) -> Result {
    require_elevated(&user)?;
    let active_only = q.active.as_deref() == Some("true");
    let limit = q.limit.filter(|l| *l > 0).unwrap_or(100);
    let (alerts, summary) = state.realtime.module.alerts_view(active_only, limit);
    Ok(envelope::ok(
        json!({ "alerts": alerts, "summary": summary }),
    ))
}

/// POST /api/realtime/monitoring/alerts — resolve one alert (CRD 4044-4049).
pub async fn resolve_alert(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<Value>>,
) -> Result {
    require_elevated(&user)?;
    let body = body_value(body);
    let Some(alert_id) = body
        .get("alertId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Err(AppError::BadRequest("Alert ID is required".into()));
    };
    if !state.realtime.module.resolve_alert(alert_id) {
        return Err(AppError::NotFound(
            "Alert not found or already resolved".into(),
        ));
    }
    Ok(envelope::ok(
        json!({ "alertId": alert_id, "resolved": true }),
    ))
}

/// GET /api/realtime/monitoring/health — dependency health detail
/// (CRD 4051-4054): the legacy streaming transport always reports down (it
/// has been removed, CRD 4189), so the service status reads degraded with all
/// other dependencies healthy and error when two or more are down (CRD 4208).
pub async fn monitoring_health(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_elevated(&user)?;
    // Database probe: degraded above ~1 second (CRD 4054).
    let started = Instant::now();
    let db_ok = sqlx::query_scalar::<_, i64>("SELECT 1::bigint")
        .fetch_one(&state.db)
        .await
        .is_ok();
    let db_ms = started.elapsed().as_millis() as u64;
    let db_status = if !db_ok {
        "down"
    } else if db_ms > 1_000 {
        "degraded"
    } else {
        "healthy"
    };
    // Key-value store probe (the in-process cache layer): degraded above
    // ~half a second (CRD 4054).
    let started = Instant::now();
    let _ = state.realtime.latest.len();
    let kv_ms = started.elapsed().as_millis() as u64;
    let kv_status = if kv_ms > 500 { "degraded" } else { "healthy" };
    // Coordination/queue layer: down when unavailable (CRD 4054); the
    // in-process queue is always reachable here.
    let queue_status = "healthy";
    let down = [db_status, kv_status, queue_status, "down"]
        .iter()
        .filter(|s| **s == "down")
        .count();
    let service_status = match down {
        0 => "running",
        1 => "degraded",
        _ => "error",
    };
    Ok(envelope::ok(json!({
        "status": service_status,
        "checks": {
            "database": { "status": db_status, "responseTime": db_ms },
            "kv": { "status": kv_status, "responseTime": kv_ms },
            "queue": { "status": queue_status },
            // Removed transport (CRD 4054, 4189).
            "legacyStreaming": { "status": "down" },
        },
        "timestamp": crate::db::now_iso(),
    })))
}

/// GET|POST /api/realtime/monitoring/config — delivery-version information
/// (CRD 4056-4060).
pub async fn monitoring_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    require_elevated(&user)?;
    let config = state.realtime.module.config();
    Ok(envelope::ok(json!({
        "currentVersion": config.delivery_version,
        "availableVersions": ["auto", "legacy", "current"],
        "capabilities": {
            "eventDrivenProcessing": config.event_driven_processing,
            "queueProcessing": config.queue_processing,
            "realtimeChannel": true,
            "legacyStreaming": false,
        },
        "recommendations": {
            "auto": "Selects the current delivery version automatically",
            "legacy": "Deprecated; migrate to the current delivery version",
            "current": "Recommended delivery version",
        },
    })))
}

// -------------------------------- programmatic publishing (CRD 4068-4127)

fn finite_num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn nonempty_str(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn publish_result(event_id: String, started: Instant, extra: Option<(&str, Value)>) -> Value {
    let mut out = json!({
        "eventId": event_id,
        "processingTime": started.elapsed().as_secs_f64() * 1000.0,
    });
    if let Some((k, v)) = extra {
        out[k] = v;
    }
    out
}

/// Publish a new-message event (CRD 4068-4074): high priority to the target
/// conversation's room.
pub fn publish_message_event(state: &AppState, _user: &AuthUser, payload: &Value) -> Result<Value> {
    let started = Instant::now();
    let valid = finite_num(payload.get("messageId")).is_some()
        && finite_num(payload.get("conversationId")).is_some()
        && payload
            .get("content")
            .map(Value::is_string)
            .unwrap_or(false)
        && nonempty_str(payload.get("messageType"))
            .is_some_and(|t| ["text", "image", "file", "sticker", "location"].contains(&t))
        && nonempty_str(payload.get("senderType"))
            .is_some_and(|t| ["customer", "agent", "system"].contains(&t));
    if !valid {
        state
            .realtime
            .module
            .record_event("new_message", "high", "api", 0.0, false);
        return Err(AppError::BadRequest("Invalid message event data".into()));
    }
    let conversation_id = num_id(&payload["conversationId"]);
    let event_id = uuid::Uuid::new_v4().to_string();
    state
        .realtime
        .to_conversation(&conversation_id, "new_message", payload.clone());
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    state
        .realtime
        .module
        .record_event("new_message", "high", "api", ms, true);
    Ok(publish_result(event_id, started, None))
}

/// Publish a typing event (CRD 4076-4081): typing_started / typing_stopped,
/// low priority, the typing user excluded from delivery.
pub fn publish_typing_event(state: &AppState, _user: &AuthUser, payload: &Value) -> Result<Value> {
    let started = Instant::now();
    let valid = finite_num(payload.get("conversationId")).is_some()
        && finite_num(payload.get("userId")).is_some()
        && nonempty_str(payload.get("username")).is_some()
        && payload
            .get("isTyping")
            .map(Value::is_boolean)
            .unwrap_or(false);
    if !valid {
        return Err(AppError::BadRequest("Invalid typing event data".into()));
    }
    let conversation_id = num_id(&payload["conversationId"]);
    let user_id = num_id(&payload["userId"]);
    let event = if payload["isTyping"].as_bool().unwrap_or(false) {
        "typing_started"
    } else {
        "typing_stopped"
    };
    let event_id = uuid::Uuid::new_v4().to_string();
    state
        .realtime
        .to_conversation_except_user(&conversation_id, &user_id, event, payload.clone());
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    state
        .realtime
        .module
        .record_event(event, "low", "api", ms, true);
    Ok(publish_result(event_id, started, None))
}

/// Publish a status-change event (CRD 4083-4088): normal priority to the
/// conversation.
pub fn publish_status_change(state: &AppState, _user: &AuthUser, payload: &Value) -> Result<Value> {
    let started = Instant::now();
    let valid = finite_num(payload.get("conversationId")).is_some()
        && nonempty_str(payload.get("previousStatus")).is_some()
        && nonempty_str(payload.get("newStatus")).is_some()
        && finite_num(payload.get("changedBy")).is_some();
    if !valid {
        return Err(AppError::BadRequest("Invalid status event data".into()));
    }
    let conversation_id = num_id(&payload["conversationId"]);
    let event_id = uuid::Uuid::new_v4().to_string();
    state
        .realtime
        .to_conversation(&conversation_id, "status_changed", payload.clone());
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    state
        .realtime
        .module
        .record_event("status_changed", "normal", "api", ms, true);
    Ok(publish_result(event_id, started, None))
}

fn valid_assignee(v: Option<&Value>) -> bool {
    v.is_some_and(|a| {
        nonempty_str(a.get("type")).is_some_and(|t| t == "user" || t == "team")
            && finite_num(a.get("id")).is_some()
            && nonempty_str(a.get("name")).is_some()
    })
}

/// Publish an assignment-change event (CRD 4101-4107): elevated/team or
/// administrator role; high priority; both the prior and new assignee are
/// notified.
pub fn publish_assignment_change(
    state: &AppState,
    user: &AuthUser,
    payload: &Value,
) -> Result<Value> {
    require_elevated(user)?;
    let started = Instant::now();
    let valid = finite_num(payload.get("conversationId")).is_some()
        && valid_assignee(payload.get("assignedTo"))
        && finite_num(payload.get("assignedBy")).is_some()
        && (payload.get("previousAssignee").is_none()
            || payload["previousAssignee"].is_null()
            || valid_assignee(payload.get("previousAssignee")));
    if !valid {
        return Err(AppError::BadRequest("Invalid assignment event data".into()));
    }
    let conversation_id = num_id(&payload["conversationId"]);
    let event_id = uuid::Uuid::new_v4().to_string();
    state
        .realtime
        .to_conversation(&conversation_id, "assignment_changed", payload.clone());
    // Prior and new assignees are both added to the delivery targets
    // (CRD 4106).
    let mut targets = vec![&payload["assignedTo"]];
    if valid_assignee(payload.get("previousAssignee")) {
        targets.push(&payload["previousAssignee"]);
    }
    for assignee in targets {
        match assignee["type"].as_str() {
            Some("team") => {
                if let Some(id) = assignee["id"].as_i64() {
                    state
                        .realtime
                        .to_team(id, "assignment_changed", payload.clone());
                }
            }
            _ => {
                let uid = num_id(&assignee["id"]);
                state
                    .realtime
                    .to_user(&uid, "assignment_changed", payload.clone());
            }
        }
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    state
        .realtime
        .module
        .record_event("assignment_changed", "high", "api", ms, true);
    Ok(publish_result(event_id, started, None))
}

/// Publish a notification event (CRD 4109-4115): elevated/team or
/// administrator role; normal priority to the listed users.
pub fn publish_notification(state: &AppState, user: &AuthUser, payload: &Value) -> Result<Value> {
    require_elevated(user)?;
    let started = Instant::now();
    let targets = payload.get("targetUsers").and_then(Value::as_array);
    let valid = finite_num(payload.get("notificationId")).is_some()
        && nonempty_str(payload.get("type")).is_some()
        && nonempty_str(payload.get("title")).is_some()
        && payload
            .get("content")
            .map(Value::is_string)
            .unwrap_or(false)
        && targets.is_some_and(|t| !t.is_empty() && t.iter().all(|u| u.is_number()));
    if !valid {
        return Err(AppError::BadRequest(
            "Invalid notification event data".into(),
        ));
    }
    let targets = targets.expect("validated");
    let event_id = uuid::Uuid::new_v4().to_string();
    for target in targets {
        state
            .realtime
            .to_user(&num_id(target), "notification", payload.clone());
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    state
        .realtime
        .module
        .record_event("notification", "normal", "api", ms, true);
    Ok(publish_result(
        event_id,
        started,
        Some(("targetCount", json!(targets.len()))),
    ))
}

/// Publish a system-announcement / broadcast event (CRD 4117-4123):
/// administrator only; priority derived from severity; global unless scoped
/// to affected users.
pub fn publish_system_event(state: &AppState, user: &AuthUser, payload: &Value) -> Result<Value> {
    require_admin(user)?;
    let started = Instant::now();
    let valid = nonempty_str(payload.get("type"))
        .is_some_and(|t| ["maintenance", "update", "alert", "info"].contains(&t))
        && nonempty_str(payload.get("message")).is_some()
        && nonempty_str(payload.get("severity"))
            .is_some_and(|s| ["low", "medium", "high", "critical"].contains(&s))
        && (payload.get("affectedUsers").is_none()
            || payload["affectedUsers"].is_null()
            || payload["affectedUsers"]
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_number)));
    if !valid {
        return Err(AppError::BadRequest("Invalid system event data".into()));
    }
    let priority = match payload["severity"].as_str() {
        Some("critical") => "urgent",
        Some("high") => "high",
        _ => "normal",
    };
    let event_id = uuid::Uuid::new_v4().to_string();
    match payload
        .get("affectedUsers")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
    {
        Some(users) => {
            for u in users {
                state
                    .realtime
                    .to_user(&num_id(u), "system_announcement", payload.clone());
            }
        }
        None => {
            state
                .realtime
                .global("system_announcement", payload.clone());
        }
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    state
        .realtime
        .module
        .record_event("system_announcement", priority, "system", ms, true);
    Ok(publish_result(event_id, started, None))
}

/// Read aggregated event statistics (CRD 4119-4121 block): elevated/team or
/// administrator role.
pub fn event_stats(state: &AppState, user: &AuthUser) -> Result<Value> {
    require_elevated(user)?;
    let mut out = state.realtime.module.stats_json();
    let (processed, succeeded, failed) = state.realtime.latest.refresh_counters();
    out["cacheRefreshes"] = json!({
        "processed": processed,
        "succeeded": succeeded,
        "failed": failed,
    });
    Ok(out)
}

/// Reset the runtime event-statistics counters (CRD 4123-4125):
/// administrator only.
pub fn reset_event_stats(state: &AppState, user: &AuthUser) -> Result<Value> {
    require_admin(user)?;
    state.realtime.module.reset_stats();
    Ok(json!({ "success": true }))
}

/// Numeric identifiers print without a fractional part when integral.
fn num_id(v: &Value) -> String {
    if let Some(i) = v.as_i64() {
        i.to_string()
    } else if let Some(f) = v.as_f64() {
        f.to_string()
    } else {
        v.as_str().unwrap_or_default().to_string()
    }
}

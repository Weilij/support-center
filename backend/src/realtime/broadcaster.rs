//! Routed event delivery (CRD §5.2 lines 3581-3660): queued broadcast with
//! priority/overflow/retry semantics, immediate targeted delivery to
//! conversations / users / teams / admins / everyone, endpoint reachability
//! registry, subscription filters, and the delivery metrics/health surface.
//!
//! Single-process implementation: delivery resolves audiences against the live
//! hub. TODO(scale-out): a multi-instance deployment would publish queued
//! events to peer instances and merge reachability registries; observable
//! behavior here is the single-instance equivalent (CRD 3542).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Weak};
use std::sync::Mutex;
use std::time::Duration;

use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::authenticate;
use crate::state::AppState;

/// Normal-queue overflow cap (CRD 3585: default cap 10000).
pub const NORMAL_QUEUE_CAP: usize = 10_000;
/// Retry ceiling for failed high/normal deliveries (CRD 3678: a small ceiling).
pub const MAX_RETRIES: u32 = 3;
/// Fast-loop interval draining the high-priority queue (CRD 3692).
const FAST_LOOP: Duration = Duration::from_millis(100);
/// Slow-loop interval draining the normal queue (CRD 3692).
const SLOW_LOOP: Duration = Duration::from_millis(2_000);

/// One queued event with its audience, options and retry accounting
/// (CRD 3670).
#[derive(Clone)]
pub struct QueuedEvent {
    pub event: Value,
    pub targets: Vec<Value>,
    pub options: Value,
    pub priority: String,
    pub queued_at: String,
    pub retry_count: u32,
    pub retry_after: Option<String>,
}

#[derive(Default)]
struct QueueState {
    normal: VecDeque<QueuedEvent>,
    high: VecDeque<QueuedEvent>,
    evicted: u64,
    total_events: u64,
    delivered: u64,
    failed: u64,
    latency_ms_total: u64,
    latency_samples: u64,
    last_processed: Option<String>,
    /// Manually registered reachable endpoints (CRD 3623-3633).
    reachable_users: HashSet<String>,
    reachable_conversations: HashSet<String>,
    registered_connections: i64,
    /// Subscription filters keyed by target (CRD 3635-3638).
    filters: HashMap<String, Value>,
    /// Queue-drain mutual exclusion flags (CRD 3672).
    draining_high: bool,
    draining_normal: bool,
}

/// Routed-delivery queue state owned by the hub.
#[derive(Default)]
pub struct BroadcastQueue {
    state: Mutex<QueueState>,
}

impl BroadcastQueue {
    /// Enqueue one event (CRD 3581-3588). High/urgent priority goes to the
    /// high-priority queue; overflow evicts the oldest normal-queue entries
    /// (low/normal priority only ever lives there). Returns the normal-queue
    /// depth after the insert.
    pub fn enqueue(&self, event: Value, targets: Vec<Value>, options: Value) -> usize {
        self.enqueue_batch(std::iter::once((event, targets, options)))
    }

    /// Bulk insert under one lock acquisition; returns the normal-queue depth
    /// after the last insert.
    pub fn enqueue_batch(
        &self,
        items: impl IntoIterator<Item = (Value, Vec<Value>, Value)>,
    ) -> usize {
        let mut s = self.state.lock().expect("queue lock");
        for (event, targets, options) in items {
            let priority = options
                .get("priority")
                .or_else(|| event.get("priority"))
                .and_then(Value::as_str)
                .unwrap_or("normal")
                .to_string();
            let item = QueuedEvent {
                event,
                targets,
                options,
                priority: priority.clone(),
                queued_at: crate::db::now_iso(),
                retry_count: 0,
                retry_after: None,
            };
            s.total_events += 1;
            if priority == "high" || priority == "urgent" {
                s.high.push_back(item);
            } else {
                s.normal.push_back(item);
                // Overflow protection: the oldest normal-queue entries are
                // evicted past the cap and counted (CRD 3585, 3588).
                while s.normal.len() > NORMAL_QUEUE_CAP {
                    s.normal.pop_front();
                    s.evicted += 1;
                }
            }
        }
        s.normal.len()
    }

    /// Take the whole queue for processing under mutual exclusion; `None`
    /// when another processor is already draining that queue (CRD 3672).
    fn take_batch(&self, high: bool) -> Option<Vec<QueuedEvent>> {
        let mut s = self.state.lock().expect("queue lock");
        let flag = if high { &mut s.draining_high } else { &mut s.draining_normal };
        if *flag {
            return None;
        }
        *flag = true;
        let batch: Vec<QueuedEvent> =
            if high { s.high.drain(..).collect() } else { s.normal.drain(..).collect() };
        Some(batch)
    }

    fn finish_drain(&self, high: bool, requeue: Vec<QueuedEvent>) {
        let mut s = self.state.lock().expect("queue lock");
        for item in requeue {
            if high {
                s.high.push_back(item);
            } else {
                s.normal.push_back(item);
            }
        }
        if high {
            s.draining_high = false;
        } else {
            s.draining_normal = false;
        }
    }

    fn record_processed(&self, delivered: u64, failed: u64, latency_ms: u64, samples: u64) {
        let mut s = self.state.lock().expect("queue lock");
        s.delivered += delivered;
        s.failed += failed;
        s.latency_ms_total += latency_ms;
        s.latency_samples += samples;
        s.last_processed = Some(crate::db::now_iso());
    }

    pub fn depths(&self) -> (usize, usize) {
        let s = self.state.lock().expect("queue lock");
        (s.normal.len(), s.high.len())
    }

    fn register(&self, kind: &str, id: &str) -> i64 {
        let mut s = self.state.lock().expect("queue lock");
        let inserted = match kind {
            "user" => s.reachable_users.insert(id.to_string()),
            _ => s.reachable_conversations.insert(id.to_string()),
        };
        if inserted {
            s.registered_connections += 1;
        }
        s.registered_connections
    }

    fn unregister(&self, kind: &str, id: &str) -> i64 {
        let mut s = self.state.lock().expect("queue lock");
        let removed = match kind {
            "user" => s.reachable_users.remove(id),
            _ => s.reachable_conversations.remove(id),
        };
        if removed {
            // The counter never goes below zero (CRD 3631).
            s.registered_connections = (s.registered_connections - 1).max(0);
        }
        s.registered_connections
    }

    fn set_filters(&self, target_key: &str, filters: Value) {
        let mut s = self.state.lock().expect("queue lock");
        s.filters.insert(target_key.to_string(), filters);
    }

    fn registry_snapshot(&self) -> (Vec<String>, Vec<String>, i64) {
        let s = self.state.lock().expect("queue lock");
        (
            s.reachable_users.iter().cloned().collect(),
            s.reachable_conversations.iter().cloned().collect(),
            s.registered_connections,
        )
    }

    fn stats_snapshot(&self) -> Value {
        let s = self.state.lock().expect("queue lock");
        let avg_latency = s.latency_ms_total.checked_div(s.latency_samples).unwrap_or(0);
        json!({
            "totalEvents": s.total_events,
            "delivered": s.delivered,
            "failed": s.failed,
            "evicted": s.evicted,
            "averageLatencyMs": avg_latency,
            "lastProcessedAt": s.last_processed,
            "normalQueueDepth": s.normal.len(),
            "highPriorityQueueDepth": s.high.len(),
            "activeExclusiveSections":
                (s.draining_high as u64) + (s.draining_normal as u64),
        })
    }
}

/// Spawn the background processing loops: a fast loop for high/urgent events
/// and a slower loop for normal/low events (CRD 3692). Both drain with mutual
/// exclusion so only one processor handles a queue at a time.
pub fn spawn_loops(state: Arc<AppState>) {
    fn spawn_loop(state: Weak<AppState>, interval: Duration, high: bool) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(state) = state.upgrade() else {
                    break;
                };
                process_queue(&state, high).await;
            }
        });
    }

    let state = Arc::downgrade(&state);
    let fast = state.clone();
    spawn_loop(fast, FAST_LOOP, true);
    spawn_loop(state, SLOW_LOOP, false);
}

/// Drain one queue and deliver every event to its audiences. Returns the
/// number of events processed. Failed high/normal-priority events are
/// re-queued with an incremented retry counter up to the ceiling; low-priority
/// events are never retried (CRD 3678).
pub async fn process_queue(state: &Arc<AppState>, high: bool) -> usize {
    let Some(batch) = state.realtime.queue.take_batch(high) else { return 0 };
    let mut processed = 0usize;
    let mut delivered = 0u64;
    let mut failed = 0u64;
    let mut latency = 0u64;
    let mut requeue = Vec::new();
    for mut item in batch {
        let started = std::time::Instant::now();
        match deliver_targets(state, &item.event, &item.targets).await {
            Ok((ok, bad)) => {
                delivered += ok;
                failed += bad;
            }
            Err(_) => {
                // Retry path (CRD 3678): low priority is abandoned outright.
                if item.priority != "low" && item.retry_count < MAX_RETRIES {
                    item.retry_count += 1;
                    item.retry_after = Some(crate::db::now_iso());
                    requeue.push(item);
                } else {
                    failed += 1;
                }
                continue;
            }
        }
        latency += started.elapsed().as_millis() as u64;
        processed += 1;
    }
    state.realtime.queue.record_processed(delivered, failed, latency, processed as u64);
    state.realtime.queue.finish_drain(high, requeue);
    processed
}

/// Resolve a team's active members: a primary-team association or any
/// multi-team membership of an active, non-deleted account (CRD 3602).
async fn team_members(state: &Arc<AppState>, team_id: i64) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT a.id FROM agents a
         JOIN team_members tm ON tm.agent_id = a.id
         WHERE tm.team_id = $1 AND a.is_active = 1",
    )
    .bind(team_id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn active_admins(state: &Arc<AppState>) -> Result<Vec<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT id FROM agents WHERE role = 'admin' AND is_active = 1")
            .fetch_all(&state.db)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

fn event_type_and_payload(event: &Value) -> (String, Value) {
    let event_type = event.get("type").and_then(Value::as_str).unwrap_or("event").to_string();
    let mut payload = event.get("data").cloned().unwrap_or_else(|| event.clone());
    if let Some(id) = event.get("id") {
        payload["eventId"] = id.clone();
    }
    (event_type, payload)
}

/// Deliver one event to a list of audience descriptors. Each descriptor names
/// an audience type (conversation / user / team / global) and identifiers.
/// Returns (successful targets, failed targets).
async fn deliver_targets(
    state: &Arc<AppState>,
    event: &Value,
    targets: &[Value],
) -> Result<(u64, u64)> {
    let (event_type, payload) = event_type_and_payload(event);
    let mut ok = 0u64;
    let mut bad = 0u64;
    for target in targets {
        let kind = target.get("type").and_then(Value::as_str).unwrap_or("global");
        let ids: Vec<Value> = match target.get("ids").and_then(Value::as_array) {
            Some(list) => list.clone(),
            None => target.get("id").cloned().into_iter().collect(),
        };
        match kind {
            "conversation" => {
                for id in &ids {
                    if let Some(cid) = id.as_str() {
                        state.realtime.to_conversation(cid, &event_type, payload.clone());
                        ok += 1;
                    } else {
                        bad += 1;
                    }
                }
            }
            "user" => {
                for id in &ids {
                    if let Some(uid) = id.as_str() {
                        state.realtime.to_user(uid, &event_type, payload.clone());
                        ok += 1;
                    } else {
                        bad += 1;
                    }
                }
            }
            "team" => {
                for id in &ids {
                    if let Some(team_id) = id.as_i64() {
                        for member in team_members(state, team_id).await? {
                            state.realtime.to_user(&member, &event_type, payload.clone());
                        }
                        ok += 1;
                    } else {
                        bad += 1;
                    }
                }
            }
            // global / everyone (and unknown audience kinds degrade to global).
            _ => {
                state.realtime.global(&event_type, payload.clone());
                ok += 1;
            }
        }
    }
    Ok((ok, bad))
}

// ----------------------------------------------------------- HTTP handlers

type Result<T = Response> = std::result::Result<T, AppError>;

/// All routed-delivery injection endpoints are trusted system surface; they
/// require an administrator credential (the CRD documents them as trusted
/// internal calls; `/api/realtime` requires bearer auth per CRD 3981).
async fn require_admin(state: &Arc<AppState>, headers: &HeaderMap) -> Result<()> {
    let user = authenticate(state, headers).await?;
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    Ok(())
}

fn body_value(body: Option<Json<Value>>) -> Value {
    body.map(|Json(v)| v).unwrap_or(Value::Null)
}

/// A well-formed event must carry an identifier, a type, a timestamp and a
/// data payload (CRD 3584).
fn validate_event(event: Option<&Value>) -> Result<Value> {
    let event = event.cloned().filter(|e| e.is_object());
    let valid = event.as_ref().is_some_and(|e| {
        e.get("id").and_then(Value::as_str).is_some()
            && e.get("type").and_then(Value::as_str).is_some()
            && e.get("timestamp").is_some()
            && e.get("data").is_some()
    });
    if !valid {
        return Err(AppError::BadRequest("Invalid event format".into()));
    }
    Ok(event.unwrap())
}

fn id_list<'a>(body: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    body.get(key)
        .or_else(|| body.get("targets"))
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest(format!("{key} must be an array")))
}

/// POST /broadcast and POST /queue-event — queue an event for routed delivery
/// (CRD 3581-3588).
pub async fn queue_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let event = validate_event(body.get("event"))?;
    let event_id = event["id"].as_str().unwrap_or_default().to_string();
    let targets = body
        .get("targets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![json!({ "type": "global" })]);
    let options = body.get("options").cloned().unwrap_or(json!({}));
    let queue_size = state.realtime.queue.enqueue(event, targets, options);
    Ok(envelope::ok(json!({
        "eventId": event_id,
        "queuedAt": crate::db::now_iso(),
        "queueSize": queue_size,
    })))
}

async fn targeted_broadcast(
    state: Arc<AppState>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
    key: &str,
    kind: &str,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let event = validate_event(body.get("event"))?;
    let ids = id_list(&body, key)?.clone();
    let started = std::time::Instant::now();
    let targets: Vec<Value> = vec![json!({ "type": kind, "ids": ids })];
    let (ok, bad) = deliver_targets(&state, &event, &targets).await?;
    state.realtime.queue.record_processed(ok, bad, started.elapsed().as_millis() as u64, 1);
    Ok(envelope::ok(json!({
        "eventId": event["id"],
        "targetCount": ids.len(),
        "successful": ok,
        "failed": bad,
        "processingTimeMs": started.elapsed().as_millis() as u64,
    })))
}

/// POST /broadcast-to-conversations (CRD 3590-3594).
pub async fn to_conversations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    targeted_broadcast(state, headers, body, "conversationIds", "conversation").await
}

/// POST /broadcast-to-users (CRD 3596-3598).
pub async fn to_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    targeted_broadcast(state, headers, body, "userIds", "user").await
}

/// POST /broadcast-to-teams (CRD 3600-3603).
pub async fn to_teams(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    targeted_broadcast(state, headers, body, "teamIds", "team").await
}

/// POST /broadcast-to-teams-and-admins (CRD 3605-3609): team data-isolation
/// while administrators still observe everything.
pub async fn to_teams_and_admins(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let event = validate_event(body.get("event"))?;
    let team_ids = id_list(&body, "teamIds")?.clone();
    let include_admins = body.get("includeAdmins").and_then(Value::as_bool).unwrap_or(true);
    let started = std::time::Instant::now();
    let targets: Vec<Value> = vec![json!({ "type": "team", "ids": team_ids })];
    let (mut ok, bad) = deliver_targets(&state, &event, &targets).await?;
    if include_admins {
        // Failure to resolve admins degrades gracefully (CRD 3609).
        if let Ok(admins) = active_admins(&state).await {
            let (event_type, payload) = event_type_and_payload(&event);
            for admin in admins {
                state.realtime.to_user(&admin, &event_type, payload.clone());
                ok += 1;
            }
        }
    }
    state.realtime.queue.record_processed(ok, bad, started.elapsed().as_millis() as u64, 1);
    Ok(envelope::ok(json!({
        "eventId": event["id"],
        "teamCount": team_ids.len(),
        "includeAdmins": include_admins,
        "successful": ok,
        "failed": bad,
        "processingTimeMs": started.elapsed().as_millis() as u64,
    })))
}

/// POST /broadcast-global (CRD 3611-3615).
pub async fn global(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let event = validate_event(body.get("event"))?;
    let target = body.get("target").and_then(Value::as_str).unwrap_or("everyone");
    let targets: Vec<Value> = vec![json!({ "type": target })];
    let (ok, bad) = deliver_targets(&state, &event, &targets).await?;
    state.realtime.queue.record_processed(ok, bad, 0, 1);
    Ok(envelope::ok(json!({
        "eventId": event["id"],
        "successful": ok,
        "failed": bad,
    })))
}

/// POST /batch-broadcast (CRD 3617-3621): one target per event; an absent
/// per-event target reuses the first target.
pub async fn batch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let events = body
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("events must be an array".into()))?;
    let targets = body
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("targets must be an array".into()))?;
    let mut processed = 0u64;
    let mut ok = 0u64;
    let mut bad = 0u64;
    for (i, raw) in events.iter().enumerate() {
        let Ok(event) = validate_event(Some(raw)) else {
            bad += 1;
            continue;
        };
        let target = targets.get(i).or_else(|| targets.first()).cloned();
        let Some(target) = target else {
            bad += 1;
            continue;
        };
        let (o, b) = deliver_targets(&state, &event, &[target]).await?;
        ok += o;
        bad += b;
        processed += 1;
    }
    state.realtime.queue.record_processed(ok, bad, 0, processed);
    Ok(envelope::ok(json!({
        "processed": processed,
        "successful": ok,
        "failed": bad,
    })))
}

/// POST /register-connection (CRD 3623-3627).
pub async fn register_connection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let (kind, id) = endpoint_descriptor(&body)?;
    let count = state.realtime.queue.register(&kind, &id);
    Ok(envelope::ok(json!({
        "activeConnections": count + state.realtime.connection_count() as i64,
    })))
}

/// POST /unregister-connection (CRD 3629-3633).
pub async fn unregister_connection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let (kind, id) = endpoint_descriptor(&body)?;
    let count = state.realtime.queue.unregister(&kind, &id);
    Ok(envelope::ok(json!({
        "activeConnections": count + state.realtime.connection_count() as i64,
    })))
}

fn endpoint_descriptor(body: &Value) -> Result<(String, String)> {
    let kind = body
        .get("type")
        .or_else(|| body.get("endpointType"))
        .and_then(Value::as_str)
        .filter(|k| *k == "conversation" || *k == "user")
        .ok_or_else(|| {
            AppError::BadRequest("type must be 'conversation' or 'user'".into())
        })?;
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("id is required".into()))?;
    Ok((kind.to_string(), id.to_string()))
}

/// POST /update-filters (CRD 3635-3638).
pub async fn update_filters(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let target_key = body
        .get("targetKey")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("targetKey is required".into()))?;
    let filters = body.get("filters").cloned().unwrap_or(json!([]));
    state.realtime.queue.set_filters(target_key, filters);
    Ok(envelope::ok(json!({ "updated": true })))
}

/// POST /flush-queue (CRD 3640-3643): force immediate processing of the
/// selected queue.
pub async fn flush_queue(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let high = body.get("priority").and_then(Value::as_str) == Some("high");
    process_queue(&state, high).await;
    let (normal, high_depth) = state.realtime.queue.depths();
    Ok(envelope::ok(json!({ "remainingEvents": normal + high_depth })))
}

/// POST /system-broadcast (CRD 3645-3648): system notification to everyone.
pub async fn system_broadcast(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result {
    require_admin(&state, &headers).await?;
    let body = body_value(body);
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| AppError::BadRequest("message is required".into()))?;
    let priority = body.get("priority").and_then(Value::as_str).unwrap_or("normal");
    let event_id = uuid::Uuid::new_v4().to_string();
    let event = json!({
        "id": event_id,
        "type": "system_notification",
        "timestamp": crate::db::now_iso(),
        "data": { "message": message, "priority": priority },
    });
    state.realtime.queue.enqueue(
        event,
        vec![json!({ "type": "global" })],
        json!({ "priority": priority }),
    );
    Ok(envelope::ok(json!({ "eventId": event_id })))
}

/// POST /metrics — routed-delivery metrics (CRD 3650-3651).
pub async fn metrics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    authenticate(&state, &headers).await?;
    let stats = state.realtime.queue.stats_snapshot();
    let (users, conversations, registered) = state.realtime.queue.registry_snapshot();
    let uptime = state.realtime.uptime_secs();
    let events_per_second = if uptime == 0 {
        0.0
    } else {
        stats["totalEvents"].as_u64().unwrap_or(0) as f64 / uptime as f64
    };
    Ok(envelope::ok(json!({
        "totalEvents": stats["totalEvents"],
        // Legacy and current delivery counter names (CRD 3651).
        "successfulDeliveries": stats["delivered"],
        "failedDeliveries": stats["failed"],
        "delivered": stats["delivered"],
        "failed": stats["failed"],
        "evicted": stats["evicted"],
        "averageLatencyMs": stats["averageLatencyMs"],
        "eventsPerSecond": events_per_second,
        "lastProcessedAt": stats["lastProcessedAt"],
        "queueSize": stats["normalQueueDepth"].as_u64().unwrap_or(0)
            + stats["highPriorityQueueDepth"].as_u64().unwrap_or(0),
        "activeConnections": state.realtime.connection_count() as i64 + registered,
        "reachableConversations": conversations.len(),
        "reachableUsers": users.len(),
        "normalQueueDepth": stats["normalQueueDepth"],
        "highPriorityQueueDepth": stats["highPriorityQueueDepth"],
        "activeExclusiveSections": stats["activeExclusiveSections"],
        "uptimeSeconds": uptime,
        "memory": { "rssBytes": Value::Null, "heapBytes": Value::Null },
    })))
}

/// POST /status and POST /health (CRD 3653-3654): degraded when the normal
/// queue exceeds 80% of its capacity.
pub async fn status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    authenticate(&state, &headers).await?;
    let stats = state.realtime.queue.stats_snapshot();
    let (normal, high) = state.realtime.queue.depths();
    let healthy = normal <= NORMAL_QUEUE_CAP * 8 / 10;
    let delivered = stats["delivered"].as_u64().unwrap_or(0);
    let failed = stats["failed"].as_u64().unwrap_or(0);
    let error_rate = if delivered + failed == 0 {
        0.0
    } else {
        failed as f64 / (delivered + failed) as f64
    };
    let uptime = state.realtime.uptime_secs();
    Ok(envelope::ok(json!({
        "healthy": healthy,
        "status": if healthy { "healthy" } else { "degraded" },
        "queueSize": normal + high,
        "processingRate": if uptime == 0 { 0.0 } else { delivered as f64 / uptime as f64 },
        "lastProcessedAt": stats["lastProcessedAt"],
        "activeConnections": state.realtime.connection_count(),
        "uptimeSeconds": uptime,
        "errorRate": error_rate,
        "averageLatencyMs": stats["averageLatencyMs"],
        "memory": { "rssBytes": Value::Null, "heapBytes": Value::Null },
        "timestamp": crate::db::now_iso(),
    })))
}

/// POST /debug-connections (CRD 3656-3657): reachable users/conversations —
/// the union of the manual registry and live hub connections.
pub async fn debug_connections(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result {
    require_admin(&state, &headers).await?;
    let (mut users, mut conversations, registered) = state.realtime.queue.registry_snapshot();
    let (live_users, live_convs) = state.realtime.reachability_snapshot();
    for u in live_users {
        if !users.contains(&u) {
            users.push(u);
        }
    }
    for c in live_convs {
        if !conversations.contains(&c) {
            conversations.push(c);
        }
    }
    Ok(envelope::ok(json!({
        "users": users,
        "conversations": conversations,
        "activeConnections": state.realtime.connection_count() as i64 + registered,
        "timestamp": crate::db::now_iso(),
    })))
}

//! Latest-Message Cache (CRD §5.5 lines 4129-4197).
//!
//! Per-conversation snapshot of the single most recent message, with a
//! 24-hour expiry, read-through population, coalesced refreshes with bounded
//! retries, explicit invalidation, and a batch warm-up. The cache is strictly
//! best-effort: every read path falls back to authoritative storage, and
//! refresh failures never block correct response generation (CRD 4168-4174).

use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::state::AppState;

/// Cache entries expire automatically after 24 hours (CRD 4131, 4173).
pub const LATEST_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// A failed refresh is retried up to a fixed maximum before being abandoned
/// (CRD 4186, 4205).
pub const REFRESH_MAX_RETRIES: usize = 3;
/// Default warm-up breadth (CRD 4160: default 50).
pub const WARMUP_DEFAULT_LIMIT: i64 = 50;

#[derive(Default)]
struct CacheInner {
    entries: HashMap<String, (Instant, Value)>,
    /// Conversations with a refresh already pending — duplicate requests are
    /// coalesced (CRD 4153, 4185).
    pending: HashSet<String>,
}

/// In-memory latest-message store plus refresh bookkeeping counters
/// (processed / succeeded / failed, CRD 4193).
#[derive(Default)]
pub struct LatestMessageCache {
    inner: Mutex<CacheInner>,
    counters: Mutex<(u64, u64, u64)>,
}

impl LatestMessageCache {
    /// Cached snapshot, if present and unexpired.
    pub fn peek(&self, conversation_id: &str) -> Option<Value> {
        let inner = self.inner.lock().ok()?;
        inner
            .entries
            .get(conversation_id)
            .filter(|(at, _)| at.elapsed() < LATEST_TTL)
            .map(|(_, v)| v.clone())
    }

    /// Store/refresh a snapshot with the 24-hour expiry (CRD 4151).
    pub fn store(&self, conversation_id: &str, snapshot: Value) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.retain(|_, (at, _)| at.elapsed() < LATEST_TTL);
            inner.entries.insert(conversation_id.to_string(), (Instant::now(), snapshot));
        }
    }

    /// Remove the cached snapshot; failures are non-fatal (CRD 4156-4158).
    pub fn invalidate(&self, conversation_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.remove(conversation_id);
        }
    }

    /// Mark a refresh pending; returns false when one is already pending
    /// (the duplicate is coalesced, CRD 4185).
    fn begin_refresh(&self, conversation_id: &str) -> bool {
        self.inner
            .lock()
            .map(|mut i| i.pending.insert(conversation_id.to_string()))
            .unwrap_or(false)
    }

    fn end_refresh(&self, conversation_id: &str, succeeded: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pending.remove(conversation_id);
        }
        if let Ok(mut c) = self.counters.lock() {
            c.0 += 1;
            if succeeded {
                c.1 += 1;
            } else {
                c.2 += 1;
            }
        }
    }

    /// (processed, succeeded, failed) refresh counters (CRD 4193).
    pub fn refresh_counters(&self) -> (u64, u64, u64) {
        self.counters.lock().map(|c| *c).unwrap_or((0, 0, 0))
    }

    /// Number of unexpired cached snapshots.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|i| i.entries.values().filter(|(at, _)| at.elapsed() < LATEST_TTL).count())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(sqlx::FromRow)]
struct LatestRow {
    id: String,
    content: Option<String>,
    created_at: String,
    sender_type: String,
    agent_id: Option<String>,
    customer_id: Option<i64>,
    content_type: String,
}

fn snapshot_json(conversation_id: &str, row: &LatestRow) -> Value {
    // Snapshot fields per CRD 4131, 4190.
    json!({
        "conversationId": conversation_id,
        "messageId": row.id,
        "content": row.content,
        "createdAt": row.created_at,
        "senderType": row.sender_type,
        "agentId": row.agent_id,
        "customerId": row.customer_id,
        "messageType": row.content_type,
        "cachedAt": crate::db::now_iso(),
    })
}

/// Derive the single most recent message by creation time from authoritative
/// storage (CRD 4138-4139).
async fn derive(state: &AppState, conversation_id: &str) -> Result<Option<Value>, sqlx::Error> {
    let row: Option<LatestRow> = sqlx::query_as(
        "SELECT id, content, created_at, sender_type, agent_id, customer_id, content_type
         FROM messages
         WHERE conversation_id = ? AND deleted_at IS NULL
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(conversation_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|r| snapshot_json(conversation_id, &r)))
}

/// Read latest message — read-through (CRD 4134-4139): cached snapshot when
/// present, otherwise derived from storage, stored, and returned. `None` when
/// the conversation has no messages.
pub async fn get_latest(state: &AppState, conversation_id: &str) -> Option<Value> {
    if let Some(hit) = state.realtime.latest.peek(conversation_id) {
        return Some(hit);
    }
    // On cache errors this is also the direct-storage fallback path
    // (CRD 4138): a derive failure simply yields absence rather than an error.
    let derived = derive(state, conversation_id).await.ok().flatten()?;
    state.realtime.latest.store(conversation_id, derived.clone());
    Some(derived)
}

/// Read latest messages for multiple conversations (CRD 4141-4145):
/// conversation id -> snapshot; conversations with no messages are omitted.
pub async fn get_latest_many(state: &AppState, conversation_ids: &[String]) -> Map<String, Value> {
    let mut out = Map::new();
    for cid in conversation_ids {
        if out.contains_key(cid) {
            continue;
        }
        if let Some(snapshot) = get_latest(state, cid).await {
            out.insert(cid.clone(), snapshot);
        }
    }
    out
}

/// One refresh pass (CRD 4203-4205): invalidate, re-derive, repopulate, and —
/// when a message exists — emit the `latest_message_updated` notification to
/// subscribers of the conversation. A refresh with no resulting message is
/// completed silently. Returns false only when storage cannot be read after
/// the bounded retries.
pub async fn refresh(state: &AppState, conversation_id: &str) -> bool {
    state.realtime.latest.invalidate(conversation_id);
    let mut derived: Option<Option<Value>> = None;
    for _ in 0..REFRESH_MAX_RETRIES {
        match derive(state, conversation_id).await {
            Ok(v) => {
                derived = Some(v);
                break;
            }
            Err(e) => {
                tracing::warn!(error = %e, conversation_id, "latest-message refresh attempt failed");
            }
        }
    }
    let Some(latest) = derived else {
        // Abandoned after the retry ceiling; the authoritative read path
        // remains available (CRD 4152, 4186).
        return false;
    };
    if let Some(snapshot) = latest {
        state.realtime.latest.store(conversation_id, snapshot.clone());
        // Exact frame shape per CRD 4180-4182; broadcast failure is non-fatal.
        let frame = json!({
            "type": "latest_message_updated",
            "conversationId": conversation_id,
            "data": {
                "content": snapshot["content"],
                "createdAt": snapshot["createdAt"],
                "senderType": snapshot["senderType"],
            },
            "timestamp": crate::db::now_iso(),
        });
        state.realtime.to_conversation_raw(conversation_id, &frame.to_string());
    }
    true
}

/// Coalescing refresh entry point (CRD 4151-4153, 4185): duplicate requests
/// for a conversation whose refresh is already pending are dropped; the
/// refresh itself runs asynchronously and its failure is logged only.
pub fn schedule_refresh(state: std::sync::Arc<AppState>, conversation_id: String) {
    if !state.realtime.latest.begin_refresh(&conversation_id) {
        return; // coalesced into the pending refresh
    }
    tokio::spawn(async move {
        let ok = refresh(&state, &conversation_id).await;
        state.realtime.latest.end_refresh(&conversation_id, ok);
        if !ok {
            tracing::warn!(conversation_id, "latest-message refresh abandoned after retries");
        }
    });
}

/// Warm up the cache for the most recently active conversations (CRD
/// 4160-4163). Returns the count of conversations warmed.
pub async fn warm_up(state: &AppState, limit: Option<i64>) -> usize {
    let limit = limit.filter(|l| *l > 0).unwrap_or(WARMUP_DEFAULT_LIMIT);
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM conversations
         WHERE deleted_at IS NULL
         ORDER BY COALESCE(last_message_at, updated_at, created_at) DESC
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut warmed = 0usize;
    for cid in ids {
        if let Ok(Some(snapshot)) = derive(state, &cid).await {
            state.realtime.latest.store(&cid, snapshot);
            warmed += 1;
        }
    }
    warmed
}

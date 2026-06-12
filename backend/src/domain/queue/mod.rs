//! Background Queue Processing (CRD §6.5, lines 5106-5245).
//!
//! One in-process work queue handles two job kinds: outbound platform message
//! delivery and inbound media retrieval. Jobs are retried with progressive
//! backoff (base 1s, x2, cap 30s, max 3 attempts) and exhausted jobs land in
//! a dead-letter holding area. Authenticated read-only monitoring endpoints
//! expose (fixed, per the behavioral boundary) health/performance figures.

pub mod handlers;
pub mod worker;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

pub const MAX_RETRIES: u32 = 3;
pub const BASE_DELAY_MS: u64 = 1000;
pub const MAX_DELAY_MS: u64 = 30_000;
pub const BACKOFF_MULTIPLIER: u64 = 2;
pub const MAX_BATCH_SIZE: usize = 10;
pub const MAX_BATCH_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Job {
    pub body: Value,
    pub attempt: u32,
}

#[derive(Debug, Default, Clone)]
pub struct QueueStats {
    pub total_processed: u64,
    pub successes: u64,
    pub errors: u64,
    pub retries: u64,
    pub total_time_ms: u128,
    pub last_processed_at: Option<String>,
}

impl QueueStats {
    pub fn average_ms(&self) -> u128 {
        if self.total_processed == 0 {
            0
        } else {
            self.total_time_ms / self.total_processed as u128
        }
    }
}

pub struct JobQueue {
    tx: mpsc::UnboundedSender<Job>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<Job>>>,
    pub stats: Mutex<QueueStats>,
    pub dead_letter: Mutex<Vec<Job>>,
}

impl Default for JobQueue {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
            stats: Mutex::new(QueueStats::default()),
            dead_letter: Mutex::new(Vec::new()),
        }
    }
}

impl JobQueue {
    /// Fire-and-forget outbound acceptance (CRD 5127-5131): never throws.
    pub fn enqueue_outbound(&self, body: Value) -> Value {
        match self.tx.send(Job { body, attempt: 0 }) {
            Ok(()) => serde_json::json!({ "success": true }),
            Err(e) => serde_json::json!({ "success": false, "error": e.to_string() }),
        }
    }

    /// Media enqueue is non-critical: failure is logged and tolerated
    /// (CRD 5145, 5148).
    pub fn enqueue_media(&self, body: Value) {
        if let Err(e) = self.tx.send(Job { body, attempt: 0 }) {
            tracing::warn!(error = %e, "media job enqueue failed (message already stored)");
        }
    }

    pub fn requeue(&self, job: Job) {
        let _ = self.tx.send(job);
    }

    pub(crate) fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<Job>> {
        self.rx.lock().ok().and_then(|mut g| g.take())
    }

    pub fn record(&self, ok: bool, retried: bool, elapsed_ms: u128) {
        if let Ok(mut s) = self.stats.lock() {
            s.total_processed += 1;
            if ok {
                s.successes += 1;
            } else {
                s.errors += 1;
            }
            if retried {
                s.retries += 1;
            }
            s.total_time_ms += elapsed_ms;
            s.last_processed_at = Some(crate::db::now_iso());
        }
    }

    pub fn dead_letter_size(&self) -> usize {
        self.dead_letter.lock().map(|d| d.len()).unwrap_or(0)
    }
}

/// Progressive retry delay (CRD 5190-5192).
pub fn retry_delay_ms(attempt: u32) -> u64 {
    (BASE_DELAY_MS * BACKOFF_MULTIPLIER.pow(attempt)).min(MAX_DELAY_MS)
}

/// Failure-category taxonomy and retryability (CRD 5188-5189).
pub fn categorize(error: &str) -> &'static str {
    let e = error.to_lowercase();
    if e.contains("network") || e.contains("connect") {
        "network"
    } else if e.contains("timeout") || e.contains("timed out") {
        "timeout"
    } else if e.contains("rate limit") || e.contains("429") {
        "rate-limit"
    } else if e.contains("validation") || e.contains("invalid") {
        "validation"
    } else if e.contains("permanent") {
        "permanent-failure"
    } else if e.contains("temporary") {
        "temporary-failure"
    } else {
        "system"
    }
}

pub fn is_retryable(category: &str) -> bool {
    matches!(category, "network" | "timeout" | "rate-limit" | "temporary-failure" | "system")
}

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/queues/stats", get(handlers::stats))
        .route("/api/queues/health", get(handlers::health))
        .route("/api/queues/performance", get(handlers::performance))
        .route("/api/queues/maintenance", post(handlers::maintenance))
        .layer(from_fn_with_state(state, require_auth))
}

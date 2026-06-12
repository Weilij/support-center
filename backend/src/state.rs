use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::Config;
use crate::middleware::rate_limit::RateLimiter;

#[derive(Clone, Debug)]
pub struct TeamMembership {
    pub team_id: i64,
    pub role: String,
    pub is_primary: bool,
}

/// Per-user team-membership cache (~60s) per CRD line 270/501.
#[derive(Default)]
pub struct TeamCache {
    entries: Mutex<HashMap<String, (Instant, Vec<TeamMembership>)>>,
}

impl TeamCache {
    pub fn get(&self, user_id: &str, max_age: std::time::Duration) -> Option<Vec<TeamMembership>> {
        let entries = self.entries.lock().ok()?;
        let (at, teams) = entries.get(user_id)?;
        (at.elapsed() < max_age).then(|| teams.clone())
    }

    pub fn put(&self, user_id: &str, teams: Vec<TeamMembership>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(user_id.to_string(), (Instant::now(), teams));
        }
    }

    pub fn invalidate(&self, user_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(user_id);
        }
    }
}

/// Debounce store so last-active is persisted at most periodically (CRD line 273).
#[derive(Default)]
pub struct LastActiveDebounce {
    entries: Mutex<HashMap<String, Instant>>,
}

impl LastActiveDebounce {
    /// Returns true when the caller should persist last-active now.
    pub fn should_persist(&self, user_id: &str, interval: std::time::Duration) -> bool {
        let Ok(mut entries) = self.entries.lock() else { return false };
        let now = Instant::now();
        match entries.get(user_id) {
            Some(last) if now.duration_since(*last) < interval => false,
            _ => {
                entries.insert(user_id.to_string(), now);
                true
            }
        }
    }
}

/// Transient prior-state capture for a member batch edit (CRD 2000-2009).
/// Tokens are advertised for ~10 seconds and retained server-side ~60 seconds.
pub struct BatchUndoEntry {
    pub user_id: String,
    pub created: Instant,
    pub snapshot: serde_json::Value,
}

pub const BATCH_UNDO_RETENTION: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Default)]
pub struct BatchUndoStore {
    entries: Mutex<HashMap<String, BatchUndoEntry>>,
}

impl BatchUndoStore {
    pub fn put(&self, token: &str, user_id: &str, snapshot: serde_json::Value) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, e| e.created.elapsed() < BATCH_UNDO_RETENTION);
            entries.insert(
                token.to_string(),
                BatchUndoEntry { user_id: user_id.to_string(), created: Instant::now(), snapshot },
            );
        }
    }

    /// Removes and returns the entry when it exists and is unexpired.
    pub fn take(&self, token: &str) -> Option<(String, serde_json::Value)> {
        let mut entries = self.entries.lock().ok()?;
        let entry = entries.get(token)?;
        if entry.created.elapsed() >= BATCH_UNDO_RETENTION {
            entries.remove(token);
            return None;
        }
        // Ownership is checked by the caller; only remove on consumption.
        let e = entries.remove(token)?;
        Some((e.user_id, e.snapshot))
    }

    /// Re-insert an entry (used when an undo attempt is rejected for ownership).
    pub fn restore(&self, token: &str, user_id: String, snapshot: serde_json::Value) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                token.to_string(),
                BatchUndoEntry { user_id, created: Instant::now(), snapshot },
            );
        }
    }
}

/// Fast-lookup recallability markers for pending delayed messages (CRD 987,
/// 1014): present while an item is still cancellable, expiring shortly after
/// its scheduled send time.
#[derive(Default)]
pub struct RecallableMarkers {
    entries: Mutex<HashMap<String, Instant>>, // value = expiry instant
}

impl RecallableMarkers {
    pub fn mark(&self, id: &str, ttl: std::time::Duration) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, expiry| *expiry > Instant::now());
            entries.insert(id.to_string(), Instant::now() + ttl);
        }
    }

    pub fn is_recallable(&self, id: &str) -> bool {
        let Ok(mut entries) = self.entries.lock() else { return false };
        match entries.get(id) {
            Some(expiry) if *expiry > Instant::now() => true,
            Some(_) => {
                entries.remove(id);
                false
            }
            None => false,
        }
    }

    pub fn clear(&self, id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(id);
        }
    }
}

pub struct AppState {
    pub db: SqlitePool,
    pub config: Config,
    pub rate_limiter: Arc<RateLimiter>,
    pub team_cache: TeamCache,
    pub last_active: LastActiveDebounce,
    pub batch_undo: BatchUndoStore,
    pub auto_reply_cache: crate::domain::auto_reply::engine::RuleCache,
    pub recallable_messages: RecallableMarkers,
    /// Central realtime hub (CRD §5.1): connection registry, rooms, channels
    /// and the broadcast API used by the domain modules.
    pub realtime: Arc<crate::realtime::RealtimeHub>,
}

impl AppState {
    pub fn new(db: SqlitePool, config: Config) -> Arc<Self> {
        Arc::new(Self {
            db,
            config,
            rate_limiter: Arc::new(RateLimiter::default()),
            team_cache: TeamCache::default(),
            last_active: LastActiveDebounce::default(),
            batch_undo: BatchUndoStore::default(),
            auto_reply_cache: crate::domain::auto_reply::engine::RuleCache::default(),
            recallable_messages: RecallableMarkers::default(),
            realtime: Arc::new(crate::realtime::RealtimeHub::new()),
        })
    }
}

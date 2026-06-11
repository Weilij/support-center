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

pub struct AppState {
    pub db: SqlitePool,
    pub config: Config,
    pub rate_limiter: Arc<RateLimiter>,
    pub team_cache: TeamCache,
    pub last_active: LastActiveDebounce,
}

impl AppState {
    pub fn new(db: SqlitePool, config: Config) -> Arc<Self> {
        Arc::new(Self {
            db,
            config,
            rate_limiter: Arc::new(RateLimiter::default()),
            team_cache: TeamCache::default(),
            last_active: LastActiveDebounce::default(),
        })
    }
}

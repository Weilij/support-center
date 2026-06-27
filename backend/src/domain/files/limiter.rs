//! Per-user in-process upload rate limiting (CRD 3196): caps on concurrent
//! uploads, uploads per minute/hour, and bytes per hour. Exceeding any cap
//! yields 429 with a descriptive message.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Copy)]
pub struct UploadPolicy {
    pub max_concurrent: u32,
    pub per_minute: u32,
    pub per_hour: u32,
    pub bytes_per_hour: u64,
}

pub const STANDARD_UPLOADS: UploadPolicy = UploadPolicy {
    max_concurrent: 3,
    per_minute: 20,
    per_hour: 100,
    bytes_per_hour: 100 * 1024 * 1024,
};
pub const ADMIN_UPLOADS: UploadPolicy = UploadPolicy {
    max_concurrent: 10,
    per_minute: 100,
    per_hour: 1000,
    bytes_per_hour: 1024 * 1024 * 1024,
};

#[derive(Default)]
struct UserCounters {
    concurrent: u32,
    events: Vec<(Instant, u64)>, // (when, bytes)
}

#[derive(Default)]
pub struct UploadLimiter {
    users: Mutex<HashMap<String, UserCounters>>,
}

/// RAII guard releasing the concurrency slot when the request completes.
pub struct ConcurrencyGuard<'a> {
    limiter: &'a UploadLimiter,
    user: String,
}

impl Drop for ConcurrencyGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut users) = self.limiter.users.lock() {
            if let Some(c) = users.get_mut(&self.user) {
                c.concurrent = c.concurrent.saturating_sub(1);
            }
        }
    }
}

impl UploadLimiter {
    /// Admit one upload of `bytes` for `user`; returns a guard on success or
    /// a descriptive refusal.
    pub fn admit<'a>(
        &'a self,
        user: &str,
        bytes: u64,
        policy: &UploadPolicy,
    ) -> Result<ConcurrencyGuard<'a>, String> {
        let mut users = self
            .users
            .lock()
            .map_err(|_| "limiter unavailable".to_string())?;
        let counters = users.entry(user.to_string()).or_default();
        let now = Instant::now();
        counters
            .events
            .retain(|(t, _)| now.duration_since(*t).as_secs() < 3600);

        if counters.concurrent >= policy.max_concurrent {
            return Err(format!(
                "Too many concurrent uploads (max {})",
                policy.max_concurrent
            ));
        }
        let last_minute = counters
            .events
            .iter()
            .filter(|(t, _)| now.duration_since(*t).as_secs() < 60)
            .count() as u32;
        if last_minute >= policy.per_minute {
            return Err(format!(
                "Upload rate exceeded ({} per minute)",
                policy.per_minute
            ));
        }
        if counters.events.len() as u32 >= policy.per_hour {
            return Err(format!(
                "Upload rate exceeded ({} per hour)",
                policy.per_hour
            ));
        }
        let hour_bytes: u64 = counters.events.iter().map(|(_, b)| *b).sum();
        if hour_bytes + bytes > policy.bytes_per_hour {
            return Err("Hourly upload byte budget exceeded".into());
        }

        counters.concurrent += 1;
        counters.events.push((now, bytes));
        Ok(ConcurrencyGuard {
            limiter: self,
            user: user.to_string(),
        })
    }
}

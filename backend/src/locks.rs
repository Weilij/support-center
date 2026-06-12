//! Mutual-exclusion guarantees (CRD §6.7, lines 5537-5551): keyed, leased
//! exclusivity over named resources.
//!
//! 1. Single-holder: at most one context mutates a keyed resource at a time.
//! 2. Lease & expiration: exclusivity is time-bounded; a lapsed lease is
//!    reclaimable, so permanent deadlock is impossible.
//! 3. Safe-release: only the owning guard releases early (RAII Drop); other
//!    actors can only reclaim after expiry.
//! 4. Bounded contention: contenders wait up to a bounded time, then report
//!    failure instead of proceeding without exclusivity.
//! 5. Guaranteed relinquish: the guard releases on every outcome (Drop runs
//!    on success, error, and panic unwinding alike).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct KeyedLeaseLock {
    // key -> (owner token, lease expiry)
    holders: Mutex<HashMap<String, (u64, Instant)>>,
    counter: std::sync::atomic::AtomicU64,
}

pub struct LeaseGuard {
    locks: Arc<KeyedLeaseLock>,
    key: String,
    token: u64,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if let Ok(mut holders) = self.locks.holders.lock() {
            // Safe-release: only remove when we are still the owner.
            if holders.get(&self.key).map(|(t, _)| *t) == Some(self.token) {
                holders.remove(&self.key);
            }
        }
    }
}

impl KeyedLeaseLock {
    /// Try to obtain exclusivity over `key` for at most `lease`, waiting up
    /// to `max_wait` for the current holder. Returns None when the bounded
    /// wait elapses (the caller must abandon the attempt, CRD 5547).
    pub async fn acquire(
        self: &Arc<Self>,
        key: &str,
        lease: Duration,
        max_wait: Duration,
    ) -> Option<LeaseGuard> {
        let deadline = Instant::now() + max_wait;
        loop {
            {
                let mut holders = self.holders.lock().ok()?;
                let now = Instant::now();
                let free = match holders.get(key) {
                    None => true,
                    // Lapsed lease: reclaimable (CRD 5543).
                    Some((_, expiry)) => *expiry <= now,
                };
                if free {
                    let token = self
                        .counter
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    holders.insert(key.to_string(), (token, now + lease));
                    return Some(LeaseGuard {
                        locks: self.clone(),
                        key: key.to_string(),
                        token,
                    });
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn single_holder_and_bounded_contention() {
        let locks = Arc::new(KeyedLeaseLock::default());
        let guard = locks
            .acquire("customer:1", Duration::from_secs(5), Duration::from_millis(10))
            .await
            .expect("first acquire succeeds");
        // A contender for the same key times out (guarantee 4)...
        let contender = locks
            .acquire("customer:1", Duration::from_secs(5), Duration::from_millis(50))
            .await;
        assert!(contender.is_none(), "bounded wait elapsed -> failure, never parallel");
        // ...while a different key is unaffected (per-resource isolation).
        let other = locks
            .acquire("customer:2", Duration::from_secs(5), Duration::from_millis(10))
            .await;
        assert!(other.is_some());
        drop(guard);
        // Released by the owner: immediately reacquirable (guarantee 3/5).
        let again = locks
            .acquire("customer:1", Duration::from_secs(5), Duration::from_millis(10))
            .await;
        assert!(again.is_some());
    }

    #[tokio::test]
    async fn lapsed_lease_is_reclaimed() {
        let locks = Arc::new(KeyedLeaseLock::default());
        let guard = locks
            .acquire("conv:9", Duration::from_millis(30), Duration::from_millis(10))
            .await
            .unwrap();
        // Simulate a crashed holder: never dropped, but the lease lapses.
        std::mem::forget(guard);
        let reclaimed = locks
            .acquire("conv:9", Duration::from_secs(5), Duration::from_millis(500))
            .await;
        assert!(reclaimed.is_some(), "no permanent deadlock (guarantee 2)");
    }

    #[tokio::test]
    async fn stale_owner_cannot_release_a_reclaimed_lock() {
        let locks = Arc::new(KeyedLeaseLock::default());
        let stale = locks
            .acquire("k", Duration::from_millis(20), Duration::from_millis(10))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        let new_owner = locks
            .acquire("k", Duration::from_secs(5), Duration::from_millis(10))
            .await
            .unwrap();
        // The stale guard drops AFTER reclamation: it must not free the
        // new owner's exclusivity (no cross-owner release, guarantee 3).
        drop(stale);
        let contender = locks
            .acquire("k", Duration::from_secs(5), Duration::from_millis(50))
            .await;
        assert!(contender.is_none(), "new owner still holds the key");
        drop(new_owner);
    }
}

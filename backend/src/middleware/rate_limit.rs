//! Sliding-window per-IP rate limiting per CRD §7.1 (lines 5620-5626).

use axum::body::Body;
use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};


#[derive(Clone, Copy, Debug)]
pub struct RatePolicy {
    pub scope: &'static str,
    pub max_requests: u32,
    pub window: Duration,
}

impl RatePolicy {
    pub const STANDARD: Self = Self::new("standard", 100, 60);
    pub const AUTH: Self = Self::new("auth", 10, 60);
    pub const LOGIN: Self = Self::new("login", 5, 300);
    pub const UPLOAD: Self = Self::new("upload", 20, 60);
    pub const WEBSOCKET: Self = Self::new("websocket", 30, 60);
    pub const ADMIN: Self = Self::new("admin", 200, 60);
    pub const HIGH_FREQUENCY: Self = Self::new("high-frequency", 500, 60);

    const fn new(scope: &'static str, max_requests: u32, window_secs: u64) -> Self {
        Self { scope, max_requests, window: Duration::from_secs(window_secs) }
    }
}

pub struct RateLimiter {
    buckets: Mutex<HashMap<(String, String), Vec<Instant>>>,
    total_checks: std::sync::atomic::AtomicU64,
    total_blocked: std::sync::atomic::AtomicU64,
    started: Instant,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            total_checks: std::sync::atomic::AtomicU64::new(0),
            total_blocked: std::sync::atomic::AtomicU64::new(0),
            started: Instant::now(),
        }
    }
}

pub struct RateDecision {
    pub allowed: bool,
    pub limit: u32,
    pub remaining: u32,
    pub reset_secs: u64,
}

impl RateLimiter {
    /// Operational statistics view (CRD 5531): distinct callers, cumulative
    /// checks/blocks, tracked entries, last checkpoint, uptime.
    pub fn stats(&self) -> serde_json::Value {
        use std::sync::atomic::Ordering;
        let (callers, entries) = self
            .buckets
            .lock()
            .map(|b| {
                let callers: std::collections::HashSet<&String> = b.keys().map(|(_, c)| c).collect();
                (callers.len(), b.len())
            })
            .unwrap_or((0, 0));
        serde_json::json!({
            "distinctCallers": callers,
            "totalChecks": self.total_checks.load(Ordering::Relaxed),
            "totalBlocked": self.total_blocked.load(Ordering::Relaxed),
            "trackedEntries": entries,
            "lastPersistedAt": null,
            "uptimeSecs": self.started.elapsed().as_secs(),
        })
    }

    /// Prune idle counters (operational facility, CRD 5533).
    pub fn prune(&self, max_window: Duration) {
        if let Ok(mut buckets) = self.buckets.lock() {
            let now = Instant::now();
            buckets.retain(|_, hits| {
                hits.retain(|t| now.duration_since(*t) < max_window);
                !hits.is_empty()
            });
        }
    }

    /// Consume one request from the caller's sliding window; never panics (fail-open).
    pub fn check(&self, policy: &RatePolicy, caller: &str) -> RateDecision {
        use std::sync::atomic::Ordering;
        self.total_checks.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        let mut buckets = match self.buckets.lock() {
            Ok(g) => g,
            Err(_) => {
                return RateDecision { allowed: true, limit: policy.max_requests, remaining: policy.max_requests, reset_secs: policy.window.as_secs() }
            }
        };
        let key = (policy.scope.to_string(), caller.to_string());
        let hits = buckets.entry(key).or_default();
        hits.retain(|t| now.duration_since(*t) < policy.window);
        let reset_secs = hits
            .first()
            .map(|t| policy.window.saturating_sub(now.duration_since(*t)).as_secs() + 1)
            .unwrap_or(policy.window.as_secs());
        if hits.len() as u32 >= policy.max_requests {
            self.total_blocked.fetch_add(1, Ordering::Relaxed);
            return RateDecision { allowed: false, limit: policy.max_requests, remaining: 0, reset_secs };
        }
        hits.push(now);
        let remaining = policy.max_requests - hits.len() as u32;
        RateDecision { allowed: true, limit: policy.max_requests, remaining, reset_secs }
    }
}

/// Caller identity per CRD 5621: CF-Connecting-IP, then X-Forwarded-For, then X-Real-IP,
/// falling back to a shared "unknown" bucket.
pub fn caller_ip(req: &Request<Body>) -> String {
    for h in ["cf-connecting-ip", "x-forwarded-for", "x-real-ip"] {
        if let Some(v) = req.headers().get(h).and_then(|v| v.to_str().ok()) {
            let first = v.split(',').next().unwrap_or(v).trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "unknown".to_string()
}

fn decorate(resp: &mut Response, d: &RateDecision) {
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&d.limit.to_string()) {
        h.insert("X-RateLimit-Limit", v);
    }
    if let Ok(v) = HeaderValue::from_str(&d.remaining.to_string()) {
        h.insert("X-RateLimit-Remaining", v);
    }
    if let Ok(v) = HeaderValue::from_str(&d.reset_secs.to_string()) {
        h.insert("X-RateLimit-Reset", v);
    }
}

/// Build an axum middleware fn for the given policy, sharing one limiter.
pub fn limit(
    limiter: std::sync::Arc<RateLimiter>,
    policy: RatePolicy,
) -> impl Fn(Request<Body>, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send
       + 'static {
    move |req: Request<Body>, next: Next| {
        let limiter = limiter.clone();
        Box::pin(async move {
            let ip = caller_ip(&req);
            let path = req.uri().path().to_string();
            let decision = limiter.check(&policy, &ip);
            if !decision.allowed {
                let retry_after = decision.reset_secs.max(1);
                // Auth/login blocks emit security warnings (CRD 5523, 5574).
                if policy.scope == "login" {
                    tracing::warn!(caller = %ip, path = %path, retry_after,
                        "login rate limit block (possible brute-force attempt)");
                } else if policy.scope == "auth" {
                    tracing::warn!(caller = %ip, path = %path, retry_after, "auth rate limit block");
                }
                // Documented throttled body (CRD 5530): error label, wait
                // message, limit, window in seconds, retry-after seconds.
                let body = serde_json::json!({
                    "success": false,
                    "error": "Rate limit exceeded",
                    "code": "TOO_MANY_REQUESTS",
                    "message": format!("Too many requests. Please retry after {retry_after} seconds"),
                    "limit": decision.limit,
                    "window": policy.window.as_secs().to_string(),
                    "retryAfter": retry_after,
                    "timestamp": crate::db::now_iso(),
                });
                let mut resp = (axum::http::StatusCode::TOO_MANY_REQUESTS, axum::Json(body))
                    .into_response();
                if let Ok(v) = retry_after.to_string().parse() {
                    resp.headers_mut().insert("Retry-After", v);
                }
                decorate(&mut resp, &decision);
                return resp;
            }
            let mut resp = next.run(req).await;
            decorate(&mut resp, &decision);
            resp
        })
    }
}

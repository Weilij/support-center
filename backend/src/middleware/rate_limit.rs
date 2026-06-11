//! Sliding-window per-IP rate limiting per CRD §7.1 (lines 5620-5626).

use axum::body::Body;
use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::AppError;

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

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<(String, String), Vec<Instant>>>,
}

pub struct RateDecision {
    pub allowed: bool,
    pub limit: u32,
    pub remaining: u32,
    pub reset_secs: u64,
}

impl RateLimiter {
    /// Consume one request from the caller's sliding window; never panics (fail-open).
    pub fn check(&self, policy: &RatePolicy, caller: &str) -> RateDecision {
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
            let decision = limiter.check(&policy, &ip);
            if !decision.allowed {
                let mut resp = AppError::TooManyRequests {
                    message: format!(
                        "Too many requests. Please retry after {} seconds",
                        decision.reset_secs
                    ),
                    retry_after: decision.reset_secs,
                }
                .into_response();
                decorate(&mut resp, &decision);
                return resp;
            }
            let mut resp = next.run(req).await;
            decorate(&mut resp, &decision);
            resp
        })
    }
}

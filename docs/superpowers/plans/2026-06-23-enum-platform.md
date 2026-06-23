# `enum Platform` Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a canonical `Platform` enum and unify the drifted auto_reply + delayed_messages allow-lists so Instagram and Shopee are accepted.

**Architecture:** A new `src/platform.rs` (Line/Facebook/Instagram/Shopee + `from_str`/`as_str`/`ALL`), registered in `lib.rs`; then replace the two leaf handlers' string allow-lists with `Platform::from_str(...).is_none()` guards. The gateway hub (`send_batch`) is deliberately untouched.

**Tech Stack:** Rust, axum, sqlx.

**Spec:** `docs/superpowers/specs/2026-06-23-enum-platform-design.md`

---

## File Structure
- `backend/src/platform.rs` — **create**: the enum.
- `backend/src/lib.rs` — **modify**: `pub mod platform;`.
- `backend/src/domain/auto_reply/handlers.rs` — **modify**: drop `PLATFORMS`, use the enum.
- `backend/src/domain/delayed_messages/handlers.rs` — **modify**: 2 allow-list sites.
- Tests: `backend/tests/auto_reply.rs`, `backend/tests/delayed_messages.rs`.

---

## Task 1: The `Platform` enum (pure, TDD)

**Files:**
- Create: `backend/src/platform.rs`
- Modify: `backend/src/lib.rs`

- [ ] **Step 1: Register the module + write the failing tests**

In `backend/src/lib.rs`, add `pub mod platform;` (alphabetically, between `pub mod middleware;` and `pub mod realtime;`).
Create `backend/src/platform.rs` with ONLY the test module first:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_four() {
        for p in Platform::ALL {
            assert_eq!(Platform::from_str(p.as_str()), Some(p));
        }
        assert_eq!(Platform::ALL.len(), 4);
    }

    #[test]
    fn as_str_values() {
        assert_eq!(Platform::Line.as_str(), "line");
        assert_eq!(Platform::Facebook.as_str(), "facebook");
        assert_eq!(Platform::Instagram.as_str(), "instagram");
        assert_eq!(Platform::Shopee.as_str(), "shopee");
    }

    #[test]
    fn unknown_and_whatsapp_are_none() {
        assert_eq!(Platform::from_str("whatsapp"), None);
        assert_eq!(Platform::from_str(""), None);
        assert_eq!(Platform::from_str("LINE"), None); // case-sensitive, canonical only
    }
}
```
Run `cd backend && cargo test --lib platform 2>&1 | tail -15` → FAIL to compile (`Platform` missing).

- [ ] **Step 2: Implement the enum**

Prepend to `backend/src/platform.rs`:
```rust
//! Canonical messaging platforms (single source of truth). The DB and API use
//! the string form; parse at the boundary with `Platform::from_str`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Line,
    Facebook,
    Instagram,
    Shopee,
}

impl Platform {
    pub const ALL: [Platform; 4] = [
        Platform::Line,
        Platform::Facebook,
        Platform::Instagram,
        Platform::Shopee,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Line => "line",
            Platform::Facebook => "facebook",
            Platform::Instagram => "instagram",
            Platform::Shopee => "shopee",
        }
    }

    pub fn from_str(s: &str) -> Option<Platform> {
        match s {
            "line" => Some(Platform::Line),
            "facebook" => Some(Platform::Facebook),
            "instagram" => Some(Platform::Instagram),
            "shopee" => Some(Platform::Shopee),
            _ => None,
        }
    }
}
```
Run `cd backend && cargo test --lib platform 2>&1 | tail -10` → 3 passing. `cargo build 2>&1 | tail -3` → success.

- [ ] **Step 3: Commit**

```bash
git add backend/src/lib.rs backend/src/platform.rs
git commit -m "feat(platform): canonical Platform enum (line/facebook/instagram/shopee)"
```

---

## Task 2: Unify the drifted allow-lists

**Files:**
- Modify: `backend/src/domain/auto_reply/handlers.rs` (`~line 29` const + `~line 508` check)
- Modify: `backend/src/domain/delayed_messages/handlers.rs` (`~line 461`, `~line 690`)
- Test: `backend/tests/auto_reply.rs`, `backend/tests/delayed_messages.rs`

- [ ] **Step 1: auto_reply — replace the allow-list**

In `backend/src/domain/auto_reply/handlers.rs`:
- DELETE the line `const PLATFORMS: &[&str] = &["line", "facebook", "whatsapp"];` (~line 29).
- At the platform check (~line 508), replace:
```rust
    if let Some(p) = &q.platform {
        if !PLATFORMS.contains(&p.as_str()) {
            return Err(AppError::BadRequest(format!(
                "Invalid platform '{p}': must be one of {PLATFORMS:?}"
            )));
        }
    }
```
with:
```rust
    if let Some(p) = &q.platform {
        if crate::platform::Platform::from_str(p).is_none() {
            return Err(AppError::BadRequest(format!(
                "Invalid platform '{p}': must be one of line, facebook, instagram, shopee"
            )));
        }
    }
```

- [ ] **Step 2: delayed_messages — replace both allow-lists**

In `backend/src/domain/delayed_messages/handlers.rs`:
- At ~line 461 (where `platform` is a `&str`), replace:
```rust
    if !["line", "facebook"].contains(&platform) {
        problems.push("platform must be one of: line, facebook");
    }
```
with:
```rust
    if crate::platform::Platform::from_str(platform).is_none() {
        problems.push("platform must be one of: line, facebook, instagram, shopee");
    }
```
- At ~line 690 (where `platform` is a `String`), replace:
```rust
    if !["line", "facebook"].contains(&platform.as_str()) {
        return Err(AppError::BadRequest(
            "Message platform does not support rescheduling".into(),
        ));
    }
```
with:
```rust
    if crate::platform::Platform::from_str(&platform).is_none() {
        return Err(AppError::BadRequest(
            "Message platform does not support rescheduling".into(),
        ));
    }
```
Run `cd backend && cargo build 2>&1 | tail -5` → success.

- [ ] **Step 3: Integration tests for the widened behavior**

Study the existing `backend/tests/auto_reply.rs` and `backend/tests/delayed_messages.rs` to find the endpoint + request helpers (the auto_reply platform check is on the logs/stats list query `?platform=`; the delayed_messages checks are on create + reschedule).

**auto_reply** (`tests/auto_reply.rs`): add a test that the logs/list endpoint accepts `?platform=instagram` and `?platform=shopee` (status is NOT `BAD_REQUEST`), and that `?platform=whatsapp` now IS `BAD_REQUEST`. Use the same request helper/endpoint the existing platform-filter test uses (search the file for `platform=` or the PLATFORMS-related test).

**delayed_messages** (`tests/delayed_messages.rs`): add/extend a test that scheduling a delayed message with `platform: "instagram"` (and `"shopee"`) is **accepted** (previously rejected with "platform must be one of: line, facebook"), reusing the existing create-delayed-message test's request shape. Keep an assertion that an unknown platform (e.g. `"telegram"`) is still rejected.

If a test currently asserts that `instagram`/`shopee` are rejected, update it to the new accepted behavior; if a test asserts `whatsapp` is accepted by auto_reply, update it to rejected (intended change).

- [ ] **Step 4: Build + suites**

- `cd backend && cargo build 2>&1 | tail -5` → success.
- `cd backend && cargo build --tests 2>&1 | tail -5` → success.
- `cd backend && cargo test --lib platform 2>&1 | grep "test result"` → green.
- `cd backend && cargo test --test auto_reply --test delayed_messages 2>&1 | grep -E "Running|test result|error\[|FAILED"` → green (incl. the new widened-behavior assertions).
- `cd backend && cargo test 2>&1 | grep -E "test result|error\[" | tail -30` → all suites green.

- [ ] **Step 5: Commit**

```bash
git add backend/src/domain/auto_reply/handlers.rs backend/src/domain/delayed_messages/handlers.rs backend/tests/auto_reply.rs backend/tests/delayed_messages.rs
git commit -m "refactor(platform): unify auto_reply/delayed allow-lists via Platform (accept IG/Shopee)"
```

---

## Final verification (after all tasks)

- [ ] `cd backend && cargo build` + `cargo build --tests` — clean
- [ ] `cd backend && grep -rn 'PLATFORMS' src/domain/auto_reply` — no matches (const removed)
- [ ] `cd backend && cargo test` — all suites green (platform unit + auto_reply/delayed widened-behavior + existing)
- [ ] `detect_changes()` before the final commit — expect LOW (new file + two leaf handlers; the gateway `send_batch` is untouched).
```

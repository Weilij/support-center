# `enum Platform` Consolidation — Design Spec

**Date:** 2026-06-23
**Track:** maintainability refactor (backend)
**Status:** design approved, pending written-spec review

---

## 0. Context

Messaging platforms are stringly-typed and scattered across ~17 backend files. The allow-lists have already **drifted**: `auto_reply` accepts `["line","facebook","whatsapp"]` and `delayed_messages` accepts `["line","facebook"]` — so the Instagram and Shopee integrations merged in B2/B3/B4 silently cannot use auto-reply rules or delayed/scheduled messages. This refactor introduces one canonical `Platform` enum and unifies the drifted allow-lists.

**GitNexus impact (pre-design):** `send_batch` (the outbound gateway) is **CRITICAL** (7 direct callers, 6 processes, 10 modules) — it is a hub, so we deliberately **do not** touch it here (converting its internal match would be high-blast-radius churn for no behavior gain). The edits in this spec are confined to a new file + two leaf route handlers (LOW blast radius).

---

## 1. Goal & non-goals

**Goal:** a single `Platform` enum as the source of truth for the integrated messaging platforms, and unified allow-lists so all of them are accepted where one currently drifts.

**Non-goals:**
- **No `Whatsapp` variant** (deliberately excluded — no backend integration). Consequence: `auto_reply` will **no longer accept `"whatsapp"`** (it currently does); this is an intended, accepted change.
- **No gateway change** — `conversations/channels.rs::send_batch` is untouched (CRITICAL hub; already handles every platform correctly).
- DB/API stay `TEXT`/strings — parse at the boundary, no `sqlx::Type` migration, no field/signature typing (that was option C).
- The **files upload-context** strings (`system`/`admin` in `files/{handlers,validate}.rs`) are a different concept — untouched.
- `webhooks/handlers.rs` (`== "instagram"`) and `messaging/service.rs` (`match platform`) string-compares are not drift sources — left as-is.

---

## 2. The enum (new `src/platform.rs`)

```rust
//! Canonical messaging platforms (single source of truth). DB/API use the
//! string form; parse at the boundary with `from_str`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Line,
    Facebook,
    Instagram,
    Shopee,
}

impl Platform {
    pub const ALL: [Platform; 4] = [Platform::Line, Platform::Facebook, Platform::Instagram, Platform::Shopee];

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
Register `pub mod platform;` in `src/lib.rs`. Unit-tested: `from_str`/`as_str` round-trip for all four; `from_str` returns `None` for `"whatsapp"`/`""`/unknown; `ALL` contains exactly the four.

---

## 3. Unify the drifted allow-lists (§2 — the behavior fix)

- **`auto_reply/handlers.rs`:** remove `const PLATFORMS: &[&str] = ["line","facebook","whatsapp"]`; change the membership check (`~line 508`) to `if crate::platform::Platform::from_str(&p).is_none() { …reject… }`. Net: accepts line/facebook/**instagram**/**shopee**; **drops** whatsapp (intended).
- **`delayed_messages/handlers.rs`** (2 sites, `~461` and `~690`): replace `["line","facebook"].contains(&platform[.as_str()])` with `crate::platform::Platform::from_str(<the platform &str>).is_none()` guards. Net: accepts line/facebook/**instagram**/**shopee**.
- Adjust the existing error messages if they enumerate platforms (so they don't claim only line/facebook). Keep the same HTTP error type/status.

---

## 4. Files & verification

- **Add:** `src/platform.rs`; register in `src/lib.rs`.
- **Modify:** `src/domain/auto_reply/handlers.rs`, `src/domain/delayed_messages/handlers.rs`.
- **Tests:**
  - unit (`src/platform.rs`): round-trip, unknown/whatsapp → None, `ALL`.
  - integration: an auto-reply rule and a delayed message for `instagram` / `shopee` are now **accepted** (previously rejected); a `"whatsapp"` auto-reply is now **rejected**; an unknown platform still rejected. Extend the existing `tests/auto_reply.rs` / `tests/delayed_messages.rs`.
- `cargo build` + `cargo build --tests` + `cargo test` green.
- `detect_changes()` before commit (expect LOW — new file + two leaf handlers).

---

## 5. Resolved decisions
- Variants: **Line, Facebook, Instagram, Shopee** (no Whatsapp).
- §2 **deliberately widens** auto_reply + delayed_messages to accept Instagram + Shopee (the drift fix); auto_reply drops `whatsapp` as a consequence of excluding it.
- **Gateway `send_batch` untouched** (CRITICAL hub per GitNexus; no §3).
- DB/API/upload-context/other string-compares out of scope.
- Two stages (enum, then apply+tests), each subagent-built, reviewed, committed.

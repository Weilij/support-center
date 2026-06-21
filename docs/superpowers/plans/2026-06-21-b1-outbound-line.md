# Outbound Foundation + Real LINE (Track B1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the stubbed outbound seam with a real `OutboundGateway` that delivers text messages to LINE via the Messaging API when a token is configured, falling back to a stub (so tests stay network-free).

**Architecture:** Add `reqwest`; introduce `enum OutboundGateway { Stub, Line(LineGateway) }` with an `async fn send_batch` and a `from_config` factory in `conversations/channels.rs`; migrate the 6 call sites from `StubGateway` to the enum and remove the old `ChannelGateway`/`StubGateway`.

**Tech Stack:** Rust, axum, tokio, reqwest (rustls), serde_json.

**Spec:** `docs/superpowers/specs/2026-06-21-b1-outbound-line-design.md`

---

## File Structure

- `backend/Cargo.toml` — add `reqwest`.
- `backend/src/domain/conversations/channels.rs` — the `OutboundGateway` enum, `LineGateway`, `build_push_body`, `from_config`, shared client, unit tests; later removes `ChannelGateway`/`StubGateway`; `deliver_pending` takes the gateway.
- 5 call-site files — swap `StubGateway` → `OutboundGateway::from_config(&…config)` + `.await`.

---

## Task 1: reqwest + OutboundGateway (additive, TDD)

**Files:**
- Modify: `backend/Cargo.toml`
- Modify: `backend/src/domain/conversations/channels.rs`

Leave the existing `ChannelGateway`/`StubGateway` in place for this task (call sites still use them; build stays green). The new enum is added alongside and unit-tested.

- [ ] **Step 1: Add reqwest to Cargo.toml**

In `backend/Cargo.toml`, under `[dependencies]`, add:
```toml
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
```

- [ ] **Step 2: Write the failing unit tests**

Append to `backend/src/domain/conversations/channels.rs` a test module:
```rust
#[cfg(test)]
mod gateway_tests {
    use super::*;

    #[test]
    fn push_body_has_to_and_text_messages() {
        let items = vec![
            OutboundItem { content: "hi".into() },
            OutboundItem { content: "bye".into() },
        ];
        let b = build_push_body("U123", &items);
        assert_eq!(b["to"], "U123");
        assert_eq!(b["messages"][0]["type"], "text");
        assert_eq!(b["messages"][0]["text"], "hi");
        assert_eq!(b["messages"][1]["text"], "bye");
        assert_eq!(b["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn from_config_picks_stub_without_token_and_line_with() {
        let mut c = crate::config::test_config();
        c.line_channel_access_token = None;
        assert!(matches!(OutboundGateway::from_config(&c), OutboundGateway::Stub));
        c.line_channel_access_token = Some("tok".into());
        assert!(matches!(OutboundGateway::from_config(&c), OutboundGateway::Line(_)));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd backend && cargo test --lib gateway_tests 2>&1 | tail -20`
Expected: FAIL to COMPILE — `build_push_body`, `OutboundGateway`, `from_config` not found.

- [ ] **Step 4: Implement the enum + gateway + pure body builder**

In `backend/src/domain/conversations/channels.rs`, add near the top (after the existing `use` lines) and after the `BATCH_CAP` constant:
```rust
use serde_json::json;
use std::sync::OnceLock;

/// Shared HTTP client (connection pooling) for all outbound platform calls.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Real LINE Messaging API gateway (global channel access token).
pub struct LineGateway {
    token: String,
}

impl LineGateway {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

/// The outbound message body for a LINE push (pure — unit-tested).
pub fn build_push_body(recipient: &str, items: &[OutboundItem]) -> serde_json::Value {
    json!({
        "to": recipient,
        "messages": items
            .iter()
            .map(|it| json!({ "type": "text", "text": it.content }))
            .collect::<Vec<_>>(),
    })
}

/// Outbound delivery gateway. `Stub` reproduces the documented observable
/// outcome without any network call (dev/tests); `Line` calls the real API.
pub enum OutboundGateway {
    Stub,
    Line(LineGateway),
}

impl OutboundGateway {
    /// Real LINE gateway when the global token is configured; otherwise the stub
    /// (so dev/test runs without a token make no network calls).
    pub fn from_config(config: &crate::config::Config) -> Self {
        match config.line_channel_access_token.as_deref() {
            Some(t) if !t.is_empty() => OutboundGateway::Line(LineGateway::new(t.to_string())),
            _ => OutboundGateway::Stub,
        }
    }

    /// Push one batch (≤ BATCH_CAP items) to the platform; returns the
    /// platform-side message id on success.
    pub async fn send_batch(
        &self,
        platform: &str,
        recipient: &str,
        items: &[OutboundItem],
    ) -> Result<String, String> {
        match self {
            OutboundGateway::Stub => match platform {
                "line" => Ok(format!("stub-line-{}", uuid::Uuid::new_v4())),
                other => Err(format!("Outbound delivery is not supported for platform '{other}'")),
            },
            OutboundGateway::Line(g) => {
                if platform != "line" {
                    return Err(format!("Outbound delivery is not supported for platform '{platform}'"));
                }
                let body = build_push_body(recipient, items);
                let resp = http_client()
                    .post("https://api.line.me/v2/bot/message/push")
                    .bearer_auth(&g.token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| format!("LINE request failed: {e}"))?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let txt = resp.text().await.unwrap_or_default();
                    return Err(format!("LINE push failed ({status}): {txt}"));
                }
                let v: serde_json::Value = resp.json().await.unwrap_or_else(|_| json!({}));
                let id = v["sentMessages"][0]["id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("line-{}", uuid::Uuid::new_v4()));
                Ok(id)
            }
        }
    }
}
```
(If `use serde_json::json;` or `uuid` is already imported at the top of the file, don't duplicate the import — reuse it.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd backend && cargo test --lib gateway_tests 2>&1 | tail -10`
Expected: PASS — 2 tests. (First build compiles reqwest; allow time.)

- [ ] **Step 6: Confirm the whole crate still builds**

Run: `cd backend && cargo build 2>&1 | tail -5`
Expected: success (the old `ChannelGateway`/`StubGateway` are still present and used; `OutboundGateway` is added but may be unused — that is fine, or add `#[allow(dead_code)]` is NOT needed because the tests reference it).

- [ ] **Step 7: Commit**

```bash
git add backend/Cargo.toml backend/Cargo.lock backend/src/domain/conversations/channels.rs
git commit -m "feat(outbound): add reqwest + OutboundGateway enum (real LINE push)"
```

---

## Task 2: Migrate call sites + remove the stub

**Files:**
- Modify: `backend/src/domain/conversations/channels.rs` (`deliver_pending` takes the gateway; remove `ChannelGateway`/`StubGateway`)
- Modify: `backend/src/domain/conversations/handlers.rs` (~line 745)
- Modify: `backend/src/domain/customer_conversations/handlers.rs` (~line 483)
- Modify: `backend/src/domain/auto_reply/engine.rs` (~line 328, fn `dispatch(state: &AppState, …)`)
- Modify: `backend/src/domain/queue/worker.rs` (~line 122, fn `process_outbound(state: &Arc<AppState>, …)`)
- Modify: `backend/src/domain/messaging/service.rs` (~lines 191 and 432)

- [ ] **Step 1: `deliver_pending` takes the gateway; remove the old trait/stub**

In `channels.rs`:
(a) Change `deliver_pending`'s signature to accept the gateway as a new last parameter, and use it instead of building `StubGateway`. Replace `let gateway = StubGateway;` with the parameter, and change the loop call `gateway.send_batch(&platform, &recipient, batch)` to `gateway.send_batch(&platform, &recipient, batch).await`:
```rust
pub async fn deliver_pending(
    db: PgPool,
    hub: std::sync::Arc<crate::realtime::RealtimeHub>,
    conversation_id: String,
    message_id: String,
    platform: String,
    recipient: String,
    items: Vec<OutboundItem>,
    gateway: OutboundGateway,
) {
    // (was: let gateway = StubGateway;)
    …
    for batch in items.chunks(BATCH_CAP) {
        match gateway.send_batch(&platform, &recipient, batch).await {
            …
        }
    }
    …
}
```
(b) Delete the `pub trait ChannelGateway { … }` block and the `impl ChannelGateway for StubGateway { … }` block and the `pub struct StubGateway;` declaration. Keep `OutboundItem`, `BATCH_CAP`, `deliver_pending`, and the new `OutboundGateway`.

- [ ] **Step 2: Update the `deliver_pending` caller**

In `backend/src/domain/conversations/handlers.rs` (~line 745), add the gateway as the last argument to the `channels::deliver_pending(…)` call:
```rust
    tokio::spawn(channels::deliver_pending(
        state.db.clone(),
        state.realtime.clone(),
        id.clone(),
        message_id.clone(),
        platform.clone(),
        recipient,
        items,
        channels::OutboundGateway::from_config(&state.config),
    ));
```
Update the `use` for channels if needed so `OutboundGateway` is in scope (the file already references `channels::deliver_pending`, so `channels::OutboundGateway::…` works without a new import).

- [ ] **Step 3: Migrate customer_conversations/handlers.rs (~line 483)**

This site spawns a task moving `items`/`recipient`. Build the gateway before the spawn and move it in; await the call. Replace the `let gateway = StubGateway;` block:
```rust
        if !items.is_empty() {
            let gateway = crate::domain::conversations::channels::OutboundGateway::from_config(&state.config);
            tokio::spawn(async move {
                for batch in items.chunks(BATCH_CAP) {
                    if let Err(e) = gateway.send_batch("line", &recipient, batch).await {
                        tracing::warn!(error = %e, "customer-conversation LINE relay failed");
                    }
                }
            });
        }
```
Update this file's `use crate::domain::conversations::channels::{ChannelGateway, OutboundItem, StubGateway, BATCH_CAP};` → `use crate::domain::conversations::channels::{OutboundGateway, OutboundItem, BATCH_CAP};`. Ensure `state` is captured/owned appropriately for the `from_config` call (it runs before the spawn, so a borrow is fine).

- [ ] **Step 4: Migrate auto_reply/engine.rs (~line 328)**

Inside `dispatch(state: &AppState, …)`: replace `let gateway = StubGateway;` with `let gateway = OutboundGateway::from_config(&state.config);` and `gateway.send_batch(ctx.platform, ctx.platform_user_id, &items)` → `gateway.send_batch(ctx.platform, ctx.platform_user_id, &items).await`. Update the file `use …channels::{ChannelGateway, OutboundItem, StubGateway};` → `use …channels::{OutboundGateway, OutboundItem};`.

- [ ] **Step 5: Migrate queue/worker.rs (~line 122)**

Inside `process_outbound(state: &Arc<AppState>, …)`: replace `let gateway = StubGateway;` with `let gateway = OutboundGateway::from_config(&state.config);` and `gateway.send_batch("line", recipient, chunk)` → `gateway.send_batch("line", recipient, chunk).await`. Update the `use …channels::{ChannelGateway, OutboundItem, StubGateway, BATCH_CAP};` → `{OutboundGateway, OutboundItem, BATCH_CAP}`.

- [ ] **Step 6: Migrate messaging/service.rs (~lines 191 and 432)**

Both sites have `state`. At ~line 191 (the `"line" | "facebook"` arm): replace `let gateway = StubGateway;` with `let gateway = OutboundGateway::from_config(&state.config);` and `gateway.send_batch(&platform, &recipient, &[OutboundItem { content }])` → `… .await`. At ~line 432 (recall notice): replace `let gateway = StubGateway;` with `let gateway = OutboundGateway::from_config(&state.config);` and the `gateway.send_batch("line", …)` call → add `.await`. Update the file `use …channels::{ChannelGateway, OutboundItem, StubGateway};` → `{OutboundGateway, OutboundItem}`.

- [ ] **Step 7: Build + full test suite**

Run: `cd backend && cargo build 2>&1 | tail -5`
Expected: success, no references to `StubGateway`/`ChannelGateway` remain (grep to confirm: `grep -rn "StubGateway\|ChannelGateway" src` → no matches).

Run: `cd backend && cargo test 2>&1 | grep -E "test result|error\[" | tail -40`
Expected: all suites pass. Tests run with no token → `OutboundGateway::Stub` → no network; the `sent/partial/failed` behavior is identical to before (the Stub variant reproduces the old StubGateway outcomes). Pay attention to `messaging`, `queue`, `auto_reply`, `conversations`, `customer_*` test files.

- [ ] **Step 8: Commit**

```bash
git add backend/src/domain/conversations/channels.rs backend/src/domain/conversations/handlers.rs backend/src/domain/customer_conversations/handlers.rs backend/src/domain/auto_reply/engine.rs backend/src/domain/queue/worker.rs backend/src/domain/messaging/service.rs
git commit -m "refactor(outbound): route all sends through OutboundGateway; drop StubGateway"
```

---

## Final verification (after all tasks)

- [ ] `cd backend && cargo build` — clean
- [ ] `cd backend && grep -rn "StubGateway\|ChannelGateway" src` — no matches
- [ ] `cd backend && cargo test` — all suites green (outbound stubbed under tests; `build_push_body` + `from_config` unit tests pass)
- [ ] `detect_changes()` before the final commit; the only changed behavior is: with a real `LINE_CHANNEL_ACCESS_TOKEN`, `send_batch("line", …)` now performs a real push (manual/live verification deferred to the credentials step).
```

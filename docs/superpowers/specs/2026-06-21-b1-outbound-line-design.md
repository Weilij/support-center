# Outbound Foundation + Real LINE (Track B1) — Design Spec

**Date:** 2026-06-21
**Track:** B1 (backend, first sub-project of the multi-platform program)
**Status:** design approved, pending written-spec review

---

## 0. Context

The backend's outbound delivery is entirely stubbed: `ChannelGateway` (in `backend/src/domain/conversations/channels.rs`) has one impl, `StubGateway`, which returns a fake id for `"line"` and an error for everything else. It is used in **5 places**: `conversations::channels::deliver_pending`, `customer_conversations/handlers.rs`, `auto_reply/engine.rs`, `queue/worker.rs`, `messaging/service.rs`. There is **no HTTP client** in `Cargo.toml`. LINE inbound is fully implemented and the global config exposes `line_channel_access_token`.

B1 makes LINE actually send by adding an HTTP client and a real gateway behind the existing seam — establishing the real-gateway pattern that B2–B4 (FB/IG/Shopee) will extend.

---

## 1. Goal & non-goals

**Goal:** when `LINE_CHANNEL_ACCESS_TOKEN` is configured, outbound text messages to LINE conversations are delivered via the real LINE Messaging API; the observable `sent / partial / failed` delivery status reflects real results.

**Non-goals:**
- No FB / IG / Shopee outbound (later sub-projects).
- No per-team tokens — B1 uses the single global token (per-team credentials deferred).
- No inbound changes.
- No attachment/image/sticker sending — text only in B1.
- No reply-token flow — B1 uses the push API (works outside the reply window).
- No retry/backoff — a failed batch maps to the existing `failed`/`partial` status (retry is a later enhancement).

---

## 2. Current state (verified)

- `channels.rs`: `OutboundItem { content: String }`, `BATCH_CAP = 5`, `trait ChannelGateway { fn send_batch(&self, platform, recipient, items) -> Result<String,String> }`, `struct StubGateway`, and `deliver_pending(db, hub, conversation_id, message_id, platform, recipient, items)` which loops batches through the gateway and persists `delivery_status` + `is_sent` + `platform_message_id`, then broadcasts `message_updated`.
- 5 call sites instantiate `StubGateway` directly and call `send_batch` synchronously (all inside async fns).
- `config.line_channel_access_token: Option<String>`; test config sets it to `None`.
- No `reqwest`/`hyper`/`ureq` dependency.

---

## 3. HTTP client (§ files: Cargo.toml, AppState)

Add `reqwest` with `default-features = false, features = ["rustls-tls", "json"]` to `backend/Cargo.toml`. Construct one shared `reqwest::Client` once and reuse it (a `OnceLock<reqwest::Client>` in `channels.rs`, or a field on `AppState`). A single client pools connections; building per-call is wasteful.

---

## 4. Outbound gateway — enum dispatch (D1)

Replace the sync trait usage with a native-async enum (no `async-trait`/`dyn`):

```rust
pub enum OutboundGateway {
    Stub,
    Line(LineGateway),
}

impl OutboundGateway {
    /// Real LINE gateway when the token is configured; otherwise the stub
    /// (so dev/tests without a token make no network calls).
    pub fn from_config(config: &Config) -> Self {
        match config.line_channel_access_token.as_deref() {
            Some(t) if !t.is_empty() => OutboundGateway::Line(LineGateway::new(t.to_string())),
            _ => OutboundGateway::Stub,
        }
    }

    pub async fn send_batch(&self, platform: &str, recipient: &str, items: &[OutboundItem]) -> Result<String, String> { … }
}
```

- `Stub` keeps today's observable behavior (`"line"` → fake id, others → "not supported" error) so the existing tests pass unchanged.
- `Line` handles `platform == "line"` via the real API (§5); for any non-`line` platform it returns the same "not supported" error (FB/IG/Shopee arrive in later tracks).
- The existing `ChannelGateway` trait + `StubGateway` are removed (or `StubGateway` folds into the `Stub` variant); `OutboundItem` and `BATCH_CAP` stay.

**Call-site migration:** each of the 5 sites replaces `let gateway = StubGateway;` with `let gateway = OutboundGateway::from_config(&<config>);` and awaits `send_batch(...).await`. `deliver_pending` gains the gateway (or the config to build it) from its caller (`conversations/handlers.rs:745`, which has `AppState`). Each site has `AppState`/config access; thread it through.

---

## 5. LINE Messaging API call

`LineGateway { token: String }`:
- `build_push_body(recipient, items) -> serde_json::Value` (**pure, unit-tested**): `{ "to": recipient, "messages": [ {"type":"text","text": item.content}, … ] }`, at most `BATCH_CAP` messages (the caller already chunks by `BATCH_CAP`).
- `send_batch`: `POST https://api.line.me/v2/bot/message/push`, header `Authorization: Bearer <token>`, JSON body from `build_push_body`. On HTTP 200 → `Ok(<id>)` where the id is the response's `sentMessages[0].id` if present, else a synthesized `"line-<uuid>"`. On non-200 or transport error → `Err(<status/message>)`, so `deliver_pending`'s `sent/partial/failed` logic is unchanged.

---

## 6. Testing

- **Unit:** `build_push_body` — correct `to`, one text message per item, respects content; empty items → empty `messages`.
- **Integration (network-free):** the existing suite runs with no token → `OutboundGateway::Stub` → the send/deliver paths behave exactly as today (no real HTTP). Add/keep a test asserting `from_config` returns `Stub` when the token is absent and `Line` when present.
- **Live LINE send** (real token + a real recipient) is a manual/config step, deferred with the Cloudflare/webhook setup.

---

## 7. Files

- `backend/Cargo.toml` — add `reqwest`.
- `backend/src/domain/conversations/channels.rs` — `OutboundGateway` enum + `from_config` + `LineGateway` + `build_push_body` + shared `reqwest::Client`; remove `ChannelGateway`/`StubGateway` (or fold stub in); keep `OutboundItem`/`BATCH_CAP`/`deliver_pending` (now async-gateway).
- `backend/src/domain/conversations/handlers.rs`, `customer_conversations/handlers.rs`, `auto_reply/engine.rs`, `queue/worker.rs`, `messaging/service.rs` — swap `StubGateway` for `OutboundGateway::from_config(&config)` + `.await`.
- (optional) `backend/src/state.rs` (AppState) — shared `reqwest::Client` if not using a `OnceLock`.

---

## 8. Verification

- `cargo build`; `cargo test` (existing suites green — outbound stays stubbed under tests); new unit test for `build_push_body` + `from_config`.
- `detect_changes()` before commit; impact-check `deliver_pending`/the gateway before editing.
- Live: with a real `LINE_CHANNEL_ACCESS_TOKEN`, a sent message reaches a LINE user and the message row flips to `sent` with a real `platform_message_id` (manual, post-credentials).

---

## 9. Resolved decisions
- **D1** — dispatch: **`enum OutboundGateway`** (native async, no async-trait/dyn).
- **D2** — message scope: **text only** in B1.
- Token: **global** `LINE_CHANNEL_ACCESS_TOKEN`. Delivery: **push** API. Errors map to existing `failed`/`partial`. No retry/backoff in B1.

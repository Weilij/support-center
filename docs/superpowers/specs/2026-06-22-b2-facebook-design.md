# Facebook Messenger End-to-End (Track B2) — Design Spec

**Date:** 2026-06-22
**Track:** B2 (backend, second sub-project of the multi-platform program)
**Depends on:** B1 (the `OutboundGateway` + reqwest foundation, merged)
**Status:** design approved, pending written-spec review

---

## 0. Context

Facebook **inbound already works**: `webhooks/handlers.rs::facebook_webhook` validates the GET handshake (`facebook_verify_token`) and the `x-hub-signature-256` HMAC (`facebook_app_secret`), then for each `page` entry's `messaging` items it parses `item.message` via `parse::normalize_facebook` and calls `ingest::ingest_message(platform:"facebook", …)`. What's missing: (a) **outbound** — `OutboundGateway` only handles `"line"` (returns "not supported" for `"facebook"`), and there's no page access token in config; (b) the webhook only processes plain `message` items and ignores **echo / postback / delivery / read** events.

B2 completes FB: real Send-API outbound + the four inbound event types. It also performs the small multi-platform refactor of the B1 gateway (which was single-platform).

---

## 1. Goal & non-goals

**Goal:** agents can reply to FB conversations through the real Graph Send API, and the FB webhook correctly handles echo, postback, delivery receipts, and read receipts.

**Non-goals:**
- No per-team tokens — global `FACEBOOK_PAGE_ACCESS_TOKEN` (mirrors B1's global LINE token).
- Text-only outbound (no image/template sends in B2).
- No Instagram/Shopee (B3/B4). (Note for B3: IG read uses the `messaging_seen` webhook field, not FB's `message_reads`, and IG has **no** delivery receipt.)
- No new webhook-subscription automation — subscribing the FB app to `messages`, `messaging_postbacks`, `message_deliveries`, `message_reads` is a console/config step, not code.

---

## 2. Current state (verified)

- `config.rs`: `facebook_app_secret`, `facebook_verify_token` exist; **no** page access token.
- `conversations/channels.rs` (post-B1): `OutboundGateway { Stub, Line(LineGateway) }`, `from_config(&Config)` picks one variant, `async send_batch(platform, recipient, items) -> Result<String,String>`; `build_push_body` (LINE); shared `http_client()` with a 10s timeout. The send flow already passes the conversation's `platform` to `send_batch`, so routing `"facebook"` is automatic once the gateway handles it.
- `messages` table: `platform_message_id TEXT` (unique partial index), `delivery_status TEXT NOT NULL DEFAULT 'delivered'`, `read_at TEXT`.
- `webhooks/handlers.rs::facebook_webhook`: processes only `item.message` (the `object=="page"` loop).
- `parse::normalize_facebook(message) -> Normalized { content, kind, media, metadata }`.
- `ingest::ingest_message(&state, InboundMessage { platform, platform_user_id, default_display_name, platform_message_id, normalized })`.

---

## 3. Stage 1 — Outbound (FB Send API)

### 3a. Config
Add `facebook_page_access_token: Option<String>` to `Config`, read from `FACEBOOK_PAGE_ACCESS_TOKEN` (empty → None). Add `None` to `test_config()`.

### 3b. Gateway refactor (multi-platform dispatch)
The B1 enum is single-platform. Refactor to a struct holding the configured tokens; the **public API (`from_config`, `send_batch`) stays identical**, so the 7 call sites are untouched:
```rust
pub struct OutboundGateway {
    line: Option<String>,      // LINE channel access token
    facebook: Option<String>,  // FB page access token
}

impl OutboundGateway {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            line: config.line_channel_access_token.clone().filter(|t| !t.is_empty()),
            facebook: config.facebook_page_access_token.clone().filter(|t| !t.is_empty()),
        }
    }

    pub async fn send_batch(&self, platform: &str, recipient: &str, items: &[OutboundItem]) -> Result<String, String> {
        match platform {
            "line" => match &self.line {
                Some(tok) => line_push(tok, recipient, items).await,
                // No token (dev/tests): preserve the old stub "success" so behavior is unchanged.
                None => Ok(format!("stub-line-{}", uuid::Uuid::new_v4())),
            },
            "facebook" => match &self.facebook {
                Some(tok) => fb_send(tok, recipient, items).await,
                None => Err("Outbound delivery is not supported for platform 'facebook'".into()),
            },
            other => Err(format!("Outbound delivery is not supported for platform '{other}'")),
        }
    }
}
```
`line_push` is the B1 LINE call extracted into a free fn; `LineGateway`/the old variants are removed. `OutboundItem`/`BATCH_CAP`/`build_push_body`/`http_client` stay.

Behavior preservation: with no tokens (the test default since B1), `line` still returns a fake id and non-line returns "not supported" — identical to today, so the suite stays green.

### 3c. FB Send API
- `fb_send_body(recipient, content) -> serde_json::Value` (**pure, unit-tested**): `{ "recipient": {"id": recipient}, "messaging_type": "RESPONSE", "message": {"text": content} }`.
- `fb_send(token, recipient, items)`: FB has no batch endpoint — POST one message per item to `https://graph.facebook.com/v21.0/me/messages?access_token=<token>` with `fb_send_body`; on the first failure return `Err`; on success return the response `message_id` (fallback `fb-<uuid>`). Reuse the shared `http_client()`.

---

## 4. Stage 2 — Inbound enhancements (FB webhook)

Extend the `object=="page"` loop in `facebook_webhook` to branch per messaging item (today it only handles `item.message`):

- **Echo** (`item.message.is_echo == true`): **skip** — `continue` without ingesting (avoids re-ingesting the page's own outbound as a customer message). Counted separately or simply skipped (not added to `total`).
- **Message** (non-echo `item.message`): unchanged — `normalize_facebook` + `ingest_message`.
- **Postback** (`item.postback`): synthesize a `Normalized` text from `postback.title` (preferred) or `postback.payload`; ingest as an inbound message (kind `"text"`, the raw postback retained in `metadata`). `platform_message_id` is absent for postbacks → pass `None`.
- **Delivery** (`item.delivery`): for each id in `delivery.mids`, `UPDATE messages SET delivery_status='delivered', updated_at=now WHERE platform_message_id = $1`. (No-op when the id isn't ours.)
- **Read** (`item.read`): FB gives only `read.watermark` (ms epoch, no ids). Resolve the conversation for `item.sender.id`/`item.recipient.id` and `UPDATE messages SET read_at=now WHERE conversation_id = $conv AND sender_type='agent' AND read_at IS NULL AND sent_at <= <watermark-as-iso>`. (Only our outbound/agent messages get a read stamp.)

A small helper for the receipt updates lives in `webhooks/ingest.rs` (e.g. `mark_delivered(db, mids)` and `mark_read(db, conversation_or_sender, watermark)`), keeping `facebook_webhook` readable.

Postback normalization: add `normalize_facebook_postback(postback: &Value) -> Normalized` in `parse.rs` (pure, unit-tested) rather than overloading `normalize_facebook`.

---

## 5. Files

- `backend/src/config.rs` — add `facebook_page_access_token` (+ `test_config` None).
- `backend/src/domain/conversations/channels.rs` — struct `OutboundGateway`, `line_push`, `fb_send`, `fb_send_body`; remove the enum variants/`LineGateway`.
- `backend/src/domain/webhooks/handlers.rs` — per-item event branches in `facebook_webhook`.
- `backend/src/domain/webhooks/parse.rs` — `normalize_facebook_postback`.
- `backend/src/domain/webhooks/ingest.rs` — `mark_delivered` / `mark_read` helpers.
- Tests: `backend/tests/webhooks.rs` (echo-skipped, postback-ingested, delivery/read status), unit tests for `fb_send_body` + `normalize_facebook_postback` + gateway dispatch.

---

## 6. Verification

- `cargo build` + `cargo test` green, network-free: the test harness has no FB token (B1 opt-in default), so `send_batch("facebook", …)` returns "not supported" and `"line"` stubs — no real HTTP. Unit tests: `fb_send_body`, `normalize_facebook_postback`, `from_config` dispatch (line/fb token presence). Webhook integration tests drive echo/postback/delivery/read through the signed `facebook_webhook` (reuse the existing FB webhook test setup with `facebook_app_secret` set + a valid signature).
- `detect_changes()` before commit; the gateway refactor keeps the public API stable (call sites unchanged).
- Live FB send + receipts need a real `FACEBOOK_PAGE_ACCESS_TOKEN`, the app subscribed to the `messages`/`messaging_postbacks`/`message_deliveries`/`message_reads` fields, and a public webhook (deferred config step).

---

## 7. Resolved decisions
- **Echo** → **skip** (don't ingest).
- Outbound token → **global** `FACEBOOK_PAGE_ACCESS_TOKEN`; **text-only**; Graph API `v21.0`, `messaging_type: "RESPONSE"`.
- **Gateway** → refactor the enum into a struct dispatcher; **public API unchanged** (7 call sites untouched); no-token behavior preserved (line stubs, others "not supported").
- **Delivery** → update by `mids` → `delivery_status='delivered'`. **Read** → watermark → `read_at` on the conversation's agent messages with `sent_at <= watermark`.
- Two stages (outbound, inbound), each subagent-built, reviewed, committed.

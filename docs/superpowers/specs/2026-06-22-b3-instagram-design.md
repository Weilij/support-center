# Instagram End-to-End (Track B3) — Design Spec

**Date:** 2026-06-22
**Track:** B3 (backend, third sub-project of the multi-platform program)
**Depends on:** B1 (OutboundGateway + reqwest), B2 (FB Send API + webhook event handling) — both merged
**Status:** design approved, pending written-spec review

---

## 0. Context

Instagram messaging rides the **same Messenger Platform** as Facebook: IG events arrive on the existing `/api/webhooks/facebook` endpoint as `object == "instagram"` (the handler validates that object type but currently only processes `object == "page"`), signed with the same `facebook_app_secret`; outbound uses the **same Graph Send API** (`/me/messages`). B2 already built the multi-platform `OutboundGateway` dispatcher (`{line, facebook}`) and the FB per-event webhook handling — B3 extends both to Instagram.

**IG differences from FB (verified against Meta IG docs, 2026-06-22):** same `entry[].messaging[]` envelope with `sender`/`recipient`; subscribe fields `messages` / `message_echoes` / `messaging_seen` / `message_reactions`; echoes via `is_echo`/`is_self`. IG has **no delivery receipt**, and IG "seen" may carry `read.mid` rather than FB's `read.watermark` — the exact seen/reaction/story payload field names were not on the fetched doc page, so the IG handlers are built **defensively** and the implementer confirms field names against the Meta IG reference during implementation.

---

## 1. Goal & non-goals

**Goal:** agents can reply to Instagram conversations via the Graph Send API, and the webhook processes IG messages (echo-skipped), postbacks, seen (read), reactions, and story mentions/replies — persisted through the existing customer/conversation/message pipeline as platform `"instagram"`.

**Non-goals:** no delivery receipts (IG has none); no per-team tokens; text-only outbound; no Shopee (B4); no new `enum Platform` refactor (platforms stay stringly-typed).

---

## 2. Current state (verified)

- `webhooks/handlers.rs::facebook_webhook`: validates `object ∈ {page, instagram, user}`; processes only `object == "page"` via an inner `for item in items` loop that (post-B2) branches on message(echo-skip)/postback/delivery/read.
- `conversations/channels.rs` (post-B2): `OutboundGateway { line: Option<String>, facebook: Option<String> }`, `from_config`, `send_batch(platform,…)`; `fb_send`/`fb_send_body` (Graph Send API); `mark_delivered`/`mark_read` live in `ingest.rs`; `normalize_facebook` + `normalize_facebook_postback` in `parse.rs`.
- `messages` table: `metadata TEXT` (JSON); the agent-message serializer already exposes `metadata.reactions` (`messaging/handlers.rs:667`); `read_at` added in B2 (migration 0013).
- `config.rs`: `facebook_page_access_token` (B2). No IG token.

---

## 3. Stage 1 — Outbound (IG via Graph Send API)

### 3a. Config
Add `instagram_access_token: Option<String>` (env `INSTAGRAM_ACCESS_TOKEN`, empty→None) + `test_config()` None.

### 3b. Gateway
`OutboundGateway` gains `instagram: Option<String>`. In `from_config`, **fall back to the page token** when the IG token is unset:
```rust
instagram: config.instagram_access_token.clone().filter(|t| !t.is_empty())
    .or_else(|| config.facebook_page_access_token.clone().filter(|t| !t.is_empty())),
```
`send_batch`: add an `"instagram"` arm → `Some(tok) => fb_send(tok, recipient, items).await` (IG uses the same Graph `/me/messages`), `None => Err("…not supported…'instagram'")`. The send routing already passes the conversation's platform, so an `"instagram"` conversation dispatches here automatically.

### 3c. Tests
`from_config` reflects the IG token and the page-token fallback (IG token unset but page token set → `instagram.is_some()`). Suite stays network-free (no tokens in the test harness).

---

## 4. Stage 2 — Inbound (`object == "instagram"`)

### 4a. Refactor: shared per-item processor
Extract the B2 inline per-item handler into `async fn process_messaging_item(state, platform: &str, default_name: &str, item: &Value, total, failed, last_error)`, and call it from both objects:
- `object == "page"` → `process_messaging_item(&state, "facebook", "Facebook User", item, …)`
- `object == "instagram"` → `process_messaging_item(&state, "instagram", "Instagram User", item, …)`

The shared processor handles all event types; FB-only (delivery) and IG-only (reactions) branches simply don't fire for the other platform. FB behavior is unchanged.

### 4b. Events in the shared processor
- **message** + **echo skip** (`message.is_echo == true` OR `message.is_self == true` → `continue`): ingest via `normalize_facebook`/`normalize_instagram` (see 4c) with the given `platform`.
- **postback** (`item.postback`): reuse `normalize_facebook_postback`.
- **delivery** (`item.delivery`): `mark_delivered(mids)` — fires for FB only (IG never sends it).
- **seen/read** (`item.read`): **D1 — handle both shapes.** If `read.watermark` present → `mark_read(platform, sender, watermark)` (existing). Else if `read.mid` present → look up that message's `sent_at` by `platform_message_id`, convert to ms, then `mark_read(platform, sender, that_ms)`. (New helper `mark_read_by_mid` or a watermark lookup in `ingest.rs`.)
- **reactions** (`item.reaction`): new `apply_reaction(db, mid, reaction: &Value)` — find the message by `platform_message_id == reaction.mid`; on `action == "react"` append `{ reaction, emoji }` to `metadata.reactions` (JSON array, dedup by reactor if a reactor id is present); on `action == "unreact"` remove it. Persist the updated `metadata`. Uses the existing `metadata.reactions` convention.

### 4c. Story mentions/replies (D2 — label + raw)
Add `normalize_instagram(message) -> Normalized` that delegates to `normalize_facebook` and, when the attachment type indicates a story (`story_mention`, or `message.reply_to.story` present), sets a clear content label (`[Story mention]` / `[Story reply]`) while keeping the raw attachment/reply object in `metadata`. For ordinary text/media IG messages it behaves like `normalize_facebook`.

**Field-name verification:** the exact IG field names for `read` (watermark vs mid), `reaction` (`action`/`reaction`/`emoji`/`mid`), and story attachment types must be confirmed against the Meta IG Messaging reference during implementation; the handlers read them via `.get(...)` defensively (missing fields → no-op, never panic).

---

## 5. Files

- `config.rs` — `instagram_access_token` + fallback in `from_config`.
- `conversations/channels.rs` — `instagram` field + `send_batch` arm (reuses `fb_send`).
- `webhooks/handlers.rs` — extract `process_messaging_item`; add the `object == "instagram"` branch; reaction + seen-by-mid branches.
- `webhooks/parse.rs` — `normalize_instagram` (story labels).
- `webhooks/ingest.rs` — `apply_reaction`; read-by-mid lookup (or extend `mark_read`).
- Tests: unit (`from_config` IG/fallback, `apply_reaction`, `normalize_instagram` story, read-by-mid); webhook integration (`object:"instagram"` message/echo/postback/seen/reaction/story).

---

## 6. Verification

- `cargo build` + `cargo test` green, network-free (no IG token in tests → "not supported"; inbound runs through the signed webhook). New unit + IG webhook integration tests.
- Run `cargo build --tests` as part of verification (B2 process note: lib-only build missed an integration fixture break).
- `detect_changes()` before commit; the gateway public API stays stable.
- Live IG send/seen/reactions need a real IG token (or the linked page token), the IG professional account linked to the FB page, and webhook subscriptions (`messages`/`message_echoes`/`messaging_seen`/`message_reactions`) — deferred config step.

---

## 7. Resolved decisions
- IG token → dedicated `INSTAGRAM_ACCESS_TOKEN`, **falling back to `FACEBOOK_PAGE_ACCESS_TOKEN`**.
- Inbound events → message+echo, postback, **seen (both `watermark` and `mid`)**, reactions (→ `metadata.reactions`), story mentions/replies (**label + raw in metadata**). No delivery (IG has none).
- Refactor the per-item webhook handler into a shared `process_messaging_item(platform, …)` used by both `page` (facebook) and `instagram`; FB behavior unchanged.
- Two stages (outbound, inbound), each subagent-built, reviewed, committed.

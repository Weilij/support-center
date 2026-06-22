# Instagram End-to-End (Track B3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Instagram outbound (Graph Send API) and process the webhook's `object == "instagram"` events (message/echo/postback/seen/reactions/story), reusing the B2 infrastructure.

**Architecture:** Add an `instagram` token to the gateway dispatcher (falling back to the FB page token) and reuse `fb_send`; extract the FB per-item webhook handler into a shared `process_messaging_item(platform, …)` called for both `page`(facebook) and `instagram`, adding seen-by-mid, reactions, and story handling.

**Tech Stack:** Rust, axum, sqlx, reqwest, serde_json, chrono.

**Spec:** `docs/superpowers/specs/2026-06-22-b3-instagram-design.md`

---

## File Structure

- `backend/src/config.rs` — `instagram_access_token` + fallback in `from_config`.
- `backend/src/domain/conversations/channels.rs` — `instagram` field + `send_batch` arm (reuses `fb_send`).
- `backend/src/domain/webhooks/parse.rs` — `normalize_instagram` (story labels).
- `backend/src/domain/webhooks/ingest.rs` — `apply_reaction`, `mark_read_by_mid`.
- `backend/src/domain/webhooks/handlers.rs` — extract `process_messaging_item`; add the `instagram` object branch.
- Tests: `backend/tests/webhooks.rs` + unit tests in channels/parse.

---

## Task 1: Outbound — Instagram via the gateway

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/domain/conversations/channels.rs`

- [ ] **Step 1: Add the IG token config**

In `backend/src/config.rs`, next to `facebook_page_access_token`:
```rust
    /// Instagram messaging access token (INSTAGRAM_ACCESS_TOKEN); falls back to
    /// the Facebook page token when unset (IG messaging uses the linked page).
    pub instagram_access_token: Option<String>,
```
In the constructor:
```rust
            instagram_access_token: std::env::var("INSTAGRAM_ACCESS_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
```
In `test_config()` add `instagram_access_token: None,`.

- [ ] **Step 2: Update the gateway unit test**

In `channels.rs` `gateway_tests`, add a test for the IG token + fallback:
```rust
    #[test]
    fn from_config_instagram_token_with_fallback() {
        let mut c = crate::config::test_config();
        c.instagram_access_token = None;
        c.facebook_page_access_token = None;
        assert!(OutboundGateway::from_config(&c).instagram.is_none());

        // Falls back to the page token when the IG token is unset.
        c.facebook_page_access_token = Some("PAGE".into());
        assert!(OutboundGateway::from_config(&c).instagram.is_some());

        // Dedicated IG token wins.
        c.instagram_access_token = Some("IG".into());
        assert!(OutboundGateway::from_config(&c).instagram.is_some());
    }
```
Run `cd backend && cargo test --lib gateway_tests 2>&1 | tail -20` → FAIL (`OutboundGateway` has no `instagram` field).

- [ ] **Step 3: Add the IG field + dispatch**

In `channels.rs`, add `instagram` to the struct:
```rust
pub struct OutboundGateway {
    line: Option<String>,
    facebook: Option<String>,
    instagram: Option<String>,
}
```
In `from_config`, add (with the page-token fallback):
```rust
            instagram: config
                .instagram_access_token
                .clone()
                .filter(|t| !t.is_empty())
                .or_else(|| config.facebook_page_access_token.clone().filter(|t| !t.is_empty())),
```
In `send_batch`, add an arm BEFORE the `other =>` catch-all:
```rust
            "instagram" => match &self.instagram {
                Some(tok) => fb_send(tok, recipient, items).await,
                None => Err("Outbound delivery is not supported for platform 'instagram'".into()),
            },
```
(IG uses the same Graph `/me/messages` endpoint as FB, so `fb_send` is reused as-is.)

- [ ] **Step 4: Tests pass + build**

`cd backend && cargo test --lib gateway_tests 2>&1 | tail -10` → all passing (incl. the new IG test).
`cd backend && cargo build 2>&1 | tail -5` → success.

- [ ] **Step 5: Commit**

```bash
git add backend/src/config.rs backend/src/domain/conversations/channels.rs
git commit -m "feat(outbound): Instagram gateway dispatch (token with page-token fallback)"
```

---

## Task 2: Inbound — `object == "instagram"` (shared per-item processor)

**Files:**
- Modify: `backend/src/domain/webhooks/parse.rs`
- Modify: `backend/src/domain/webhooks/ingest.rs`
- Modify: `backend/src/domain/webhooks/handlers.rs`
- Test: `backend/tests/webhooks.rs`

- [ ] **Step 1: Unit test for `normalize_instagram` story handling**

In `parse.rs`'s `#[cfg(test)] mod tests`:
```rust
    #[test]
    fn instagram_story_mention_is_labelled() {
        let n = normalize_instagram(&json!({
            "attachments": [{ "type": "story_mention", "payload": { "url": "https://x/s.jpg" } }]
        }));
        assert_eq!(n.content, "[Story mention]");
        assert!(n.metadata.contains_key("storyMention"));
    }

    #[test]
    fn instagram_plain_text_passes_through() {
        let n = normalize_instagram(&json!({ "mid": "m1", "text": "hi" }));
        assert_eq!(n.content, "hi");
        assert_eq!(n.kind, "text");
    }
```
Run `cd backend && cargo test --lib parse 2>&1 | tail -15` → FAIL (missing fn).

- [ ] **Step 2: Implement `normalize_instagram`**

In `parse.rs` (next to `normalize_facebook`):
```rust
/// Instagram inbound message. Delegates to `normalize_facebook` (same envelope)
/// and labels story mentions / replies, keeping the raw object in metadata.
/// (Exact IG attachment field names confirmed vs the Meta IG reference.)
pub fn normalize_instagram(message: &Value) -> Normalized {
    let is_story_mention = message
        .get("attachments")
        .and_then(Value::as_array)
        .map(|a| a.iter().any(|att| att.get("type").and_then(Value::as_str) == Some("story_mention")))
        .unwrap_or(false);
    let story_reply = message.get("reply_to").and_then(|r| r.get("story")).cloned();

    let mut n = normalize_facebook(message);
    if is_story_mention {
        n.content = "[Story mention]".into();
        if let Some(atts) = message.get("attachments") {
            n.metadata.insert("storyMention".into(), atts.clone());
        }
    } else if let Some(story) = story_reply {
        if n.content.is_empty() || n.content == "[Unknown message]" {
            n.content = "[Story reply]".into();
        }
        n.metadata.insert("storyReply".into(), story);
    }
    n
}
```
Run `cd backend && cargo test --lib parse 2>&1 | tail -10` → PASS.

- [ ] **Step 3: Add `apply_reaction` + `mark_read_by_mid` in `ingest.rs`**

`ingest.rs` already has `mark_delivered`, `mark_read`, `watermark_to_iso`, and imports `PgPool`/`now_iso`/`json`. Add:
```rust
/// IG/FB message reaction: update the target message's `metadata.reactions`.
/// (Reaction field names — action/reaction/emoji/mid — confirmed vs the Meta
/// IG reference; read defensively.)
pub async fn apply_reaction(db: &PgPool, reaction: &serde_json::Value) {
    let Some(mid) = reaction.get("mid").and_then(serde_json::Value::as_str) else { return };
    let action = reaction.get("action").and_then(serde_json::Value::as_str).unwrap_or("react");
    let react_type = reaction.get("reaction").and_then(serde_json::Value::as_str);
    let emoji = reaction.get("emoji").and_then(serde_json::Value::as_str);

    // None => no such message; Some(meta_text) => found (meta_text may be NULL/None).
    let found: Option<Option<String>> =
        sqlx::query_scalar("SELECT metadata FROM messages WHERE platform_message_id = $1")
            .bind(mid)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    let Some(meta_text) = found else { return };
    let mut meta: serde_json::Value =
        meta_text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    if !meta.is_object() {
        meta = json!({});
    }
    let arr = meta
        .as_object_mut()
        .unwrap()
        .entry("reactions")
        .or_insert_with(|| json!([]));
    if let Some(list) = arr.as_array_mut() {
        if action == "unreact" {
            list.retain(|r| r.get("reaction").and_then(serde_json::Value::as_str) != react_type);
        } else {
            list.push(json!({ "reaction": react_type, "emoji": emoji }));
        }
    }
    if let Err(e) = sqlx::query("UPDATE messages SET metadata = $1, updated_at = $2 WHERE platform_message_id = $3")
        .bind(meta.to_string())
        .bind(now_iso())
        .bind(mid)
        .execute(db)
        .await
    {
        tracing::warn!(error = %e, "reaction metadata update failed");
    }
}

/// Read receipt keyed by a specific message id (IG "seen" may carry `read.mid`
/// instead of a watermark): mark agent messages up to that message's sent_at.
pub async fn mark_read_by_mid(db: &PgPool, platform: &str, platform_user_id: &str, mid: &str) {
    let at: Option<Option<String>> =
        sqlx::query_scalar("SELECT sent_at FROM messages WHERE platform_message_id = $1")
            .bind(mid)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    let Some(Some(sent_at)) = at else { return };
    if let Err(e) = sqlx::query(
        "UPDATE messages SET read_at = $1
         WHERE sender_type = 'agent' AND read_at IS NULL AND sent_at <= $2
           AND conversation_id IN (
             SELECT c.id FROM conversations c
             JOIN customers cu ON cu.id = c.customer_id
             WHERE cu.platform = $3 AND cu.platform_user_id = $4
           )",
    )
    .bind(now_iso())
    .bind(&sent_at)
    .bind(platform)
    .bind(platform_user_id)
    .execute(db)
    .await
    {
        tracing::warn!(error = %e, "read-by-mid update failed");
    }
}
```
Run `cd backend && cargo build 2>&1 | tail -5` → success.

- [ ] **Step 4: Extract `process_messaging_item` + add the IG object branch in `handlers.rs`**

Replace the entire `if object == "page" { … }` block (currently lines ~354-414) with calls to a shared helper, and ADD a sibling `process_messaging_item` function above `facebook_webhook` (or below it in the same module). Insert the helper:
```rust
enum ItemResult {
    None,
    Ingested(Result<(), String>),
}

/// Handle one `messaging[]` item for a Meta platform (facebook | instagram).
async fn process_messaging_item(
    state: &std::sync::Arc<AppState>,
    platform: &str,
    default_name: &str,
    item: &Value,
) -> ItemResult {
    let sender = item["sender"]["id"].as_str().unwrap_or_default().to_string();
    if let Some(message) = item.get("message").filter(|m| m.is_object()) {
        // Skip the page/account's own echoed messages.
        if message.get("is_echo").and_then(Value::as_bool).unwrap_or(false)
            || message.get("is_self").and_then(Value::as_bool).unwrap_or(false)
        {
            return ItemResult::None;
        }
        let normalized = if platform == "instagram" {
            parse::normalize_instagram(message)
        } else {
            parse::normalize_facebook(message)
        };
        let mid = message.get("mid").and_then(Value::as_str);
        return ItemResult::Ingested(
            ingest::ingest_message(
                state,
                InboundMessage { platform, platform_user_id: &sender, default_display_name: default_name, platform_message_id: mid, normalized },
            )
            .await
            .map(|_| ()),
        );
    }
    if let Some(postback) = item.get("postback") {
        let normalized = parse::normalize_facebook_postback(postback);
        return ItemResult::Ingested(
            ingest::ingest_message(
                state,
                InboundMessage { platform, platform_user_id: &sender, default_display_name: default_name, platform_message_id: None, normalized },
            )
            .await
            .map(|_| ()),
        );
    }
    if let Some(delivery) = item.get("delivery") {
        let mids: Vec<&str> = delivery
            .get("mids")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        ingest::mark_delivered(&state.db, &mids).await;
        return ItemResult::None;
    }
    if let Some(read) = item.get("read") {
        if let Some(wm) = read.get("watermark").and_then(Value::as_i64) {
            ingest::mark_read(&state.db, platform, &sender, wm).await;
        } else if let Some(mid) = read.get("mid").and_then(Value::as_str) {
            ingest::mark_read_by_mid(&state.db, platform, &sender, mid).await;
        }
        return ItemResult::None;
    }
    if let Some(reaction) = item.get("reaction") {
        ingest::apply_reaction(&state.db, reaction).await;
        return ItemResult::None;
    }
    ItemResult::None
}
```
Then replace the `if object == "page" { … }` block with:
```rust
    if object == "page" || object == "instagram" {
        let (platform, default_name) = if object == "instagram" {
            ("instagram", "Instagram User")
        } else {
            ("facebook", "Facebook User")
        };
        for entry in entries.unwrap_or(&Vec::new()) {
            let Some(items) = entry.get("messaging").and_then(Value::as_array) else { continue };
            for item in items {
                match process_messaging_item(&state, platform, default_name, item).await {
                    ItemResult::Ingested(r) => {
                        total += 1;
                        if let Err(e) = r {
                            failed += 1;
                            last_error = Some(e);
                        }
                    }
                    ItemResult::None => {}
                }
            }
        }
    }
```
Confirm `InboundMessage`, `parse`, `ingest`, `AppState`, `Value` are already imported in this file (they are). If `ingest_message` returns `Result<(), String>` already, `.map(|_| ())` is harmless; if its Ok type differs, `.map(|_| ())` normalizes it.

- [ ] **Step 5: Webhook integration tests for Instagram**

In `backend/tests/webhooks.rs`, reuse the existing signed-FB-webhook helper (HMAC over the body with `facebook_app_secret`) but POST payloads with `"object": "instagram"`. Add tests asserting real DB effects:
- **IG message ingested as platform "instagram":** a signed `instagram` payload with `entry[].messaging[].message.text` creates a message whose customer is platform `"instagram"`.
- **IG echo skipped:** `message.is_echo=true` creates no message.
- **IG reaction:** seed a message with a known `platform_message_id`, POST `messaging[].reaction { mid, action:"react", reaction:"love", emoji:"❤️" }`; assert that message's `metadata` now contains `reactions` with the entry.
- **IG seen by mid:** seed an `agent` message (platform "instagram" customer + conversation) with a `platform_message_id` and `sent_at`; POST `messaging[].read { mid: <that id> }`; assert `read_at` is set.
- **IG story mention:** a `message.attachments[0].type == "story_mention"` payload ingests a message whose content is `[Story mention]`.
Reuse `tests/common` seed helpers (`seed_customer`/`seed_conversation`/`seed_message`) for the seeded rows. Network-free.

- [ ] **Step 6: Build + suites**

- `cd backend && cargo build 2>&1 | tail -5` → success.
- `cd backend && cargo build --tests 2>&1 | tail -5` → success (catches integration-fixture breaks; B2 process note).
- `cd backend && cargo test --test webhooks 2>&1 | grep -E "test result|error\[|FAILED"` → green.
- `cd backend && cargo test 2>&1 | grep -E "test result|error\[" | tail -40` → all suites green (FB page behavior unchanged through the shared processor).

- [ ] **Step 7: Commit**

```bash
git add backend/src/domain/webhooks/parse.rs backend/src/domain/webhooks/ingest.rs backend/src/domain/webhooks/handlers.rs backend/tests/webhooks.rs
git commit -m "feat(webhooks): Instagram object handling (message/echo/postback/seen/reactions/story)"
```

---

## Final verification (after all tasks)

- [ ] `cd backend && cargo build` + `cargo build --tests` — clean
- [ ] `cd backend && cargo test` — all suites green (FB page path unchanged; IG path + unit tests pass)
- [ ] `detect_changes()` before the final commit — gateway public API stable; the FB `object=="page"` path runs through the same shared processor (verify FB webhook tests still pass). Live IG send/seen/reactions deferred to the credentials + webhook-subscription step.
```

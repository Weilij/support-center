# Platform Profile (Real Name + Avatar) — Design Spec

**Date:** 2026-06-24
**Track:** backend platform integration + minimal frontend rendering
**Status:** design approved, pending written-spec review

---

## 0. Context

Inbound messages from LINE/Facebook/Instagram create a customer record whose
`display_name` defaults to a placeholder — `"LINE User"` (`webhooks/handlers.rs:202`),
and the equivalent `"Facebook User"` / `"Instagram User"` for Meta. The end-user's
real name is never fetched, so the inbox shows the placeholder and an initials
avatar. `ingest.rs:575` already carries a `TODO(live-platform)` marker for the
profile fetch (CRD 2818).

Each platform exposes a profile lookup that returns the display name **and** an
avatar URL. The outbound gateway (`conversations/channels.rs::OutboundGateway`)
already holds the per-platform credentials (`line` / `facebook` / `instagram`)
and a shared `reqwest` client, so it is the natural home for the fetch.

`customers.avatar_url TEXT` already exists (`migrations/0001_init.sql:47`) and is
serialized by the customer store and the conversation **list** query
(`conversations/store.rs:206` → `avatarUrl`). **No migration is required.**

---

## 1. Goal & non-goals

**Goal:** When a customer record is created — or already exists but still carries
the platform placeholder name — fetch the end-user's real `displayName` and avatar
from the platform and store them, so the inbox shows the real name and photo. Cover
LINE, Facebook, and Instagram.

**Non-goals:**
- **No periodic re-fetch.** Profile is fetched only while the stored name is a
  placeholder (or the customer is new). Once a real name is stored, we stop calling.
- **No Shopee** (no chat profile in scope).
- **No new DB column / migration** — reuse `customers.avatar_url`.
- **No outbound-gateway change** — `send_batch` (CRITICAL hub per GitNexus) is
  untouched; we only **add** a `fetch_profile` method to the same struct.
- **No avatar storage/proxying** — store the platform-hosted URL as-is; the
  frontend `<img>` loads it directly (platform CDNs allow hotlinking for these).

---

## 2. Architecture & units

### 2.1 `OutboundGateway::fetch_profile` (new method, `channels.rs`)

```rust
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Profile {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl OutboundGateway {
    /// Best-effort end-user profile lookup. Returns an empty `Profile` (both
    /// fields `None`) on any failure, missing token, or empty fields — never errors.
    pub async fn fetch_profile(&self, platform: &str, user_id: &str) -> Profile { ... }
}
```

Per-platform dispatch mirrors `send_batch`:

| Platform  | Request                                                                                   | Name field    | Avatar field  |
|-----------|-------------------------------------------------------------------------------------------|---------------|---------------|
| line      | `GET https://api.line.me/v2/bot/profile/{user_id}`, `Authorization: Bearer {line}`        | `displayName` | `pictureUrl`  |
| facebook  | `GET https://graph.facebook.com/v21.0/{psid}?fields=name,profile_pic&access_token={facebook}` | `name`    | `profile_pic` |
| instagram | `GET https://graph.facebook.com/v21.0/{igsid}?fields=name,username,profile_pic&access_token={instagram}` | `name` (fallback `username`) | `profile_pic` |
| other     | — (returns empty `Profile`)                                                                | —             | —             |

- **No token configured** for the platform → return empty `Profile` immediately
  (no network call). This keeps the test harness — which defaults all tokens to
  `None` (B1 convention) — network-free and green.
- **Timeout:** per-request `.timeout(Duration::from_secs(5))` on the shared client,
  so a slow/down profile API cannot delay the webhook ack beyond ~5s.
- **Parsing:** extract the name/avatar fields; an empty string maps to `None`.
  The JSON-shape parsing is split into a pure helper per family
  (`parse_line_profile`, `parse_meta_profile`) so it is unit-testable without a
  network call.

### 2.2 Placeholder helpers (`webhooks/ingest.rs` or a small shared module)

Centralizes the placeholder strings currently scattered across `handlers.rs`:

```rust
/// The default name used when no real profile is known.
pub fn default_display_name(platform: &str) -> &'static str {
    match platform {
        "line" => "LINE User",
        "facebook" => "Facebook User",
        "instagram" => "Instagram User",
        _ => "Customer",
    }
}

/// True when `name` is absent/empty or still the platform placeholder — i.e. a
/// real profile has not been captured yet.
pub fn is_placeholder_name(platform: &str, name: Option<&str>) -> bool {
    match name.map(str::trim) {
        None | Some("") => true,
        Some(n) => n == default_display_name(platform),
    }
}
```

`webhooks/handlers.rs` is updated to source its `default_display_name`/`default_name`
values from `default_display_name(platform)` instead of inline literals (so LINE,
FB, and IG share one source of truth).

### 2.3 Wiring into inbound (`ingest.rs::ingest_message`)

After `find_or_create_customer` (which still seeds the placeholder name on create),
before inserting the message:

```text
if is_placeholder_name(platform, customer.display_name.as_deref()) {
    let gateway = OutboundGateway::from_config(&state.config);
    let p = gateway.fetch_profile(platform, platform_user_id).await;
    if p.display_name.is_some() || p.avatar_url.is_some() {
        UPDATE customers
           SET display_name = COALESCE($name, display_name),
               avatar_url   = COALESCE($avatar, avatar_url),
               updated_at   = $now
         WHERE id = $customer_id;
        // refresh the in-memory `customer` so the message sender_name + the
        // realtime broadcast use the real name.
    }
}
```

- Synchronous (awaited) within ingest, so the **first** message, the customer
  record, and the realtime `new_message` broadcast all carry the real name/avatar.
- `COALESCE` so a partial profile (e.g. name but no avatar) never nulls an existing
  field.
- The `message.sender_name` written for the inbound row uses the resolved name.

### 2.4 Follow lifecycle (`ingest.rs::handle_line_follow`, the TODO at ~575)

Replace the `TODO(live-platform)` block: when the stored name is a placeholder,
call `gateway.fetch_profile("line", user_id)` and use the returned name/avatar for
the create-or-update (CRD 2818). Failure tolerated → existing/default name, exactly
as today.

### 2.5 Conversation **detail** response (`conversations/store.rs` / `handlers.rs`)

The list builder already emits `avatarUrl` (`store.rs:206`). The detail/summary
builder that the inbox thread header consumes (the `/api/conversations/{id}`
response feeding `Inbox.tsx` `onMetaLoaded`) returns `customerName` but not the
avatar — add `customerAvatarUrl: r.cust_avatar` there so the thread header can
render it.

### 2.6 Frontend rendering

- **`components/Avatar.tsx`** — add an optional `src?: string | null` prop. When
  present and non-empty, render `<img src=... class="cs-av ..." onError={fall back
  to initials}>`; otherwise the current initials + hashed color. `onError` flips a
  local `failed` state so a broken/expired URL degrades to initials.
- **Plumb `avatar_url` through** to the customer `<Avatar>` call sites:
  - Inbox conversation **list** item (uses the list `avatarUrl`).
  - Inbox **thread header** + customer panel (uses the detail `customerAvatarUrl`
    via `onMetaLoaded` → `meta`).
  - `stores/conversations.ts` list item type carries `avatarUrl` through.
  Agent/teammate avatars (Dashboard, sidebar) keep initials — out of scope.

---

## 3. Data flow

```
inbound webhook
  → parse → ingest_message
    → find_or_create_customer (seeds placeholder on create)
    → is_placeholder_name? ── yes ──> gateway.fetch_profile(platform, user_id)
                                         → UPDATE customers.display_name/avatar_url
    → INSERT message (sender_name = resolved name)
    → realtime new_message broadcast (carries real name)
  → conversation list/detail API → avatarUrl / customerAvatarUrl
  → frontend <Avatar src=…> renders photo (initials fallback)
```

---

## 4. Error handling

- `fetch_profile` is **best-effort**: any HTTP error, non-2xx, parse failure,
  missing token, or empty field yields an empty/partial `Profile`. It never returns
  `Err` and never panics.
- Inbound ingest proceeds regardless — on an empty profile the placeholder name is
  kept, identical to today's behavior.
- Per-request 5s timeout bounds the added webhook latency. (LINE/Meta tolerate a
  webhook response within their retry window; 5s is well inside it.)
- A broken/expired avatar URL is handled client-side (`<img onError>` → initials).

---

## 5. Testing

**Unit (network-free):**
- `parse_line_profile` / `parse_meta_profile`: extract name + avatar from a sample
  body; missing/empty fields → `None`; `username` fallback for IG.
- `default_display_name` / `is_placeholder_name`: truth table across
  line/facebook/instagram — `None`, `""`, the exact placeholder, and a real name.
- `fetch_profile` with **no token** returns an empty `Profile` without a network
  call (assert it does not block / errors).

**Integration (existing harness, tokens default to `None`):**
- An inbound LINE message with no configured token still produces a customer named
  `"LINE User"` (the fetch no-ops) — i.e. existing ingest tests stay green.
- A customer whose stored name is the placeholder is **eligible** for re-fetch on
  the next inbound (assert the code path runs; with no token it falls back) — guards
  the placeholder-detection wiring without hitting the network.

Real network calls are **not** exercised in tests (no live api.line.me /
graph.facebook.com). The parse helpers + no-token fallback cover the logic; the
live path is verified manually against the real LINE OA during live testing.

**Frontend:** `Avatar` renders `<img>` when `src` is set and initials when not;
`onError` falls back to initials (vitest + @testing-library/react).

---

## 6. Verification

- `cd backend && cargo build && cargo build --tests && cargo test` — green.
- `cd frontend && npm run build && npx vitest run` — green.
- `impact()` before editing `OutboundGateway` / `ingest_message`;
  `detect_changes()` before each commit. `send_batch` must **not** appear in the
  changed set.
- Manual: a fresh LINE user messages the OA → the inbox shows their real LINE
  display name + photo on the first message (no `"LINE User"` flash).

---

## 7. Resolved decisions

- Fetch is **synchronous** at customer resolution (first message shows the real
  name; accept ~100-300ms on the first-ever message from a user).
- Scope: **LINE + Facebook + Instagram** via one `fetch_profile(platform, …)`.
- Trigger: **new customer OR stored name is a placeholder/empty** (backfills old
  `"LINE User"` records on their next message; no migration).
- **Avatar included** alongside the name (reuses the existing `avatar_url` column;
  frontend `<Avatar src>` with initials fallback).
- Profile fetch lives on **`OutboundGateway`** (reuses tokens + client; `send_batch`
  untouched).
- Best-effort, 5s timeout, no periodic re-fetch, Shopee excluded.

# Platform Profile (Real Name + Avatar) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fetch the real end-user display name + avatar from LINE/Facebook/Instagram when a customer is new or still carries the platform placeholder name, store them, and render the avatar in the inbox.

**Architecture:** Add a best-effort `fetch_profile` method to the existing `OutboundGateway` (reuses its per-platform tokens + shared reqwest client; `send_batch` is untouched). Wire it synchronously into inbound ingest and the LINE follow handler, gated by a placeholder-name check. Reuse the existing `customers.avatar_url` column (no migration). On the frontend, add an optional `src` prop to `Avatar` and plumb the customer avatar URL through the conversation list and thread header.

**Tech Stack:** Rust, axum, sqlx, reqwest; React 18 + TypeScript + Vite; vitest + @testing-library/react.

**Spec:** `docs/superpowers/specs/2026-06-24-platform-profile-name-avatar-design.md`

---

## File Structure

- `backend/src/domain/conversations/channels.rs` — **modify**: add `Profile` struct, pure parse helpers (`parse_line_profile`, `parse_meta_profile`), per-platform fetchers (`line_profile`, `meta_profile`), and `OutboundGateway::fetch_profile`. Unit tests in the existing `gateway_tests` module.
- `backend/src/domain/webhooks/ingest.rs` — **modify**: add `default_display_name` / `is_placeholder_name` helpers; call `fetch_profile` in `ingest_message` and `handle_line_follow`.
- `backend/src/domain/webhooks/handlers.rs` — **modify**: source the placeholder names from `ingest::default_display_name(...)`.
- `backend/src/domain/conversations/store.rs` — **modify**: add `customerAvatarUrl` to the conversation detail JSON.
- `frontend/src/components/Avatar.tsx` — **modify**: optional `src` prop with `<img>` + initials fallback.
- `frontend/src/components/Avatar.test.tsx` — **create**: render tests.
- `frontend/src/pages/Inbox.tsx` — **modify**: plumb avatar URL into the list item + thread header `<Avatar>` calls and `ConvMeta`.

---

## Task 1: Profile fetch on the gateway (`channels.rs`)

**Files:**
- Modify: `backend/src/domain/conversations/channels.rs`

- [ ] **Step 1: Write the failing unit tests**

Add to the `gateway_tests` module (`backend/src/domain/conversations/channels.rs`, the `#[cfg(test)] mod gateway_tests` block near the end):

```rust
    #[test]
    fn parse_line_profile_extracts_name_and_avatar() {
        let v = serde_json::json!({ "displayName": "陳小明", "pictureUrl": "https://p/x.jpg" });
        let p = parse_line_profile(&v);
        assert_eq!(p.display_name.as_deref(), Some("陳小明"));
        assert_eq!(p.avatar_url.as_deref(), Some("https://p/x.jpg"));
    }

    #[test]
    fn parse_line_profile_empty_fields_are_none() {
        let v = serde_json::json!({ "displayName": "", "pictureUrl": "  " });
        let p = parse_line_profile(&v);
        assert_eq!(p, Profile::default());
    }

    #[test]
    fn parse_meta_profile_prefers_name_then_username() {
        let with_name = serde_json::json!({ "name": "Jane", "username": "jane_ig", "profile_pic": "https://p/a.jpg" });
        assert_eq!(parse_meta_profile(&with_name).display_name.as_deref(), Some("Jane"));
        let only_user = serde_json::json!({ "username": "jane_ig" });
        assert_eq!(parse_meta_profile(&only_user).display_name.as_deref(), Some("jane_ig"));
        assert_eq!(parse_meta_profile(&with_name).avatar_url.as_deref(), Some("https://p/a.jpg"));
    }

    #[tokio::test]
    async fn fetch_profile_without_token_is_empty() {
        let mut c = crate::config::test_config();
        c.line_channel_access_token = None;
        c.facebook_page_access_token = None;
        c.instagram_access_token = None;
        let g = OutboundGateway::from_config(&c);
        assert_eq!(g.fetch_profile("line", "U1").await, Profile::default());
        assert_eq!(g.fetch_profile("facebook", "P1").await, Profile::default());
        assert_eq!(g.fetch_profile("instagram", "I1").await, Profile::default());
        assert_eq!(g.fetch_profile("shopee", "S1").await, Profile::default());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd backend && cargo test --lib gateway_tests 2>&1 | tail -15`
Expected: FAIL to compile (`Profile`, `parse_line_profile`, `parse_meta_profile`, `fetch_profile` not found).

- [ ] **Step 3: Implement the struct, parsers, fetchers, and method**

In `backend/src/domain/conversations/channels.rs`, immediately after the `fb_send` function (around line 99, before `pub struct OutboundGateway`), add:

```rust
/// End-user profile from a platform lookup (best-effort; both fields optional).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Profile {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Trimmed non-empty string from a JSON field, else `None`.
fn non_empty(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a LINE `GET /v2/bot/profile/{userId}` body (pure — unit-tested).
pub fn parse_line_profile(v: &serde_json::Value) -> Profile {
    Profile {
        display_name: non_empty(v.get("displayName")),
        avatar_url: non_empty(v.get("pictureUrl")),
    }
}

/// Parse a Meta Graph `?fields=name,username,profile_pic` body (pure — unit-tested).
pub fn parse_meta_profile(v: &serde_json::Value) -> Profile {
    Profile {
        display_name: non_empty(v.get("name")).or_else(|| non_empty(v.get("username"))),
        avatar_url: non_empty(v.get("profile_pic")),
    }
}

async fn line_profile(token: &str, user_id: &str) -> Profile {
    let url = format!("https://api.line.me/v2/bot/profile/{user_id}");
    match http_client()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            parse_line_profile(&resp.json::<serde_json::Value>().await.unwrap_or_else(|_| json!({})))
        }
        _ => Profile::default(),
    }
}

async fn meta_profile(token: &str, user_id: &str) -> Profile {
    let url = format!(
        "https://graph.facebook.com/v21.0/{user_id}?fields=name,username,profile_pic&access_token={token}"
    );
    match http_client()
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            parse_meta_profile(&resp.json::<serde_json::Value>().await.unwrap_or_else(|_| json!({})))
        }
        _ => Profile::default(),
    }
}
```

Then add this method inside `impl OutboundGateway` (after `send_batch`, before the closing `}` of the impl block — do **not** modify `send_batch`):

```rust
    /// Best-effort end-user profile lookup (name + avatar). Returns an empty
    /// `Profile` for an unknown platform, a missing token, an empty user id, or
    /// any network/parse failure — never errors, never panics.
    pub async fn fetch_profile(&self, platform: &str, user_id: &str) -> Profile {
        if user_id.is_empty() {
            return Profile::default();
        }
        match platform {
            "line" => match &self.line {
                Some(t) => line_profile(t, user_id).await,
                None => Profile::default(),
            },
            "facebook" => match &self.facebook {
                Some(t) => meta_profile(t, user_id).await,
                None => Profile::default(),
            },
            "instagram" => match &self.instagram {
                Some(t) => meta_profile(t, user_id).await,
                None => Profile::default(),
            },
            _ => Profile::default(),
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd backend && cargo test --lib gateway_tests 2>&1 | grep "test result"`
Expected: `test result: ok.` (all gateway_tests pass, including the 4 new ones).

Also: `cd backend && cargo build 2>&1 | tail -3` → success.

- [ ] **Step 5: Commit**

```bash
git add backend/src/domain/conversations/channels.rs
git commit -m "feat(channels): best-effort fetch_profile (name+avatar) for line/facebook/instagram"
```

---

## Task 2: Placeholder-name helpers + centralize the literals

**Files:**
- Modify: `backend/src/domain/webhooks/ingest.rs`
- Modify: `backend/src/domain/webhooks/handlers.rs`

- [ ] **Step 1: Write the failing unit tests**

Add a test module at the **end** of `backend/src/domain/webhooks/ingest.rs`:

```rust
#[cfg(test)]
mod placeholder_tests {
    use super::{default_display_name, is_placeholder_name};

    #[test]
    fn defaults_per_platform() {
        assert_eq!(default_display_name("line"), "LINE User");
        assert_eq!(default_display_name("facebook"), "Facebook User");
        assert_eq!(default_display_name("instagram"), "Instagram User");
        assert_eq!(default_display_name("shopee"), "Customer");
    }

    #[test]
    fn placeholder_detection() {
        assert!(is_placeholder_name("line", None));
        assert!(is_placeholder_name("line", Some("")));
        assert!(is_placeholder_name("line", Some("   ")));
        assert!(is_placeholder_name("line", Some("LINE User")));
        assert!(is_placeholder_name("facebook", Some("Facebook User")));
        assert!(!is_placeholder_name("line", Some("陳小明")));
        // A real name that happens to match another platform's placeholder is
        // still real for this platform.
        assert!(!is_placeholder_name("line", Some("Facebook User")));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd backend && cargo test --lib placeholder_tests 2>&1 | tail -15`
Expected: FAIL to compile (`default_display_name`, `is_placeholder_name` not found).

- [ ] **Step 3: Implement the helpers**

Add near the top of `backend/src/domain/webhooks/ingest.rs` (after the imports, before `pub struct InboundMessage`):

```rust
/// The default customer name used until a real profile is captured.
pub fn default_display_name(platform: &str) -> &'static str {
    match platform {
        "line" => "LINE User",
        "facebook" => "Facebook User",
        "instagram" => "Instagram User",
        _ => "Customer",
    }
}

/// True when `name` is absent/blank or still this platform's placeholder — i.e.
/// no real profile has been captured for the customer yet.
pub fn is_placeholder_name(platform: &str, name: Option<&str>) -> bool {
    match name.map(str::trim) {
        None | Some("") => true,
        Some(n) => n == default_display_name(platform),
    }
}
```

- [ ] **Step 4: Use the helper in `handlers.rs`**

In `backend/src/domain/webhooks/handlers.rs`:

- Replace the LINE inbound default (around line 202) `default_display_name: "LINE User",` with:
```rust
                            default_display_name: ingest::default_display_name("line"),
```
- Replace the Meta default tuple (around line 415-419):
```rust
        let (platform, default_name) = if object == "instagram" {
            ("instagram", "Instagram User")
        } else {
            ("facebook", "Facebook User")
        };
```
with:
```rust
        let platform = if object == "instagram" { "instagram" } else { "facebook" };
        let default_name = ingest::default_display_name(platform);
```

- [ ] **Step 5: Run the tests + build**

Run: `cd backend && cargo test --lib placeholder_tests 2>&1 | grep "test result"` → `ok.`
Run: `cd backend && cargo build 2>&1 | tail -3` → success.

- [ ] **Step 6: Commit**

```bash
git add backend/src/domain/webhooks/ingest.rs backend/src/domain/webhooks/handlers.rs
git commit -m "feat(webhooks): centralize platform default names + placeholder detection"
```

---

## Task 3: Fetch the profile during inbound ingest

**Files:**
- Modify: `backend/src/domain/webhooks/ingest.rs`
- Test: `backend/tests/webhooks.rs` (or the existing inbound/LINE test file — see Step 4)

- [ ] **Step 1: Wire the fetch into `ingest_message`**

In `backend/src/domain/webhooks/ingest.rs::ingest_message`, change the customer binding (currently `let customer = find_or_create_customer(...).await?;`, around line 254) to `let mut customer = ...` and insert the profile fetch immediately after it (before the `team_id` resolution / message insert):

```rust
    let mut customer = find_or_create_customer(
        &state.db,
        inbound.platform,
        inbound.platform_user_id,
        inbound.default_display_name,
    )
    .await?;

    // Fill the real name + avatar while we still only have the placeholder
    // (covers brand-new customers and old "<Platform> User" records). Best-effort:
    // a failed/absent profile leaves the placeholder untouched (CRD 2818).
    if is_placeholder_name(inbound.platform, customer.display_name.as_deref()) {
        let gateway =
            crate::domain::conversations::channels::OutboundGateway::from_config(&state.config);
        let profile = gateway.fetch_profile(inbound.platform, inbound.platform_user_id).await;
        if profile.display_name.is_some() || profile.avatar_url.is_some() {
            let _ = sqlx::query(
                "UPDATE customers
                    SET display_name = COALESCE($1, display_name),
                        avatar_url   = COALESCE($2, avatar_url),
                        updated_at   = $3
                  WHERE id = $4",
            )
            .bind(profile.display_name.as_deref())
            .bind(profile.avatar_url.as_deref())
            .bind(now_iso())
            .bind(customer.id)
            .execute(&state.db)
            .await;
            if let Some(name) = profile.display_name {
                customer.display_name = Some(name);
            }
        }
    }
```

The existing message-insert `sender_name` bind (around line 297, `customer.display_name.as_deref().unwrap_or(inbound.default_display_name)`) now picks up the resolved name automatically — leave it as-is.

- [ ] **Step 2: Build to verify it compiles**

Run: `cd backend && cargo build 2>&1 | tail -5`
Expected: success. (If `customer` triggers an "does not need to be mutable" warning, the `mut` is still correct because the `if let Some(name)` branch assigns to it.)

- [ ] **Step 3: Find the inbound integration test + helper shape**

Run: `cd backend && grep -rln "ingest\|/api/webhook\|line_webhook\|platform_user_id" tests/ | head` and open the file that drives an inbound LINE webhook (e.g. `tests/webhooks.rs`). Identify the helper that posts an inbound message and the assertion that reads the created customer/conversation name.

- [ ] **Step 4: Add a regression test (no token → placeholder kept, no network)**

In that inbound test file, add a test asserting that an inbound LINE message — with the harness default of no `line_channel_access_token` — still creates a customer whose name is the placeholder `"LINE User"` (the fetch no-ops, so existing behavior holds and no network call is made). Reuse the file's existing inbound-post helper and customer/name assertion. Concretely, the test posts one inbound LINE text event and asserts the resulting conversation's `customerName` (or the customer row's `display_name`) equals `"LINE User"`.

Example shape (adapt to the file's actual helpers/spawn_app):

```rust
#[tokio::test]
async fn inbound_without_line_token_keeps_placeholder_name() {
    let app = spawn_app().await; // harness defaults line_channel_access_token = None
    // ... post a single inbound LINE text webhook for user "Uplaceholdertest" ...
    // ... fetch the conversation list / customer ...
    assert_eq!(customer_name, "LINE User");
}
```

- [ ] **Step 5: Run the suites**

Run: `cd backend && cargo build --tests 2>&1 | tail -5` → success.
Run: `cd backend && cargo test --test webhooks 2>&1 | grep -E "Running|test result|error\[|FAILED"` → green (use the actual test file name from Step 3).

- [ ] **Step 6: Commit**

```bash
git add backend/src/domain/webhooks/ingest.rs backend/tests/
git commit -m "feat(ingest): fetch real profile name+avatar on inbound when name is a placeholder"
```

---

## Task 4: Fetch the profile on LINE follow

**Files:**
- Modify: `backend/src/domain/webhooks/ingest.rs`

- [ ] **Step 1: Replace the TODO block in `handle_line_follow`**

In `backend/src/domain/webhooks/ingest.rs::handle_line_follow`, replace the existing block (the `TODO(live-platform)` comment plus the `let existing = ...` / `let display_name = ...` lines, around lines 575-583):

```rust
    // TODO(live-platform): fetch the end-user's profile (display name, avatar)
    // from the platform; failure is tolerated and a default or previously
    // captured name is used (CRD 2818).
    let existing = find_customer(&state.db, "line", user_id).await?;
    let display_name = existing
        .as_ref()
        .and_then(|c| c.display_name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "LINE User".into());
```

with:

```rust
    // Capture the real profile (name + avatar) on follow when we still only have
    // a placeholder; failure is tolerated and the previous/default name is used
    // (CRD 2818).
    let existing = find_customer(&state.db, "line", user_id).await?;
    let stored = existing.as_ref().and_then(|c| c.display_name.clone());
    let mut display_name = stored
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| default_display_name("line").to_string());
    let mut avatar_url: Option<String> = None;
    if is_placeholder_name("line", stored.as_deref()) {
        let gateway =
            crate::domain::conversations::channels::OutboundGateway::from_config(&state.config);
        let profile = gateway.fetch_profile("line", user_id).await;
        if let Some(name) = profile.display_name {
            display_name = name;
        }
        avatar_url = profile.avatar_url;
    }
```

- [ ] **Step 2: Persist the avatar in the follow UPDATE**

Still in `handle_line_follow`, the create-then-update writes the name (the `UPDATE customers SET display_name = $1, metadata = ... WHERE id = $4` around lines 619-628). Replace that statement with one that also sets the avatar:

```rust
    let customer = find_or_create_customer(&state.db, "line", user_id, &display_name).await?;
    let mut meta = json!({ "lastFollowedAt": now });
    if routed_via_tracking {
        meta["assignedViaTracking"] = json!(true);
    }
    let _ = sqlx::query(
        "UPDATE customers SET display_name = $1,
                avatar_url = COALESCE($2, avatar_url),
                metadata = (COALESCE(metadata, '{}')::jsonb || $3::jsonb)::text,
                updated_at = $4
         WHERE id = $5",
    )
    .bind(&display_name)
    .bind(avatar_url.as_deref())
    .bind(meta.to_string())
    .bind(&now)
    .bind(customer.id)
    .execute(&state.db)
```

> Note: this re-binds the trailing parameters — verify the original statement's tail (the `.bind(&display_name).bind(meta...).bind(&now).bind(customer.id)` chain and any `.await`/error handling that follows) is replaced consistently with the 5-bind version above, keeping whatever `.await` + `if let Err(...)` handling already wraps it.

- [ ] **Step 3: Build + run the LINE follow tests**

Run: `cd backend && cargo build --tests 2>&1 | tail -5` → success.
Run: `cd backend && grep -rln "follow" tests/ | head` then `cargo test --test <that_file> follow 2>&1 | grep -E "test result|error\[|FAILED"` → green. If no dedicated follow test exists, run the webhooks suite: `cargo test --test webhooks 2>&1 | grep "test result"`.

- [ ] **Step 4: Commit**

```bash
git add backend/src/domain/webhooks/ingest.rs
git commit -m "feat(webhooks): fetch real LINE profile name+avatar on follow"
```

---

## Task 5: Expose the customer avatar on the conversation detail response

**Files:**
- Modify: `backend/src/domain/conversations/store.rs`

- [ ] **Step 1: Add the flattened field**

In `backend/src/domain/conversations/store.rs`, the conversation **detail** JSON builder already emits the flattened `"customerName": r.cust_id.map(|_| customer_name(r)),` (around line 250). Add directly below it:

```rust
        "customerAvatarUrl": r.cust_avatar,
```

(`r.cust_avatar` is already selected — `store.rs:51` `cu.avatar_url AS cust_avatar` — and the list builder already emits it as `avatarUrl` at line 206.)

- [ ] **Step 2: Build + sanity-check the suites**

Run: `cd backend && cargo build 2>&1 | tail -3` → success.
Run: `cd backend && cargo test --test conversations 2>&1 | grep -E "test result|error\[|FAILED"` → green (existing detail tests unaffected; if the file name differs, find it with `grep -rln "customerName" tests/`).

- [ ] **Step 3: Commit**

```bash
git add backend/src/domain/conversations/store.rs
git commit -m "feat(conversations): expose customerAvatarUrl on the detail response"
```

---

## Task 6: `Avatar` component renders a photo when given a `src`

**Files:**
- Modify: `frontend/src/components/Avatar.tsx`
- Create: `frontend/src/components/Avatar.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/Avatar.test.tsx`:

```tsx
import { render } from '@testing-library/react'
import { describe, it, expect } from 'vitest'
import { Avatar } from './Avatar'

describe('Avatar', () => {
  it('renders an <img> when src is provided', () => {
    const { container } = render(<Avatar name="Alice" src="https://cdn/x.png" />)
    const img = container.querySelector('img')
    expect(img).toBeTruthy()
    expect(img?.getAttribute('src')).toBe('https://cdn/x.png')
  })

  it('renders initials (no <img>) when src is absent', () => {
    const { container } = render(<Avatar name="Alice" />)
    expect(container.querySelector('img')).toBeNull()
    expect(container.textContent).toBe('ce') // last-two-chars behaviour
  })

  it('renders initials when src is empty string', () => {
    const { container } = render(<Avatar name="Bob" src="" />)
    expect(container.querySelector('img')).toBeNull()
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/Avatar.test.tsx 2>&1 | tail -15`
Expected: FAIL (the `src`-provided case renders a `<span>`, not an `<img>`).

- [ ] **Step 3: Implement the `src` prop**

Replace the contents of `frontend/src/components/Avatar.tsx` body (keep `AV_COLORS` + `avColor` unchanged) with:

```tsx
// Avatar — round avatar: a photo when `src` is given, else initials with a
// hashed color from AV_COLORS. A broken/expired image falls back to initials.
import { useState } from 'react'

export const AV_COLORS = [
  '#0284c7', '#0d9488', '#7c3aed', '#db2777',
  '#ea580c', '#4f46e5', '#0891b2', '#65a30d',
]

export function avColor(name: string): string {
  let h = 0
  for (const ch of name) h = ((h * 31 + ch.charCodeAt(0)) >>> 0)
  return AV_COLORS[h % AV_COLORS.length]
}

export interface AvatarProps {
  name: string
  src?: string | null
  size?: 'sm' | 'md' | 'lg'
  ring?: boolean
}

export function Avatar({ name, src, size = 'md', ring = false }: AvatarProps) {
  const [failed, setFailed] = useState(false)
  const cls = `cs-av cs-av-${size}${ring ? ' cs-av-ring' : ''}`
  if (src && !failed) {
    return (
      <img
        className={cls}
        src={src}
        alt={name}
        onError={() => setFailed(true)}
        style={{ objectFit: 'cover' }}
      />
    )
  }
  return (
    <span className={cls} style={{ background: avColor(name) }}>
      {name.slice(-2)}
    </span>
  )
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/Avatar.test.tsx 2>&1 | tail -10`
Expected: PASS (3/3).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/Avatar.tsx frontend/src/components/Avatar.test.tsx
git commit -m "feat(frontend): Avatar renders a photo via src with initials fallback"
```

---

## Task 7: Plumb the customer avatar through the inbox

**Files:**
- Modify: `frontend/src/pages/Inbox.tsx`

- [ ] **Step 1: Carry the avatar URL in `ConvMeta` + `onMetaLoaded`**

In `frontend/src/pages/Inbox.tsx`, add an avatar field to the `ConvMeta` interface (the block containing `platformUserId?: string` and `customerName?: string`, around line 64-71):

```tsx
  avatarUrl?: string | null
```

In the meta fetch's `onMetaLoaded({ ... })` call (around line 414-420), the inline response type declares `customerName?: string` (around line 411). Add `customerAvatarUrl?: string` to that inline type, and add this field to the `onMetaLoaded` object:

```tsx
      onMetaLoaded({
        platform: resp.data.platform,
        platformUserId: resp.data.platformUserId,
        teamId: resp.data.teamId ?? null,
        customerId: resp.data.customerId ?? null,
        customerName: resp.data.customerName,
        avatarUrl: resp.data.customerAvatarUrl ?? null,
      })
```

- [ ] **Step 2: Pass `src` to the thread-header avatars**

Still in the `Thread` component, just below `const customerName = meta.customerName ?? ''` (around line 512), add:

```tsx
  const customerAvatarUrl = meta.avatarUrl ?? undefined
```

Then update the two thread `<Avatar name={customerName || '?'} ... />` calls (around lines 532 and 660) to pass `src`:

```tsx
          <Avatar name={customerName || '?'} src={customerAvatarUrl} size="md" />
```
```tsx
                <Avatar name={customerName || '?'} src={customerAvatarUrl} size="sm" />
```

- [ ] **Step 3: Pass `src` to the conversation-list-item avatar**

In the list-item component (`ConvItem`/the block with `const name = conv.customerName ?? conv.id`, around line 125), the `conv` object carries the list response's `avatarUrl` (the store spreads all fields). Update the avatar call (around line 138):

```tsx
        <Avatar name={name} src={conv.avatarUrl as string | undefined} size="md" />
```

- [ ] **Step 4: Pass `src` to the customer-panel avatar**

In the customer-panel component (the block with `const name = customer?.display_name ?? meta.customerName ?? ''`, around line 1038), the panel's `customer` (from the customers store) has `avatar_url`. Update its `<Avatar name={name || '?'} size="lg" ring />` call (around line 1068):

```tsx
        <Avatar name={name || '?'} src={customer?.avatar_url ?? meta.avatarUrl ?? undefined} size="lg" ring />
```

- [ ] **Step 5: Type-check, build, and run the frontend suites**

Run: `cd frontend && npm run build 2>&1 | tail -6`
Expected: `tsc -b` clean + `vite build` success.
Run: `cd frontend && npx vitest run 2>&1 | tail -10`
Expected: green (incl. Task 6's Avatar tests).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/pages/Inbox.tsx
git commit -m "feat(frontend): show customer avatar in conversation list, thread header, and panel"
```

---

## Final verification (after all tasks)

- [ ] `cd backend && cargo build && cargo build --tests` — clean.
- [ ] `cd backend && cargo test 2>&1 | grep -E "test result|error\[" | tail -30` — all suites green (gateway parse/no-token units, placeholder units, the inbound placeholder-kept regression, existing webhooks/conversations).
- [ ] `cd backend && grep -rn '"LINE User"\|"Facebook User"\|"Instagram User"' src/domain/webhooks/handlers.rs` — no inline literals remain (all via `default_display_name`).
- [ ] `cd frontend && npm run build && npx vitest run` — green.
- [ ] `detect_changes()` before the final review — confirm `send_batch` is **not** in the changed set (only `fetch_profile` was added to the gateway).
- [ ] Manual live check against the real LINE OA: a fresh user messages the bot → the inbox shows their real LINE display name + photo on the first message (no `"LINE User"` flash).

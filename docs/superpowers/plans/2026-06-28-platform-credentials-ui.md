# Platform Credentials from the UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an admin enter LINE/Facebook/Instagram credentials in the UI and have the running system use them (one shared set per platform, `.env` as fallback).

**Architecture:** Reuse the existing `channel_integrations` store + `/api/channels` (encrypt via `crypto::protect`, verify via `verify_channel`). Add a `resolve_channel(state, platform)` layer that returns the single active integration's decrypted credentials (or the `.env` value), and wire the outbound gateway, LINE webhook signature check, inbound-media proxy, and profile fetch to use it. The frontend `Channels.tsx` gains per-platform credential forms.

**Tech Stack:** Rust, axum, sqlx; React 18 + TypeScript + Vite; vitest.

**Verification gates (this repo's CI):** every backend task must end green on `cargo test` AND `cargo clippy --all-targets -- -D warnings`; frontend tasks on `npm run build` + `npx vitest run` (keep `package-lock.json` in sync).

---

## File Structure

- `backend/src/domain/channels/handlers.rs` — **modify**: `PLATFORMS` + `platform_fields` (add `instagram`, optional `liffId` for line); validation handles optional plain fields.
- `backend/src/domain/channels/store.rs` — **modify**: `find_active_by_platform`; ensure `view` does not leak secrets + expose which secrets are set.
- `backend/src/domain/channels/resolve.rs` — **create**: `ResolvedChannel` + `resolve_channel(state, platform)` (DB → `.env`).
- `backend/src/domain/channels/mod.rs` — **modify**: `pub mod resolve;`.
- `backend/src/domain/conversations/channels.rs` — **modify**: `OutboundGateway::resolve(state)`.
- Runtime gateway call sites — **modify**: use `resolve(&state).await`.
- `backend/src/domain/webhooks/handlers.rs` — **modify**: LINE secret via resolver.
- `backend/src/domain/conversations/handlers.rs` — **modify**: media-proxy token via resolver.
- `backend/tests/channels.rs` — **modify**: IG + optional liffId + resolver precedence.
- `frontend/src/pages/Channels.tsx` — **modify**: per-platform credential forms.

---

## Task 1: Channels schema — add Instagram + optional LINE `liffId`, keep secrets out of responses

**Files:**
- Modify: `backend/src/domain/channels/handlers.rs`
- Modify: `backend/src/domain/channels/store.rs`
- Test: `backend/tests/channels.rs`

- [ ] **Step 1: Extend `PLATFORMS` + `platform_fields` (4-tuple with optional plain)**

In `backend/src/domain/channels/handlers.rs`:
- Change `pub const PLATFORMS: [&str; 3] = ["line", "facebook", "whatsapp"];` to:
```rust
pub const PLATFORMS: [&str; 4] = ["line", "facebook", "instagram", "whatsapp"];
```
- Change `platform_fields` to return a 4-tuple `(config_key, plain, optional_plain, secret)`:
```rust
fn platform_fields(
    platform: &str,
) -> (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
) {
    match platform {
        "line" => ("lineConfig", &["channelId"], &["liffId"], &["channelAccessToken", "channelSecret"]),
        "facebook" => ("facebookConfig", &["pageId"], &[], &["accessToken", "appSecret"]),
        "instagram" => ("instagramConfig", &["igId"], &[], &["accessToken"]),
        "whatsapp" => ("whatsappConfig", &["phoneNumber", "businessAccountId"], &[], &["accessToken"]),
        _ => ("config", &[], &[], &[]),
    }
}
```
- In `create_channel` and `update_channel`, update the destructuring and the required-field check. `create_channel` currently does `let (config_key, plain_fields, secret_fields) = platform_fields(&platform);` and validates `plain_fields.iter().chain(secret_fields.iter())`. Change to:
```rust
    let (config_key, plain_fields, optional_plain, secret_fields) = platform_fields(&platform);
```
Keep the **required** check over `plain_fields.iter().chain(secret_fields.iter())` (unchanged — optional fields are NOT required). When building the stored `config` map, also copy any **present** optional plain field:
```rust
    let mut config = Map::new();
    for field in plain_fields {
        config.insert((*field).to_string(), supplied[*field].clone());
    }
    for field in optional_plain {
        if let Some(v) = supplied.get(*field).filter(|v| v.as_str().is_some_and(|s| !s.trim().is_empty())) {
            config.insert((*field).to_string(), v.clone());
        }
    }
```
Apply the analogous 4-tuple destructure + optional-plain handling in `update_channel` (it also calls `platform_fields`). Also update the two hard-coded error strings that say `"Supported platforms: line, facebook, whatsapp"` to `"line, facebook, instagram, whatsapp"`.

- [ ] **Step 2: Keep secrets out of `view`; expose which secrets are set**

In `backend/src/domain/channels/store.rs`, read `pub fn view(row: &ChannelRow) -> Value`. Confirm it does NOT include the raw `credentials` blob. Add a `credentialsSet` object listing which secret fields have a stored value (so the form shows "•••• set" without revealing the secret). Inside `view`, parse `row.credentials` JSON and emit the KEYS only:
```rust
    let creds_set: Vec<String> = row
        .credentials
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default();
```
and add `"credentialsSet": creds_set` to the returned JSON. Do NOT decrypt or return any secret value.

- [ ] **Step 3: Tests**

In `backend/tests/channels.rs` (study the existing create/list test helpers + admin auth), add:
- `create_channel` with `{platform:"instagram", instagramConfig:{igId:"IG1", accessToken:"tok"}}` → 201/200 success; the listed channel has `platform:"instagram"` and `credentialsSet` containing `"accessToken"` (and NO `accessToken` value field).
- `create_channel` with `{platform:"line", lineConfig:{channelId:"C1", channelAccessToken:"at", channelSecret:"sec", liffId:"liff-1"}}` → success; the stored `config` exposes `liffId:"liff-1"` and `credentialsSet` has `channelAccessToken`+`channelSecret`.
- `create_channel` line WITHOUT `liffId` still succeeds (optional).

- [ ] **Step 4: Build + clippy + tests**

- `cd backend && cargo build 2>&1 | tail -3` → success.
- `cd backend && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3` → clean.
- `cd backend && cargo test --test channels 2>&1 | grep -E "test result|error\[|FAILED"` → green.

- [ ] **Step 5: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add backend/src/domain/channels backend/tests/channels.rs
git commit -m "feat(channels): support instagram + optional LINE liffId; expose credentialsSet"
```

---

## Task 2: Credential resolution layer (DB → .env)

**Files:**
- Create: `backend/src/domain/channels/resolve.rs`
- Modify: `backend/src/domain/channels/mod.rs`
- Modify: `backend/src/domain/channels/store.rs`
- Test: `backend/tests/channels.rs`

- [ ] **Step 1: Add `find_active_by_platform` to the store**

In `backend/src/domain/channels/store.rs`, add (reusing the existing `SELECT` const + `ChannelRow`):
```rust
/// The single active integration for a platform (the shared system credential).
pub async fn find_active_by_platform(
    db: &PgPool,
    platform: &str,
) -> Result<Option<ChannelRow>, AppError> {
    let sql = format!("{SELECT} WHERE platform = $1 AND is_active = 1 ORDER BY updated_at DESC NULLS LAST LIMIT 1");
    sqlx::query_as::<_, ChannelRow>(&sql)
        .bind(platform)
        .fetch_optional(db)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}
```
(Match the error variant the file's other store fns use — copy their `.map_err(...)` form.)

- [ ] **Step 2: Create the resolver + write its tests**

Create `backend/src/domain/channels/resolve.rs`:
```rust
//! Resolve the live credentials for a platform: the single active channel
//! integration (decrypted), falling back to `.env`/config. Never panics.

use serde_json::{Map, Value};

use crate::state::AppState;
use super::store;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedChannel {
    pub access_token: Option<String>,
    pub secret: Option<String>,
    pub config: Map<String, Value>, // plain fields: channelId / liffId / pageId / igId
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

pub async fn resolve_channel(state: &AppState, platform: &str) -> ResolvedChannel {
    let mut access_token = None;
    let mut secret = None;
    let mut config = Map::new();

    if let Ok(Some(row)) = store::find_active_by_platform(&state.db, platform).await {
        config = row
            .config
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        if let Ok(creds) = store::decrypt_credentials(state.config.encryption_key.as_deref(), &row.credentials) {
            let get = |k: &str| creds.get(k).and_then(Value::as_str).map(str::to_string);
            match platform {
                "line" => { access_token = get("channelAccessToken"); secret = get("channelSecret"); }
                "facebook" => { access_token = get("accessToken"); secret = get("appSecret"); }
                "instagram" => { access_token = get("accessToken"); }
                _ => {}
            }
        }
    }

    // Fall back to .env / config for anything the DB did not provide.
    let cfg = &state.config;
    let access_token = non_empty(access_token).or_else(|| match platform {
        "line" => cfg.line_channel_access_token.clone(),
        "facebook" => cfg.facebook_page_access_token.clone(),
        "instagram" => cfg
            .instagram_access_token
            .clone()
            .or_else(|| cfg.facebook_page_access_token.clone()),
        _ => None,
    });
    let secret = non_empty(secret).or_else(|| match platform {
        "line" => cfg.line_channel_secret.clone(),
        _ => None,
    });

    ResolvedChannel { access_token: non_empty(access_token), secret: non_empty(secret), config }
}
```
Register it in `backend/src/domain/channels/mod.rs`: add `pub mod resolve;`.

Add an integration test to `backend/tests/channels.rs` (uses the harness app + DB; the harness defaults `.env` LINE token to `None`):
- Seed nothing → `resolve_channel(state, "line")` access_token/secret match the harness config (likely `None`). (If the resolver is not directly reachable from the integration test, instead assert via behavior in Task 3/4; in that case make this a `#[cfg(test)]` unit test inside `resolve.rs` using a `test_config()` AppState if one is constructible — otherwise rely on the Task 3 gateway test. Pick whichever the harness supports; do not invent a harness.)

- [ ] **Step 3: Build + clippy + tests**

- `cd backend && cargo build 2>&1 | tail -3` → success.
- `cd backend && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3` → clean.
- `cd backend && cargo test --test channels 2>&1 | grep -E "test result|error\[|FAILED"` → green.

- [ ] **Step 4: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add backend/src/domain/channels
git commit -m "feat(channels): resolve_channel layer (DB credentials with .env fallback)"
```

---

## Task 3: Wire the outbound gateway to the resolver (CRITICAL hub)

**Files:**
- Modify: `backend/src/domain/conversations/channels.rs`
- Modify: gateway runtime call sites (compiler/grep-enumerated)
- Test: `backend/tests/channels.rs` or `backend/tests/conversations.rs`

- [ ] **Step 1: Add `OutboundGateway::resolve`**

In `backend/src/domain/conversations/channels.rs`, add to `impl OutboundGateway` (do NOT change `from_config`, `send_batch`, or `build_push_body` signatures):
```rust
    /// Build the gateway from the live resolved credentials (DB → .env) for each
    /// platform. Used by runtime send paths; `from_config` remains for tests.
    pub async fn resolve(state: &crate::state::AppState) -> Self {
        use crate::domain::channels::resolve::resolve_channel;
        Self {
            line: resolve_channel(state, "line").await.access_token,
            facebook: resolve_channel(state, "facebook").await.access_token,
            instagram: resolve_channel(state, "instagram").await.access_token,
        }
    }
```
(`resolve_channel` already applies the IG→FB fallback and empties filter, so the fields match `from_config`'s shape.)

- [ ] **Step 2: Convert runtime construction sites**

Run `cd backend && grep -rn "OutboundGateway::from_config(&state.config)\|Self::from_config(&state.config)" src/` to list the **runtime** sites (NOT the `#[cfg(test)]` ones in `channels.rs` that use a local `c`/`test_config`). Known runtime sites: `conversations/channels.rs` (the deliver path), `conversations/handlers.rs` (`send_message` spawn), `realtime/customer.rs`, `customer_conversations/handlers.rs`, and the profile-fetch sites in `webhooks/ingest.rs` (`ingest_message` + `handle_line_follow`). Convert each from
`OutboundGateway::from_config(&state.config)` (or `Self::from_config(&state.config)`) to
`OutboundGateway::resolve(&state).await` (or `Self::resolve(state).await` where `state` is `&AppState`). Each site is already in an `async fn` with `state` in scope. Leave every `#[cfg(test)]` `from_config(&c)` untouched.

- [ ] **Step 3: Build + clippy**

- `cd backend && cargo build 2>&1 | tail -3` → success.
- `cd backend && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3` → clean.

- [ ] **Step 4: Test — gateway prefers DB token**

In `backend/tests/channels.rs` (or `conversations.rs`), add an integration test: seed an active LINE `channel_integrations` row whose encrypted `channelAccessToken` is a known value (insert via the create endpoint OR a direct insert using `crypto::protect` with the harness `encryption_key`), then assert `OutboundGateway::resolve(&state).await` yields that token as `line` (add a `#[cfg(test)]`-visible accessor or assert via a thin wrapper — if the field is private, compare behavior by checking that `resolve` for a no-DB platform falls back to config). Keep the test network-free (do not actually push to LINE). If exposing the private `line` field is awkward, assert through `resolve_channel(state,"line").await.access_token` instead, which is public.

- [ ] **Step 5: Full suite + commit**

- `cd backend && cargo test 2>&1 | grep -E "test result|error\[" | tail -20` → all green (text-only callers unchanged).
```bash
cd /Users/kkllzz_0/support-center
git add backend/src
git commit -m "feat(channels): outbound gateway resolves live credentials (DB → .env)"
```

---

## Task 4: Wire the LINE webhook secret + inbound-media token to the resolver

**Files:**
- Modify: `backend/src/domain/webhooks/handlers.rs`
- Modify: `backend/src/domain/conversations/handlers.rs`
- Test: `backend/tests/webhooks.rs`

- [ ] **Step 1: Webhook signature secret from the resolver**

In `backend/src/domain/webhooks/handlers.rs`, the LINE handler currently does
`let Some(secret) = state.config.line_channel_secret.as_deref() else { … }` (~line 300). Replace with the resolved secret (DB → `.env`):
```rust
    let resolved_secret = crate::domain::channels::resolve::resolve_channel(&state, "line").await.secret;
    let Some(secret) = resolved_secret.as_deref() else {
```
(keep the existing `else { … }` rejection branch unchanged.)

- [ ] **Step 2: Media-proxy token from the resolver**

In `backend/src/domain/conversations/handlers.rs`, the media proxy resolves the LINE token from `state.config.line_channel_access_token` (~line 703, inside `proxy_media_inner`). Replace that resolution so it uses the live credential:
```rust
    let token = crate::domain::channels::resolve::resolve_channel(&state, "line")
        .await
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::NotFound("Media unavailable".into()))?;
```
(remove the old `state.config.line_channel_access_token.clone()...` resolution it replaces; keep the rest of `proxy_media_inner` unchanged.)

- [ ] **Step 3: Build + clippy + tests**

- `cd backend && cargo build 2>&1 | tail -3` → success.
- `cd backend && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3` → clean.
- Add/extend a `backend/tests/webhooks.rs` test: with an active LINE integration seeded whose `channelSecret` is known, an inbound LINE webhook signed with that secret is accepted (and one signed with a wrong secret is rejected). Reuse the file's existing LINE-signature helpers; sign with the DB secret. If seeding the encrypted secret is awkward, assert the existing `.env`-secret path still works (no regression) and cover the DB path via `resolve_channel` from Task 2.
- `cd backend && cargo test --test webhooks 2>&1 | grep -E "test result|error\[|FAILED"` → green.

- [ ] **Step 4: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add backend/src backend/tests/webhooks.rs
git commit -m "feat(channels): LINE webhook secret + media proxy token use resolved credentials"
```

---

## Task 5: Frontend — credential forms on `Channels.tsx`

**Files:**
- Modify: `frontend/src/pages/Channels.tsx`
- Create: `frontend/src/pages/Channels.test.tsx`

- [ ] **Step 1: Failing test**

Create `frontend/src/pages/Channels.test.tsx` that renders the page (mock `get` to return an empty channel list and `post`), opens the LINE form, fills the fields, submits, and asserts `post` was called with `'/api/channels'` and a body `{ platform: 'line', lineConfig: { channelId, channelAccessToken, channelSecret } }`. Mirror the mocking style of the other page tests (e.g. how `MessageMedia`/store tests mock `../api/client`). Run `cd frontend && npx vitest run src/pages/Channels.test.tsx 2>&1 | tail -10` → FAIL.

- [ ] **Step 2: Build the per-platform forms**

Rework `frontend/src/pages/Channels.tsx` from a read-only list into a manage screen. Keep the existing load + verify. For each platform in `['line','facebook','instagram']`, render a card with:
- the field inputs from this descriptor (secret fields are `type="password"`, never pre-filled with a real value; if the loaded channel's `credentialsSet` includes the field, show placeholder `已設定 ••••`):
```ts
const PLATFORM_FORMS = {
  line: { key: 'lineConfig', plain: [['channelId','Channel ID'],['liffId','LIFF ID（選填）']], secret: [['channelAccessToken','Channel access token'],['channelSecret','Channel secret']] },
  facebook: { key: 'facebookConfig', plain: [['pageId','Page ID']], secret: [['accessToken','Page access token'],['appSecret','App secret']] },
  instagram: { key: 'instagramConfig', plain: [['igId','IG ID']], secret: [['accessToken','Access token']] },
} as const
```
- a **Save** button → if a channel of that platform already exists in the loaded list, `PUT /api/channels/{id}` else `POST /api/channels`, body `{ platform, [key]: { ...filled fields } }` (omit empty secret fields on update so a blank field doesn't overwrite a stored secret). On success, reload + toast.
- the existing **驗證** button (calls `/api/channels/{id}/verify`) shown when the channel exists.
- a read-only **Webhook 路徑** hint for LINE: show the literal path `/api/webhook` with a note "接在你的公開後端網址後（例如 `https://<your-tunnel-or-domain>/api/webhook`），填入 LINE 後台 Webhook URL"。Do NOT use `window.location.origin` (in dev that is the Vite host, which LINE cannot reach). A copy button may copy just the `/api/webhook` path.
- Keep the page admin-gated exactly as today (`can(...)` / `area: 'system'`).

- [ ] **Step 3: Test passes + build + suite**

- `cd frontend && npx vitest run src/pages/Channels.test.tsx 2>&1 | tail -8` → PASS.
- `cd frontend && npm run build 2>&1 | tail -4` → tsc clean + vite success.
- `cd frontend && npx vitest run 2>&1 | tail -6` → green.

- [ ] **Step 4: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/pages/Channels.tsx frontend/src/pages/Channels.test.tsx
git commit -m "feat(frontend): credential-entry forms for LINE/FB/IG on the channels page"
```

---

## Final verification (after all tasks)

- [ ] `cd backend && cargo build && cargo build --tests && cargo test && cargo clippy --all-targets -- -D warnings` — all clean/green.
- [ ] `cd frontend && npm run build && npx vitest run` — green; `npm ci` in sync.
- [ ] `cd backend && grep -rn "OutboundGateway::from_config(&state.config)\|state.config.line_channel_secret\|state.config.line_channel_access_token" src/` — only `#[cfg(test)]` / config-default sites remain (runtime paths go through the resolver).
- [ ] `detect_changes()` before final review — confirm `send_batch`/`build_push_body` signatures unchanged (only gateway construction + credential sources changed).
- [ ] Manual live (LINE OA): in the channels page, enter the LINE channelId/accessToken/secret, Save, Verify succeeds; remove the LINE values from `.env` and restart — inbound (webhook signature), outbound (send), media proxy, and profile still work using the DB credentials.

> **Note (deferred):** the spec mentioned a short TTL cache over `resolve_channel`. This plan resolves with a direct (indexed) query per send/webhook — acceptable at support-inbox volume — and leaves the cache as a follow-up optimization. `verify_channel` is unchanged; it already re-checks platform credentials on demand.

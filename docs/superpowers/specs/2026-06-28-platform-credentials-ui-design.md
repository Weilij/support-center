# Platform Credentials from the UI — Design Spec

**Date:** 2026-06-28
**Track:** channel integrations (backend credential resolution + admin UI)
**Status:** design approved, pending written-spec review

---

## 0. Context

Platform credentials (LINE channel access token / secret, FB page token, etc.) are
read from **`.env` / `config`** at runtime: the outbound gateway
(`OutboundGateway::from_config(&state.config)`), the LINE webhook signature check
(`state.config.line_channel_secret`), the inbound-media proxy
(`config.line_channel_access_token`), and the profile fetch (via the gateway). To
change credentials today you edit `.env` and restart.

There is already a per-team store and admin API for channel integrations that is
**disconnected from this runtime path**:
- `channel_integrations` table (`team_id`, `platform`, `config`, encrypted
  `credentials`, `webhook_config`, `is_active`, `is_verified`).
- `/api/channels` handlers: `create_channel` / `update_channel` / `list_channels` /
  `verify_channel`. `create_channel` validates per-platform fields via
  `platform_fields(platform)`, encrypts secrets with `crypto::protect(key, …)`, and
  `verify_channel` calls the platform API. `PLATFORMS = ["line","facebook","whatsapp"]`.
- Frontend `Channels.tsx` is a **read-only** list (platform + active/verified +
  error count + a "驗證" button) — no credential-entry form.

**Goal of this work:** let an admin enter each platform's credentials in the UI and
have the running system actually use them — one shared credential set per platform
(one LINE OA, one FB page, one IG account, system-wide).

---

## 1. Goal & non-goals

**Goal:** A credential-entry UI for **LINE / Facebook / Instagram**, stored encrypted
via the existing `/api/channels` path; and a backend **resolution layer** so the
gateway, webhook signature check, media proxy, and profile fetch use the single
active DB integration's credentials, falling back to `.env` when none is configured.

**Non-goals:**
- **No multi-channel / per-team routing.** One shared credential set per platform
  (the single active integration); the LINE webhook stays the global `/api/webhook`,
  only its **secret source** changes (DB → `.env`). The per-connection webhook URLs
  that `create_channel` generates are not adopted here.
- **No WhatsApp / Shopee** in the UI (Shopee keeps its own OAuth flow).
- **No new credential table / encryption scheme** — reuse `channel_integrations` +
  `crypto::protect`/`decrypt_credentials`.
- **No removal of `.env`** — it remains the bootstrap fallback.

---

## 2. Backend — extend the channels schema (LINE LIFF + Instagram)

In `backend/src/domain/channels/handlers.rs`:
- Add `"instagram"` to `PLATFORMS` (→ `["line","facebook","instagram","whatsapp"]`).
- Extend `platform_fields`:
  - `instagram` → body key `"instagramConfig"`, plain `["igId"]`, secret `["accessToken"]`.
  - `line` → add optional plain `liffId`. Because `platform_fields` requires every
    listed field, introduce a third "optional plain" slot (or a small change to the
    create/update validation) so `liffId` is stored when supplied but not required.
    LINE required stays `channelId` (plain) + `channelAccessToken`/`channelSecret`
    (secret); `liffId` is optional plain.

Existing `create_channel`/`update_channel` then handle IG and the optional LINE
`liffId` with no other change (they iterate `platform_fields`, encrypt secrets,
store plain config).

---

## 3. Backend — credential resolution layer (the core)

A new resolver reads the **single active integration** for a platform and decrypts
its credentials, falling back to `config`.

```text
resolve_channel(state, platform) -> ResolvedChannel {
    access_token: Option<String>,   // line channelAccessToken / fb,ig accessToken
    secret:       Option<String>,   // line channelSecret / fb appSecret
    config:       Map,              // plain fields: channelId / pageId / igId / liffId
}
  1. SELECT … FROM channel_integrations
     WHERE platform = $1 AND is_active = 1
     ORDER BY updated_at DESC LIMIT 1        -- the one shared integration
  2. If found: decrypt_credentials(state.config.encryption_key, row.credentials)
     → access_token/secret; parse row.config for plain fields.
  3. Fall back to `config` (.env) for any field the DB did not provide:
     line → config.line_channel_access_token / line_channel_secret,
     facebook → config.facebook_page_access_token,
     instagram → config.instagram_access_token (∥ facebook fallback, as today).
```

Wiring (each currently reads `.env`):
- **`OutboundGateway`:** add `async fn resolve(state) -> OutboundGateway` that builds
  the `{line, facebook, instagram}` tokens from `resolve_channel` per platform
  (DB → config). Replace the **runtime** `OutboundGateway::from_config(&state.config)`
  call sites (`conversations/channels.rs`, `realtime/customer.rs`,
  `customer_conversations/handlers.rs`) with `OutboundGateway::resolve(&state).await`.
  `from_config` stays for tests / pure config use. Signatures of `send_batch`/
  `build_push_body` are **unchanged** (CRITICAL hub — only construction changes).
- **Webhook signature:** the LINE handler's `state.config.line_channel_secret`
  becomes `resolve_channel(state,"line").await.secret` (DB → `.env`).
- **Media proxy** (`proxy_media`/`fetch_line_media`): the
  `config.line_channel_access_token` becomes the resolved LINE access token.
- **Profile fetch** already goes through the gateway, so it inherits DB creds once
  the gateway resolves.

A short in-process cache (e.g. 30–60 s TTL keyed by platform) avoids a DB read on
every send/webhook; on `create_channel`/`update_channel`/`verify_channel` the cache
is invalidated so new credentials take effect promptly.

---

## 4. Frontend — credential-entry forms (`Channels.tsx`)

Turn the read-only list into a manage screen. For each of **LINE / Facebook /
Instagram**, show a card with its current status (active / verified / last error)
and an **edit form** whose fields mirror `platform_fields`:

| platform | fields (secret marked 🔒) |
|----------|----------------------------|
| LINE | `channelId`, `liffId` (optional), `channelAccessToken` 🔒, `channelSecret` 🔒 |
| Facebook | `pageId`, `accessToken` 🔒, `appSecret` 🔒 |
| Instagram | `igId`, `accessToken` 🔒 |

- **Save:** `POST /api/channels` (create) or `PUT /api/channels/{id}` (update) with the
  platform's `{platform, <platform>Config: {…fields…}}` body shape the backend expects.
- Secret fields render as password inputs and are **write-only** (never echoed back —
  the list/detail response must not return decrypted secrets; show a "•••• set"
  placeholder when a secret already exists).
- **Verify** button → `POST /api/channels/{id}/verify` (existing), showing the result.
- Show the **webhook URL** the admin must paste into the platform console — for LINE
  that is `{backend_url}/api/webhook` (the global webhook), displayed read-only with a
  copy affordance.
- Admin-gated (the page is already `area: 'system'`).

---

## 5. Error handling

- Resolver: a missing/disabled integration, a decrypt failure, or a missing field →
  fall back to `config` (.env); if `.env` is also empty, the existing behavior holds
  (LINE outbound stub success, FB/IG "not supported", webhook 401 on absent secret).
  The resolver never panics.
- Save: backend already validates required fields (400 on missing) and enforces "one
  active integration per (team, platform)". The form surfaces these messages.
- Verify failure: shown inline; does not block saving.

---

## 6. Testing

**Backend:**
- `resolve_channel` precedence (unit/integration): with an active LINE integration
  seeded → returns its decrypted token/secret; with none → returns the `.env` values;
  decrypt failure → `.env` fallback.
- `create_channel` accepts `instagram` (igId + accessToken) and a LINE body with an
  optional `liffId` (extend `tests/channels.rs`).
- Gateway: `OutboundGateway::resolve` prefers a seeded DB token over config (the no-DB
  path still matches `from_config`). Webhook signature verifies against a DB-seeded
  secret.

**Frontend (vitest):** the LINE form submits the `{platform:"line", lineConfig:{…}}`
shape to `/api/channels`; secret fields are password-type and not pre-filled with the
real value.

---

## 7. Verification

- `cd backend && cargo build && cargo build --tests && cargo test && cargo clippy --all-targets -- -D warnings` — all clean.
- `cd frontend && npm run build && npx vitest run` — green; `npm ci` stays in sync.
- `impact()` on `OutboundGateway`/`from_config`/`send_batch` and the webhook handler before editing (CRITICAL hub — additive construction change). `detect_changes()` before commits.
- Manual live (LINE OA): enter the LINE credentials in the UI, Verify succeeds; with `.env` LINE values removed, inbound + outbound still work using the DB credentials.

---

## 8. Resolved decisions

- **One shared credential set per platform**, system-wide (single active integration);
  no per-team / per-channel routing.
- Reuse `channel_integrations` + `/api/channels` + `crypto` + `verify_channel`; add
  `instagram` to the channels module and an **optional** `liffId` to LINE.
- Runtime **resolves DB credentials with `.env` fallback** for the gateway, webhook
  secret, media proxy, and profile; short TTL cache invalidated on save/verify.
- Webhook stays the global `/api/webhook`; only the **secret source** moves to DB.
- Secrets are write-only in the UI (never returned decrypted).
- Platforms this round: **LINE / Facebook / Instagram** (LINE live-tested; FB/IG wired,
  credentials entered/tested later). WhatsApp/Shopee out of scope.

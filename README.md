# Multi-Channel Customer Support System (MCSS)

A clean-room re-implementation of a multi-channel customer support platform
(LINE Official Account, Facebook Messenger, Instagram Messaging, and Shopee
foundation), built entirely from the behavioral specification in
[`Rust_CRD.md`](Rust_CRD.md).

## Architecture

| Component | Stack | Path |
|---|---|---|
| Backend API | Rust · axum 0.8 · sqlx/PostgreSQL · JWT · argon2 · AES-256-GCM | `backend/` |
| Frontend SPA | Vite · React 18 · TypeScript | `frontend/` |
| Installer | Rust (standalone binary) | `backend/src/bin/installer.rs` |

**Backend** — 23 domain modules covering the full spec surface: auth with
refresh rotation + reuse detection, conversations/messaging with async
delivery, teams/agents/customers/tags, channel integrations with encrypted
credentials, LINE/Facebook/Instagram webhook ingestion, real outbound gateway
dispatch for LINE, Facebook, and Instagram when platform tokens are configured,
LINE media fetch/proxy, auto-reply engine, delayed messages, file management
with signed URLs, WebSocket realtime (rooms, broadcasts, presence,
collaboration), background job queue with retries + dead-letter,
notifications/reminders/alerting, monitoring + circuit breaker,
analytics/dashboards, reports + scheduling, system administration.
**545 tests (integration + installer); clippy-clean.**

**Frontend** — shared API client (single-flight token refresh, guarded login
redirect, backoff retries), optimistic-update store layer, realtime client
with reconnection, route guards, and screens for every documented
destination, plus the installer setup wizard at `/install`.

## Running

Requires a running PostgreSQL server. The backend connects to
`DATABASE_URL` (default `postgres://localhost/mcss`) and applies its
migrations automatically:

```bash
createdb mcss                     # once
# Backend (port 3000)
cd backend && cargo run
# Frontend dev server (proxies /api to :3000)
cd frontend && npm install && npm run dev
# Installer service (port 8976)
cd backend && cargo run --bin installer
```

Key environment variables (see `backend/src/config.rs`): `DATABASE_URL`,
`JWT_SECRET`, `ENCRYPTION_KEY` (32-byte hex, enables credential encryption at
rest), `LINE_CHANNEL_SECRET`, `LINE_CHANNEL_ACCESS_TOKEN`, `LINE_BOT_ID`,
`FACEBOOK_APP_SECRET` or `FB_APP_SECRET`, `FACEBOOK_VERIFY_TOKEN`,
`FACEBOOK_PAGE_ACCESS_TOKEN`, `INSTAGRAM_ACCESS_TOKEN`, `LIFF_ID`,
`FRONTEND_URL`, `BACKEND_URL`, `PUBLIC_STORAGE_URL`, `FILE_SIGNING_SECRET`,
`SHOPEE_PARTNER_ID`, `SHOPEE_PARTNER_KEY`, `SHOPEE_HOST`.

Bootstrap demo data (admin account, team, sample conversations):

```bash
cd backend && cargo run --example seed
# sign in with admin@example.com / admin123
```

The seeder is idempotent. All subsequent administration happens through the
API/UI (or the installer wizard).

Frontend unit tests: `cd frontend && npm test` (vitest).

### Docker

```bash
printf 'JWT_SECRET=change-me\nPOSTGRES_PASSWORD=change-me-too\n' > .env
docker compose up --build   # frontend on :8080, backend + postgres internal
```

Runtime-verified: both images build, the backend reports healthy through
the nginx proxy, and the SPA (including fallback routes) serves on :8080.

## Testing

Backend tests create a throwaway PostgreSQL database per test
(`mcss_test_*`) via `TEST_DATABASE_ADMIN_URL`
(default `postgres://localhost/postgres`):

```bash
cd backend && cargo test          # 545 tests (integration + installer)
cd backend && cargo clippy --all-targets
cd frontend && npm run build      # type-check + bundle
```

## Deliberate boundaries

External integrations that still require real credentials/infrastructure are
kept at clearly marked boundaries:

- Platform tokens are optional in dev/test. With tokens configured, outbound
  dispatch uses the real LINE Push API and Meta Send API for Facebook and
  Instagram. Without tokens, LINE keeps the documented no-network stub success
  and other platforms report unsupported delivery.
- LINE inbound media download is implemented through the channel token and an
  authenticated media proxy. Other platform media handling currently falls back
  to stored/proxied URLs or text link delivery where applicable.
- Shopee currently has the Open Platform foundation plus first messaging
  support: signed requests, OAuth token exchange, encrypted per-shop token
  storage, refresh-before-expiry, callback wiring, signature-gated Webchat push
  ingestion, and SellerChat text outbound using shop-scoped tokens. Richer
  Shopee media/chat surfaces remain future integration work.
- `TODO(cloud)` — installer's real cloud-provider provisioning.
- Realtime customer-channel events now fan out across backend instances through
  Postgres-backed relay/ack tables. Broader room/presence scale-out remains a
  future hardening area if the deployment needs every realtime surface to span
  multiple instances.

Everything else — including every documented status code, envelope shape,
authorization rule, and side effect — is implemented per the CRD, with the
build history and per-section progress recorded in
[`docs/plans/PROGRESS.md`](docs/plans/PROGRESS.md).

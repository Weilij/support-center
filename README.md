# Multi-Channel Customer Support System (MCSS)

A clean-room re-implementation of a multi-channel customer support platform
(LINE Official Account + Facebook Messenger), built entirely from the
behavioral specification in [`Rust_CRD.md`](Rust_CRD.md).

## Architecture

| Component | Stack | Path |
|---|---|---|
| Backend API | Rust · axum 0.8 · sqlx/SQLite · JWT · argon2 · AES-256-GCM | `backend/` |
| Frontend SPA | Vite · React 18 · TypeScript | `frontend/` |
| Installer | Rust (standalone binary) | `backend/src/bin/installer.rs` |

**Backend** — 23 domain modules covering the full spec surface: auth with
refresh rotation + reuse detection, conversations/messaging with async
delivery, teams/agents/customers/tags, channel integrations with encrypted
credentials, LINE/Facebook webhook ingestion, auto-reply engine, delayed
messages, file management with signed URLs, WebSocket realtime (rooms,
broadcasts, presence, collaboration), background job queue with retries +
dead-letter, notifications/reminders/alerting, monitoring + circuit breaker,
analytics/dashboards, reports + scheduling, system administration.
**545 tests (integration + installer); clippy-clean.**

**Frontend** — shared API client (single-flight token refresh, guarded login
redirect, backoff retries), optimistic-update store layer, realtime client
with reconnection, route guards, and screens for every documented
destination, plus the installer setup wizard at `/install`.

## Running

```bash
# Backend (port 3000; creates data/mcss.db)
cd backend && cargo run
# Frontend dev server (proxies /api to :3000)
cd frontend && npm install && npm run dev
# Installer service (port 8976)
cd backend && cargo run --bin installer
```

Key environment variables (see `backend/src/config.rs`): `JWT_SECRET`,
`ENCRYPTION_KEY` (32-byte hex, enables credential encryption at rest),
`LINE_CHANNEL_SECRET`, `LINE_CHANNEL_ACCESS_TOKEN`, `FACEBOOK_APP_SECRET`,
`FACEBOOK_VERIFY_TOKEN`, `LIFF_ID`, `FRONTEND_URL`, `BACKEND_URL`.

Bootstrap an admin: insert an `agents` row with an argon2 hash, or run the
installer wizard. All subsequent administration happens through the API/UI.

Frontend unit tests: `cd frontend && npm test` (vitest).

## Testing

```bash
cd backend && cargo test          # 545 tests (integration + installer)
cd backend && cargo clippy --all-targets
cd frontend && npm run build      # type-check + bundle
```

## Deliberate boundaries

External integrations are stubbed at clearly marked points, each requiring
real credentials/infrastructure to complete:

- `TODO(channels)` — live LINE/Facebook API delivery and media download
- `TODO(cloud)` — installer's real cloud-provider provisioning
- `TODO(scale-out)` — multi-instance realtime fan-out

Everything else — including every documented status code, envelope shape,
authorization rule, and side effect — is implemented per the CRD, with the
build history and per-section progress recorded in
[`docs/plans/PROGRESS.md`](docs/plans/PROGRESS.md).

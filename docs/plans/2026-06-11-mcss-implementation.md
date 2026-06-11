# MCSS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.
> **Source of truth for every behavior:** `Rust_CRD.md` — each task cites the exact line range
> to read before implementing. Do NOT implement from memory of this plan alone.
> **Cross-session resume:** check `docs/plans/PROGRESS.md` first, then `git log`.

**Goal:** Re-implement the Multi-Channel Customer Support System from `Rust_CRD.md` in Rust,
matching the observable wire contract exactly.

**Architecture:** axum 0.8 service in `backend/`; tower layers reproduce the CRD §7.1 pipeline
order; one `domain/<area>` module per CRD section; SQLite via sqlx with string timestamps;
JWT (jti-revocable) auth; argon2 passwords; AES-256-GCM credential encryption.
See `docs/superpowers/specs/2026-06-11-mcss-architecture-design.md`.

**Tech Stack:** rust 1.96, axum 0.8, tokio, tower-http, sqlx(sqlite), serde, jsonwebtoken,
argon2, uuid, chrono, aes-gcm, rand, thiserror.

---

## Phase 1 — Core (this plan covers Phase 1; later phases get their own plan files)

### Task 1.1: Cargo scaffold + config + DB bootstrap

**Files:** Create `backend/Cargo.toml`, `backend/src/main.rs`, `backend/src/config.rs`,
`backend/src/db.rs`, `backend/src/app.rs`, `backend/.env.example`, `backend/migrations/0001_init.sql`.

- [x] Cargo.toml with deps pinned; `cargo build` passes.
- [x] `Config::from_env()`: `DATABASE_URL` (default `sqlite://data/mcss.db?mode=rwc`),
  `JWT_SECRET`, `ENCRYPTION_KEY` (optional), `ENVIRONMENT` (development/production),
  `FRONTEND_URL`, `BACKEND_URL`, `PUBLIC_STORAGE_URL` (optional), `EXTRA_ORIGINS`, `PORT` (3000).
- [x] `db.rs`: SqlitePool, WAL mode, foreign_keys ON, runs migrations at startup.
- [x] Migration `0001_init.sql`: full conceptual data model from CRD lines 5729–5832
  (teams, agents, team_members, customers, qr_codes, qr_scans, team_liff_links,
  customer_team_assignments, conversations, messages, scheduled_messages, attachments,
  conversation_sessions, conversation_transfers, message_recall_logs, notifications, tags,
  customer_tags, conversation_tags, activity_logs, system_settings, metrics,
  channel_integrations, webhook_security_events, cors_events, reports, scheduled_reports,
  scheduled_report_runs, report_downloads, report_templates, task_reminders,
  customer_feedback, auto_reply_rules, auto_reply_conditions, auto_reply_actions,
  auto_reply_business_hours, auto_reply_logs, auto_reply_deliveries, auth_sessions,
  refresh_tokens, revoked_tokens). TEXT timestamps; soft-delete `deleted_at` per CRD §7.2
  lifecycle list (line 5817); FK actions per "Referential removal behavior" (lines 5829–5832).
- [x] `GET /` root probe (CRD 5632–5635): 200 `{message, timestamp, version}`. Test + commit.

### Task 1.2: Envelopes + error taxonomy

**Files:** Create `backend/src/envelope.rs`, `backend/src/error.rs`. Test `backend/tests/pipeline.rs`.
**Read CRD 5648–5701 first.**

- [x] Success envelope `{success:true, data?, message?, timestamp, requestId}`; helper for 200/201.
- [x] Paginated envelope (items, page, pageSize+limit, total, totalPages, hasNext, hasPrev) with
  clamping rules (CRD 5663: page size 1–100 default 20, max page 1000 — clamp, don't reject).
- [x] `AppError` enum → (status, machine code, message) per taxonomy table (CRD 5686–5697);
  validation errors carry field-problem array under data.
- [x] 404 unknown-route fallback `{error:"Not Found", message, timestamp}` (no success flag, CRD 5637–5640).
- [x] Tests: error mapping statuses + envelope shapes. Commit.

### Task 1.3: Pipeline middleware (CORS, security headers, rate limit, metrics)

**Files:** Create `backend/src/middleware/{mod,cors,security_headers,rate_limit,metrics}.rs`.
**Read CRD 5590–5630 (CORS/preflight/rejection), 5620–5626 (rate limit), 5615–5618 (metrics),
5656 (allowed-origins policy), 5628–5630 (security headers).**

- [x] Allowed-origins: dev set (localhost/127.0.0.1 common ports) in non-production + configured
  FRONTEND/BACKEND/PUBLIC_STORAGE/EXTRA origins.
- [x] OPTIONS preflight: allowed → 204 with methods/headers/credentials/max-age 86400, no-cache;
  disallowed → 403 structured body with `CORS_ORIGIN_NOT_ALLOWED`/`CORS_CONFIGURATION_MISSING` + header echo.
- [x] Non-OPTIONS: handler runs first; allowed origin → echo allow-origin + credentials headers.
- [x] Security headers on every response; HSTS only if secure transport.
- [x] Sliding-window in-process rate limiter with presets (standard 100/60, auth 10/60,
  login 5/300, upload 20/60, ws 30/60, admin 200/60, high-freq 500/60); headers
  X-RateLimit-Limit/Remaining/Reset on all responses; 429 + Retry-After when exceeded; fail-open.
- [x] Metrics layer: skip OPTIONS/non-API/health/static; async emit {method, normalized path
  (ids→placeholder), status, elapsed ms, ts} to metrics table; never fails request.
- [x] Tests for each. Commit.

### Task 1.4: Auth domain (§1.1) — CRD lines 126–293

**Files:** Create `backend/src/domain/auth/{mod,routes,handlers,tokens,store}.rs`,
test `backend/tests/auth.rs`. **Read CRD 126–293 fully before coding.**

- [x] JWT claims: {sub=userId, email, name, role, primaryTeamId, teams[], type: access|refresh|temp_change|service, jti, exp, iat}.
  Access 2h, refresh 7d, temp-change 30m. Refresh single-use rotation: `refresh_tokens` table
  rows {jti, user_id, consumed_at, revoked_at}; reuse → revoke + 401 "reuse detected".
- [x] POST /api/auth/login (CRD 135–143): trim, generic 401 for all failures, must_change →
  tempToken flow, session record + activity log, expiresIn 7200, agent view (createdAt epoch ms).
- [x] POST /api/auth/register (145–153): admin-only, role admin|agent, optional teamId → primary
  member, soft-deleted email reactivation (clear old memberships), 409 on active duplicate.
- [x] POST /api/auth/logout (155–163): X-Session-ID required, revoke access jti, optionally
  revoke body refreshToken only if owner matches.
- [x] POST /api/auth/refresh (165–172): rotation + reuse detection, account re-check, team
  claims re-read, fail-closed.
- [x] GET /api/auth/profile (174–180), GET /api/auth/me (182–188), PUT /api/auth/me (190–197,
  displayName 1–50, no-op skip), POST /api/auth/change-password (199–206, current-password proof,
  audit), POST /api/teams/members/:memberId/reset (208–215, manager+, no self-reset, policy).
- [x] /phase2-auth/*: monitoring-token, user-token, batch-tokens (non-prod), verify-token,
  refresh-token, status (217–263).
- [x] Tests per operation incl. error conditions. Commit.

### Task 1.5: AuthN/AuthZ middleware (§1.3) — CRD lines 485–650

**Files:** Create `backend/src/middleware/auth.rs`, `backend/src/domain/authz.rs`,
test `backend/tests/authz.rs`. **Read CRD 485–650.**

- [x] Bearer guard per CRD 492–515: 401 paths, refresh-as-access rejected, jti revocation
  (fail closed 503 REVOCATION_CHECK_FAILED), inactive account 401, membership re-read w/ 60s
  cache, X-Context-Team-ID validation, last-active debounce.
- [x] Guards: require_role(exact), require_role_level(min), require_team_access,
  require_team_role(min), require_team_permission; admin bypass everywhere.
- [x] Optional-auth gate; system-key gate (X-System-Key); session-based gate (X-Session-ID).
- [x] Tests. Commit.

### Task 1.6: Auth sessions persistence (§1.2 Part A) — CRD lines 301–328

- [x] `auth_sessions` table ops: create on login (24h expiry), lookup, sliding touch, delete on
  logout, expire. Already covered partly by 1.4; verify behaviors + tests. Commit.

## Phase 2+ (separate plan files when reached)

- Phase 2 org: CRD 1644–2611 (customers/teams/agents/collaboration/activity).
- Phase 3 conversations: CRD 651–1643 + 329–483.
- Phase 4 realtime: CRD 3221–4200.
- Phase 5 channels: CRD 2612–3220 + 1171–1452 + 5106–5246.
- Phase 6 ops: CRD 4201–5578.
- Phase 7 frontend: CRD 5844–6723. Phase 8 installer: CRD 6724–6979.

## Verification per task

`cd backend && cargo test` green before every commit; `cargo clippy` clean at phase end.

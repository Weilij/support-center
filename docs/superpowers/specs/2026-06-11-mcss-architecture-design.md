# Multi-Channel Customer Support System — Architecture Design

Date: 2026-06-11
Source spec: `Rust_CRD.md` (clean-room functional specification, ~7000 lines, 500+ operations).
This document records the *clean-room implementation decisions* — everything behavioral comes
from the CRD; nothing here may contradict it.

## Goal

Re-implement the Multi-Channel Customer Support System (LINE OA + Facebook Messenger) described
by `Rust_CRD.md` in Rust, matching the observable wire contract: HTTP routes, envelopes, error
taxonomy, WebSocket events, authorization rules, and side effects.

## Technology decisions

| Concern | Choice | Rationale |
|---|---|---|
| Language | Rust (edition 2021) | Mandated by the CRD ("target language: Rust"). |
| Web framework | axum 0.8 + tower middleware | The CRD's fixed pipeline-stage order (§7.1) maps directly onto tower layers; axum has first-class WebSocket support needed for §5. |
| Async runtime | tokio | axum requirement; background queues (§6.5) and delayed messages (§2.4) need timers/tasks. |
| Database | SQLite via `sqlx` (runtime queries, no compile-time macros) | CRD §7.2: "All persisted records use string-based timestamps" — fits SQLite. Zero-ops local dev; sqlx keeps a Postgres migration path open. WAL mode for concurrency. |
| Passwords | `argon2` (one-way hash) | CRD requires non-reversible hash, never recoverable. |
| Tokens | `jsonwebtoken` JWT; every token carries a unique `jti` | CRD requires independently revocable access/refresh credentials, type markers, rotation with reuse detection. |
| Credential encryption at rest | AES-256-GCM (`aes-gcm` crate) with random nonce | CRD §7.2 guarantees: non-deterministic, tamper-evident, key-gated, mixed plaintext/protected tolerance. AEAD satisfies all four. |
| IDs | Teams/tags/customers/integrations: integer autoincrement. Staff/conversations/messages/sessions/files/notifications/reports: UUID-v4 strings | Matches the CRD's "numeric identifier" vs "string identifier" distinction per entity. |
| WebSocket | axum's built-in `ws` | Single-process room registry first; the CRD's multi-instance scaling is an observable-behavior target for a later phase. |
| Frontend (Phase 7) | Vite + React SPA | CRD §8 describes an SPA with role-gated routes, optimistic updates, i18n. |

## Repository layout

```
support-center/
├── Rust_CRD.md              # the spec (read-only input)
├── docs/
│   ├── superpowers/specs/   # this design doc
│   └── plans/               # implementation plan + progress state
├── backend/                 # Rust cargo crate (axum service)
│   ├── migrations/          # sqlx SQLite migrations
│   ├── src/
│   │   ├── main.rs          # bootstrap only
│   │   ├── app.rs           # router assembly = CRD pipeline order
│   │   ├── config.rs        # env config (JWT secret, encryption key, origins…)
│   │   ├── db.rs            # pool init + migration runner
│   │   ├── error.rs         # error taxonomy → wire mapping (§7.1)
│   │   ├── envelope.rs      # success / paginated / error envelopes (§7.1)
│   │   ├── middleware/      # cors, security headers, rate limit, metrics, auth guards
│   │   ├── domain/          # one module per CRD area (auth, teams, conversations…)
│   │   └── realtime/        # WS gateway, rooms, broadcast (§5)
│   └── tests/               # integration tests (tower oneshot per endpoint)
└── frontend/                # Phase 7
```

Each `domain/<area>` module owns its routes, handlers, storage queries, and types — bounded by
the CRD's own section boundaries, so a module can be built/tested against its spec section alone.

## Cross-cutting contracts (implemented once, inherited everywhere)

- **Envelopes** (§7.1): `{success, data?, message?, timestamp, requestId}`; paginated variant
  with `items/page/pageSize/limit/total/totalPages/hasNext/hasPrev`; error variant
  `{success:false, error, timestamp, requestId}` with optional validation detail.
- **Error taxonomy**: typed `AppError` enum mapping to the CRD's status + machine-code table
  (`VALIDATION_ERROR` 422, `UNAUTHORIZED` 401, `FORBIDDEN` 403, `NOT_FOUND` 404, `CONFLICT` 409,
  `TOO_MANY_REQUESTS` 429, `INTERNAL_ERROR` 500, …).
- **Pipeline order** (load-bearing, §7.1): CORS preflight → metrics → public/priority routes
  (webhooks, health, signed file proxy, WS upgrade) → domain routes → error trap → security
  headers → root/404 fallback. Public routes must never be shadowed by auth catch-alls.
- **Rate limiting**: sliding-window per-IP per-scope, in-process store; presets standard 100/60s,
  auth 10/60s, login 5/300s, upload 20/60s, ws 30/60s, admin 200/60s, high-freq 500/60s;
  fail-open; rate headers on every response.
- **AuthN middleware**: Bearer JWT → signature/expiry → type check (refresh rejected) → jti
  revocation check (fail closed 503 `REVOCATION_CHECK_FAILED`) → account active check → team
  memberships re-read (~60s cache) → optional `X-Context-Team-ID`.
- **AuthZ model** (§1.3): system roles `admin` > `agent`; team roles `member` < `lead` <
  `supervisor`; admin bypasses all checks; team-scoped guards.
- **Soft delete** (§7.2): `deleted_at` nullable column on units, staff, customers,
  conversations, messages, labels, reports, schedules, templates, auto-reply rules; normal reads
  filter it; append-only logs never soft-delete.

## Build phases (dependency order)

1. **Core**: scaffold, config, envelopes, errors, pipeline middleware, full DB migration set
   (§7.2 conceptual model), auth (§1.1), sessions (§1.2A), authz middleware (§1.3). 
2. **Org**: teams (§3.2), agents (§3.3), customers (§3.1), tags (§2.6), activity log (§3.5).
3. **Conversations**: agent-side (§2.1), messaging (§2.2), customer-facing (§2.3),
   conversation sessions (§1.2B).
4. **Realtime**: WS gateway (§5.1), rooms/broadcast (§5.2-5.5), collaboration (§3.4).
5. **Channels**: integrations (§4.1), webhook ingestion (§4.2), LIFF (§4.3), files (§4.4),
   delayed messages (§2.4), auto-reply (§2.5), background queue (§6.5).
6. **Ops**: analytics (§6.1), reports (§6.2), monitoring (§6.3), notifications (§6.4),
   settings (§6.6), rate-limit guarantees (§6.7).
7. **Frontend** (§8). 8. **Installer** (§9).

Within a phase: read the relevant CRD lines → write integration tests from the operation blocks
→ implement → verify (`cargo test`). Commit per module.

## Error handling & testing

- All handlers return `Result<_, AppError>`; the trap layer renders the canonical envelope.
- Integration tests drive the axum `Router` via `tower::ServiceExt::oneshot` against a temp
  SQLite file per test; each CRD operation block becomes at least one test asserting status,
  envelope shape, and the documented error conditions.

## Out of scope for now

Real LINE/Facebook API delivery (stubbed behind a trait — observable side effects recorded
locally), multi-instance WS scaling, and the cloud-provisioning installer's real cloud calls.

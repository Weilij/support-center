# Rust_CRD.md
# Multi-Channel Customer Support System — Clean-Room Functional Specification

> **Document type:** Black-box behavioral contract (Clean-Room / Chinese-Wall reverse-engineering output)
> **Produced:** 2026-06-05
> **Coverage:** 38/38 system areas · 659 top-level operation blocks, covering HTTP endpoints, real-time events, background triggers, and client-visible flows.
> **Source disclosure:** NONE. This document contains no original source code, no internal identifiers, no implementation algorithms, and no internal storage-schema names.

---

## 0. Methodology & The Wall

### 0.1 What this document is

This is a **functional specification** describing the *observable behavior* of an existing Multi-Channel Customer Support System (LINE Official Account + Facebook Messenger). It was produced by a **Dirty Room team** that studied the running system and its source, and records ONLY *what the system does* — given input X, what output and side effects are observable as Y.

It is intended to be read by a **Clean Room team** who will re-implement the system (target language: Rust) having **never seen the original source code**. The wall between the two teams is the entire point: everything a re-implementer is legally permitted to know lives in this document; nothing that would contaminate a clean-room implementation does.

### 0.2 The isolation boundary (exactly where the wall stands)

**INCLUDED — observable external interface ("input X → output Y"):**

- HTTP method + route path of every endpoint (the external wire contract any client can observe).
- Request fields (path / query / body) and response fields *as they travel on the wire*, with meaning, type, constraints, and required/optional status.
- HTTP status codes and machine-readable error codes returned to callers.
- Real-time (WebSocket) event names and the shape of their payloads as broadcast to clients.
- Authorization rules: which role/state may invoke each operation, and what is observed on denial.
- Observable side effects: state a caller can later observe, events emitted, downstream actions triggered.

**EXCLUDED — implementation (would contaminate the clean room):**

- Original internal identifiers: function, class, method, private-variable, file, and module-export names.
- Implementation algorithms and data structures — the *how*. Only the observable *what* is recorded.
- Source code or pseudocode of any kind.
- Internal storage schema: database table and column names. Persisted data is described **conceptually** with neutral, re-named terms.

> **Why wire field names are kept but storage column names are not:** a field visible in an HTTP request/response or a WebSocket event is *observable by any black-box client*, so it is part of "input X → output Y". A database column name is *not* externally observable; copying it would prescribe the re-implementation's internal storage, so it is replaced by a neutral conceptual description.

### 0.3 Clean-room integrity audit

Contamination was removed in two passes. First, an independent auditor re-examined every section for leaks of the *how* and flagged **11 items** (predominantly low-severity descriptions of internal mechanisms — credential-protection construction, throttling computation, mutual-exclusion implementation, password-protection family, internal soft-delete storage layout); the categorized ledger is in Appendix A. Second, a deterministic token-level sweep was run across the whole document to catch mechanical leaks the semantic auditor can miss — this removed residual disclosures of credential-protection construction details, repeated throttling-computation wording, mutual-exclusion mechanism hints bleeding into unrelated sections, and platform signature-scheme specifics (now referred to the external platform's published scheme). **All identified items have been neutralized** to their observable guarantee.

### 0.4 Behavioral boundaries (where the system stops)

Some areas of the live system deliberately do nothing beyond acknowledging a request, or surface a fixed value for a metric. Such limits are recorded as observable facts under a **`### Behavioral Boundary (Under-specified)`** heading, phrased strictly as "within the current system boundary, behavior X is *not* exhibited." This tells a re-implementer exactly where the black box ends: a conforming implementation must reproduce the documented operations and is **not** required to build the out-of-boundary capabilities. These notes describe observable behavior only; they make no claim about why a capability is absent. If a boundary later moves, that is a future iteration of this specification, not a defect in the re-implementation.

### 0.5 How to read an operation block

Each operation block is specified as: **Invocation** (how it is triggered) · **Inputs** · **Preconditions & Authorization** · **Behavior** (observable outcome only) · **Success Output** · **Side Effects** · **Error Conditions** · **Invariants & Guarantees**. Subsections that do not apply to a given operation are omitted. The coverage count above counts top-level `####` operation blocks only; nested bullets may describe sub-behaviors but are not counted as separate operation blocks.

---

## Table of Contents

**1. Identity, Access & Sessions**

- 1.1 [Authentication & Account Management](#authentication-account-management) — *Email/password authentication with JWT access+refresh token issuance, rotation with reuse detection, per-token revocation on logout, admin-only account creation, self-service profile/password change, admin password reset, and system/monitoring token management.* (15 ops)
- 1.2 [Sessions & Session Persistence](#sessions-session-persistence) — *Two session capabilities: authentication-session persistence (create/lookup/expire/invalidate login sessions) and an HTTP conversation-session management module (lifecycle, boundary detection, stats, batch ops).* (23 ops)
- 1.3 [Authorization, Roles & Permission Model](#authorization-roles-permission-model) — *Defines the dual-role authorization model (system Admin/Agent + per-team Member/Lead/Supervisor), team scoping, multi-team claims, and the middleware/permission-check denial behavior for HTTP and WebSocket access.* (11 ops)

**2. Conversations & Messaging**

- 2.1 [Conversations (Agent Side)](#conversations-agent-side) — *Agent-facing conversation management: list/filter/paginate, detail retrieval, team assignment/unassignment/transfer, mark-as-read, tag management, bulk operations, message listing/sending with async delivery, and attachment upload.* (12 ops)
- 2.2 [Messaging](#messaging) — *Agent-facing messaging API covering message create/read/update/recall, conversation listing, search/stats, bulk ops, attachments, forwarding, per-message tags, export, plus delayed-send and recall services and offline buffering/batching behavior.* (23 ops)
- 2.3 [Customer-Facing Conversations](#customer-facing-conversations) — *Customer-facing endpoints for accessing a single conversation's message history, posting agent replies, uploading files, and a real-time WebSocket channel — all gated by a shared four-way access check.* (4 ops)
- 2.4 [Delayed / Scheduled Messages](#delayed-scheduled-messages) — *Delayed/scheduled outbound message sends with 1-120s buffering, instant cancellation, reschedule, retry behavior, terminal-failure handling, and real-time countdown events across LINE and Facebook.* (11 ops)
- 2.5 [Auto-Reply](#auto-reply) — *Keyword/welcome/off-hours/fallback automated replies to inbound LINE messages, with rule and business-hours management, audit logging, and duplicate-send protection.* (8 ops)
- 2.6 [Tags & Labeling](#tags-labeling) — *Tag CRUD, customer/conversation tag assignment with counts, soft-delete, bulk ops, and real-time tag-change events.* (15 ops)

**3. Customer & Organization**

- 3.1 [Customers](#customers) — *Customer directory: list/lookup of multi-platform customer records with team-scoped visibility, plus per-customer tag association management with real-time broadcasts.* (8 ops)
- 3.2 [Teams](#teams) — *Team and team-member management: team CRUD, multi-team agent membership with in-team roles and a primary team, member account lifecycle, password reset/change, per-team QR codes, scoping rules, and real-time member-change events.* (31 ops)
- 3.3 [Agents / Operators](#agents-operators) — *Operator account management: profile CRUD, team membership/transfer, per-operator skills, real-time presence status, and availability eligibility lookups, all role-gated.* (14 ops)
- 3.4 [Collaboration](#collaboration) — *Real-time collaboration: conversation viewer/presence/typing tracking and event broadcasting over WebSocket-backed durable rooms, exposed via authenticated HTTP endpoints.* (9 ops)
- 3.5 [Activity Log & Reversible Actions](#activity-log-reversible-actions) — *An audit-trail subsystem that records user actions and lets eligible callers undo specific reversible actions (e.g. conversation transfers) with conflict detection.* (12 ops)

**4. Channels & Integration**

- 4.1 [Channel Integrations](#channel-integrations) — *Multi-tenant channel integration management: per-team messaging-platform configuration with encrypted credential storage, webhook config, verification, per-channel stats/health, and JSON-config extensibility for new platforms.* (8 ops)
- 4.2 [Inbound Webhook Ingestion & Platform Parsing](#inbound-webhook-ingestion-platform-parsing) — *Inbound webhook endpoints that authenticate, validate, deduplicate, and normalize external chat-platform events (LINE/Facebook/Instagram) into internal customers, conversations, and messages with real-time broadcasts.* (6 ops)
- 4.3 [LIFF (LINE Front-end Framework) Integration](#liff-line-front-end-framework-integration) — *Public LIFF onboarding endpoints plus team/admin QR-code generation that bind LINE users to support teams via scannable codes.* (12 ops)
- 4.4 [File & Attachment Management](#file-attachment-management) — *Authenticated file upload/listing/download plus unauthenticated signature-protected public proxy delivery of stored attachments.* (20 ops)

**5. Real-time Infrastructure**

- 5.1 [WebSocket Gateway & Protocol](#websocket-gateway-protocol) — *The real-time gateway: authenticated WebSocket handshake, conversation/user connection routing, an event taxonomy, a client inbound message protocol, and role/team-scoped broadcast fan-out, plus HTTP health/metrics/analytics endpoints.* (33 ops)
- 5.2 [Conversation Room & Message Broadcast Delivery](#conversation-room-message-broadcast-delivery) — *Real-time coordination behavior: per-conversation room membership (WebSocket join/leave, message fan-out, ordering, reconnection sync, transparent multi-instance scaling) and global event broadcasting to conversations/users/teams/global with priority and retry semantics.* (28 ops)
- 5.3 [User Real-time Sessions](#user-real-time-sessions) — *Per-user real-time session behavior covering WebSocket lifecycle, conversation subscriptions, presence, preferences, security (rate limiting/auth), and cross-conversation broadcast fan-out for one user.* (13 ops)
- 5.4 [Customer-Side Real-time Channels](#customer-side-real-time-channels) — *Per-conversation real-time and message behavior covering agent WebSocket connections, presence, broadcasts, message fetch/create/upload endpoints, LINE delivery, and fan-out.* (6 ops)
- 5.5 [Realtime Module & Latest-Message Cache](#realtime-module-latest-message-cache) — *Real-time event dispatch, observable latest-message freshness guarantees, and monitoring/management endpoints for the live-messaging subsystem.* (23 ops)

**6. Operations, Analytics & System**

- 6.1 [Analytics](#analytics) — *Analytics provides authenticated, role-scoped reporting over conversations, messages, users, performance, custom queries, exportable reports, raw/aggregated time-series metrics, period-over-period comparisons, configurable dashboards/widgets, and a security-events dashboard.* (29 ops)
- 6.2 [Reports](#reports) — *Reporting subsystem: generate/list/download/delete reports, statistics, batch ops, preview with sample data, and scheduled reports with automated cron-driven execution.* (15 ops)
- 6.3 [Monitoring & Health](#monitoring-health) — *System health checks, real-time infrastructure metrics aggregation, alerting thresholds, and circuit-breaker control endpoints for the multi-channel support platform.* (14 ops)
- 6.4 [Notifications](#notifications) — *In-app and external notification system: per-user notification records (create/list/read/delete/stats) with real-time WebSocket delivery, type-specific triggers, scheduled task reminders, and operational/security alerting over external channels.* (28 ops)
- 6.5 [Background Queue Processing](#background-queue-processing) — *Background processing that asynchronously delivers outbound LINE messages and processes inbound LINE media, with retry behavior, terminal-failure handling, delivery-status persistence, WebSocket broadcasting, and authenticated monitoring endpoints.* (7 ops)
- 6.6 [System Settings & Administration](#system-settings-administration) — *Administrative and observability surface: system info/health/status/metrics, persisted system settings, channel integration testing, customer feedback, task reminders, notification-channel and data/UX-monitoring configuration, storage-efficiency monitoring, and admin-only data migration.* (52 ops)
- 6.7 [Rate Limiting & Mutual-Exclusion Guarantees](#rate-limiting-mutual-exclusion-guarantees) — *Request throttling behavior plus the mutual-exclusion and data-consistency guarantees the system requires for concurrent critical sections.*

**7. Cross-cutting Backend**

- 7.1 [Request Pipeline, Routing & Conventions](#request-pipeline-routing-conventions) — *Defines the shared HTTP request lifecycle, middleware order, route precedence rules, CORS/security policy, the canonical success/error response envelope, error taxonomy, and global limits for the entire API surface.* (9 ops)
- 7.2 [Conceptual Data Model & Credential Security](#conceptual-data-model-credential-security) — *Defines the system's conceptual entity-relationship model (teams, agents, customers, conversations, messages, tags, channel integrations, reports, auto-reply, etc.), soft-delete conventions, and the observable confidentiality/integrity guarantees for integration credentials at rest.* (5 ops)

**8. Frontend Behavior**

- 8.1 [Frontend State Model](#frontend-state-model) — *Browser-visible state behavior governing auth/session, conversations, messages, teams, notifications, tags, auto-reply, QR codes, system settings, real-time WebSocket fan-out, and shared caching/query behaviors with optimistic updates and rollback.* (95 ops)
- 8.2 [Frontend Views & User Flows](#frontend-views-user-flows) — *The web client's screens and user flows: authenticated SPA with role-gated navigation covering login, dashboard, conversation handling, customer/tag management, team/channel admin, settings, reports, and notifications.* (23 ops)
- 8.3 [Frontend Real-time Client & Sync](#frontend-real-time-client-sync) — *Browser-side real-time layer: WebSocket connect/auth/reconnect, event routing to channels, list/detail sync, optimistic reconciliation, and background workers for processing and search.* (7 ops)
- 8.4 [Frontend Routing, Guards, API Layer & i18n](#frontend-routing-guards-api-layer-i18n) — *Frontend SPA route protection/redirects, the shared authenticated HTTP API client layer (token refresh, retries, team context), the full set of client-side endpoint contracts mirroring the backend, and multi-language localization behavior.* (95 ops)
- 8.5 [Frontend-to-Backend Traceability Matrix](#frontend-to-backend-traceability-matrix) ??*Cross-check matrix mapping front-end requirements to concrete backend endpoints.* (1 op)

**9. Web Installer**

- 9.1 [Web Installer — Backend / Provisioning](#web-installer-backend-provisioning) — *Self-hosted provisioning system that, from a single configuration request, creates a tenant's full cloud stack (database, key stores, file store, queue, backend service, frontend site, optional custom domain, admin account) with progress polling/streaming and automatic rollback on failure.* (9 ops)
- 9.2 [Web Installer — Frontend / Setup Wizard](#web-installer-frontend-setup-wizard) — *A multi-step browser-based setup wizard that authenticates a hosting account, collects deployment configuration, triggers provisioning, polls live status, and presents generated admin credentials.* (8 ops)

- [Appendix A — Clean-Room Integrity Audit Ledger](#appendix-a--clean-room-integrity-audit-ledger)


---

# 1. Identity, Access & Sessions

## Authentication & Account Management

### Purpose
This area lets staff users sign in with an email and password, receive short-lived access credentials and a long-lived renewal credential, renew those credentials without re-entering a password, and sign out in a way that immediately invalidates the issued credentials. It also lets administrators create staff accounts, lets a signed-in user view and lightly edit their own profile and change their own password, lets managers/administrators reset another member's password and enforce a forced-change policy, and provides administrators with facilities to mint, inspect, and renew internal/monitoring service credentials. Routes in this area live under a common `/api/auth` prefix unless otherwise noted; the service-token management operations live under a separate `/phase2-auth` prefix.

### Operations

#### Sign In — POST /api/auth/login
- Invocation: unauthenticated client request.
- Inputs (JSON body): `email` (string, required); `password` (string, required). Both are trimmed of leading/trailing whitespace before use.
- Preconditions & Authorization: none (public). Subject to a strict per-client-IP rate limit (allows roughly 5 attempts per 5-minute window).
- Behavior: validates that both fields are present after trimming. Looks up the account by email and verifies the password. To prevent account enumeration, every failure mode (no such email, account disabled, wrong password) returns one identical generic error and the request takes comparable time regardless of which failure occurred. On a correct password, if the account is flagged as requiring a forced password change, the caller is NOT fully signed in: instead a short-lived single-purpose change-password credential (valid ~30 minutes) is returned along with a flag indicating a change is required. Otherwise the user is fully signed in: the last-login timestamp is recorded (best-effort; failure does not block sign-in), a short-lived access credential (valid 2 hours) and a long-lived renewal credential (valid 7 days) are issued, the renewal credential is registered as the user's single active renewal slot, a server-side session record is created, and the sign-in is recorded in the activity log with method, IP, and user-agent.
- Success Output (HTTP 200): an envelope with `success: true`, a `timestamp`, and a `data` object. For a normal sign-in `data` contains `token` (access credential), `refreshToken` (renewal credential), `sessionId`, `expiresIn` (7200 seconds), and an `agent` object (`id` as string, `email`, `name`, `displayName`, `role`, `isActive`, `createdAt` as epoch milliseconds). For a forced-change response `data` contains `mustChangePassword: true`, `tempToken` (the change-only credential), and the same `agent` object, with a message indicating the password must be changed.
- Side Effects: a server-side session is established and an opaque session identifier returned; the issued renewal credential is recorded so it can be redeemed once; last-login timestamp updated; sign-in activity logged.
- Error Conditions: missing email or password after trimming → HTTP 400 with a "required" message. Any authentication failure → HTTP 401 with a single generic "invalid email or password" message. Rate limit exceeded → HTTP 429 with retry-after information. Unexpected server failure → standard error response.
- Invariants & Guarantees: identical error message and comparable response time across all credential-failure cases (no enumeration oracle). Access and renewal credentials each carry an independent unique identifier so they can be revoked separately. Disabled accounts can never sign in.

#### Create Account (Administrator) — POST /api/auth/register
- Invocation: authenticated client request.
- Inputs (JSON body): `email` (string, required), `password` (string, required), `displayName` (string, required), `role` (string, required, must be one of `admin` or `agent`), `teamId` (number, optional — assigns the new user to a team as a primary member with the lowest team role).
- Preconditions & Authorization: requires a valid access credential AND the caller's system role must be `admin`.
- Behavior: validates all required fields are present and that `role` is an allowed value. Rejects if an active (non-deleted) account already uses that email. Creates the account; if a previously soft-deleted account exists with the same email, that record is reactivated and re-provisioned rather than duplicated, and its prior team memberships are cleared first. If a team was specified, a primary team membership is created.
- Success Output (HTTP 200): envelope with `success: true`, `timestamp`, and `data.user` containing `id`, `email`, `displayName`, `role`, `teamId` (primary team), and `teamName`.
- Side Effects: a new (or reactivated) account is persisted with a securely hashed password; optional team membership created; the creation is recorded in the activity log (with the created user's email/displayName/role/team) as a reversible action.
- Error Conditions: any required field missing → HTTP 400. Invalid `role` → HTTP 400. Email already in use by an active account → HTTP 409 conflict. Missing/invalid credential → HTTP 401. Non-admin caller → HTTP 403.
- Invariants & Guarantees: email is unique among active accounts; soft-deleted accounts are reactivated in place rather than creating duplicates.

#### Sign Out — POST /api/auth/logout
- Invocation: authenticated client request (session-based).
- Inputs: `X-Session-ID` header (the session identifier to terminate); `Authorization` header carrying the access credential; optional JSON body with a `refreshToken` field naming the renewal credential to also invalidate.
- Preconditions & Authorization: requires a valid session identifier resolving to an active user.
- Behavior: deletes the named server-side session. If an access credential bearing a unique identifier is present, that credential is marked revoked for the remainder of its validity, so it is rejected by protected endpoints thereafter. If a renewal credential is supplied in the body, it is invalidated the same way — but only after verifying it belongs to the same signed-in user (a supplied renewal credential belonging to a different user is ignored, preventing a denial-of-service by revoking someone else's session). All credential-revocation steps are best-effort and do not block the response.
- Success Output (HTTP 200): envelope with `success: true`, a "logged out" message, and a `timestamp`.
- Side Effects: session record deleted; access (and optionally renewal) credential marked revoked; sign-out recorded in the activity log.
- Error Conditions: missing/invalid session identifier → HTTP 401. Server failure → standard error response.
- Invariants & Guarantees: a revoked credential is rejected by any protected endpoint on its next use; a renewal credential is only revoked if its owner matches the signed-in user.

#### Renew Credentials — POST /api/auth/refresh
- Invocation: unauthenticated client request carrying a renewal credential.
- Inputs (JSON body): `refreshToken` (string, required).
- Preconditions & Authorization: none beyond a valid, unexpired renewal credential of the correct type. Subject to a per-IP rate limit (~5 per 5-minute window).
- Behavior: verifies the renewal credential, enforces expiry, and confirms its declared type is the renewal type. Enforces single-use rotation with reuse detection: a renewal credential can be successfully redeemed once, and each successful renewal returns a fresh access credential (2 hours) plus a fresh renewal credential (7 days). If a credential is presented after it has already been redeemed or otherwise invalidated, reuse is detected: the presented credential is revoked, a security warning with user/IP/user-agent is logged, and the caller must sign in again. Before issuing, the account is confirmed to still exist and be active, and the user's team memberships are re-read from authoritative storage so the new credentials reflect current team data rather than stale claims. Credentials that predate the unique-credential scheme bypass rotation enforcement for backward compatibility.
- Success Output (HTTP 200): envelope with `success: true`, `timestamp`, and `data` containing a new `token` and `refreshToken`.
- Error Conditions: missing `refreshToken` → HTTP 400. Invalid/expired/malformed credential or wrong payload shape → HTTP 401. Wrong credential type → HTTP 401. Reuse detected → HTTP 401 with a "reuse detected — please log in again" message. Account not found or inactive → HTTP 401. If the renewal credential's replay/validation state cannot be read, the request fails closed → HTTP 401. Rate limit exceeded → HTTP 429.
- Invariants & Guarantees: rolling rotation — each renewal yields a new renewal credential and consumes the old one; replay of a consumed credential is detected and the credential blocklisted; team data is always re-derived from authoritative storage at renewal time.

#### View Own Profile — GET /api/auth/profile
- Invocation: authenticated client request.
- Inputs: none.
- Preconditions & Authorization: valid access credential.
- Behavior: returns the signed-in user's profile.
- Success Output (HTTP 200): envelope with `data.user` containing `id`, `email`, `displayName`, `role`, primary team identifier, team name, `isActive`, created and updated timestamps.
- Error Conditions: missing/invalid/revoked credential → HTTP 401.

#### View Current User (alias) — GET /api/auth/me
- Invocation: authenticated client request.
- Inputs: none.
- Preconditions & Authorization: valid access credential.
- Behavior: returns a compact view of the signed-in user.
- Success Output (HTTP 200): envelope with `data` containing `id` (string), `email`, `name`, `displayName`, `role`, `isActive`, and `createdAt` (epoch milliseconds).
- Error Conditions: missing/invalid/revoked credential → HTTP 401.

#### Edit Own Profile — PUT /api/auth/me
- Invocation: authenticated client request.
- Inputs (JSON body): `displayName` (string, optional). Only the display name is editable through self-service; any other field is ignored. Email and role can only be changed by administrators through team-member management, not here.
- Preconditions & Authorization: valid access credential; a user edits only their own record.
- Behavior: trims and validates the display name (must be 1–50 characters). If the submitted value equals the current value (or no editable field was supplied), no write occurs and a "no changes" response is returned. Otherwise the display name is updated, the updated timestamp is set, and the change is recorded in the activity log (self-service flag plus old/new values) as a reversible action.
- Success Output (HTTP 200): envelope with the updated compact user view and a "profile updated" (or "no changes") message.
- Error Conditions: display name out of the 1–50 length range → HTTP 400. No updatable field provided → HTTP 400. User record not found → HTTP 404. Update failed → HTTP 500. Missing/invalid credential → HTTP 401.
- Invariants & Guarantees: strict allowlist prevents privilege escalation via this endpoint; no-op updates are skipped (no write, no log entry).

#### Change Own Password — POST /api/auth/change-password
- Invocation: authenticated client request.
- Inputs (JSON body): `currentPassword` (string, required), `newPassword` (string, required).
- Preconditions & Authorization: valid access credential; operates only on the caller's own account.
- Behavior: requires both fields. Re-verifies the supplied current password against the stored hash. On mismatch, a security event (failed change, wrong current password) is logged and the request is rejected. On success the new password is hashed and stored, the updated timestamp is set, and a success audit entry (without the password) is logged.
- Success Output (HTTP 200): envelope with `success: true`, a "password changed" message, and a `timestamp`.
- Error Conditions: either field missing → HTTP 400. User not found → HTTP 404. Current password incorrect → HTTP 401 (and a failed-attempt audit entry is recorded). Server failure → standard error response.
- Invariants & Guarantees: the stored password material is never returned or logged; current-password proof is required before any change.

#### Reset Member Password (Manager/Administrator) — POST /api/teams/members/:memberId/reset
- Invocation: authenticated client request.
- Inputs: path `memberId` (target account identifier); JSON body `newPassword` (string, required) and optional `policy` (one of `changeable`, `unchangeable`, `must_change`).
- Preconditions & Authorization: valid access credential AND a manager-or-administrator role level.
- Behavior: requires the new password. Refuses to reset the caller's own password through this endpoint (directing them to the self-service change endpoint). Hashes and stores the new password for the target member, updates the timestamp, and if a policy was supplied, stores that password policy (notably `must_change` will force the target to change the password at their next sign-in).
- Success Output (HTTP 200): envelope with a "password reset" message and `data.passwordPolicy` reflecting the stored policy.
- Error Conditions: missing new password → HTTP 400. Attempt to reset own password here → HTTP 403. Target member not found → HTTP 404. Missing/invalid credential → HTTP 401. Insufficient role level → HTTP 403.
- Invariants & Guarantees: self-reset is disallowed via this admin path; setting the `must_change` policy diverts the target's next sign-in into the forced-change flow.

#### Issue Monitoring Service Credential (Administrator) — POST /phase2-auth/monitoring-token
- Invocation: authenticated client request.
- Inputs: query `expiresIn` (seconds, optional, default 604800 / 7 days; must be between 3600 and 2,592,000).
- Preconditions & Authorization: valid access credential AND system role `admin`.
- Behavior: mints a long-lived service credential identified as a system/monitoring credential.
- Success Output (HTTP 200): the credential, its type (`monitoring`), `expiresIn`, an absolute expiry timestamp, and the issuing administrator's identifier.
- Error Conditions: non-admin → HTTP 403. `expiresIn` out of range → HTTP 400.

#### Issue User Service Credential (Administrator) — POST /phase2-auth/user-token
- Invocation: authenticated client request.
- Inputs (JSON body): `targetUserId` (required), `expiresIn` (seconds, optional, default 3600; must be between 300 and 86,400).
- Preconditions & Authorization: valid access credential AND system role `admin`.
- Behavior: looks up the target user and mints a service credential on their behalf.
- Success Output (HTTP 200): the credential, type (`user`), a summary of the target user (`id`, `displayName`, `role`, primary team), `expiresIn`, an absolute expiry, and the issuing administrator's identifier.
- Error Conditions: non-admin → HTTP 403. Missing `targetUserId` → HTTP 400. `expiresIn` out of range → HTTP 400.

#### Batch-Issue Test Credentials (Administrator, non-production) — POST /phase2-auth/batch-tokens
- Invocation: authenticated client request.
- Inputs (JSON body): `users` (array of user descriptors, required, 1–10 entries), `expiresIn` (seconds, optional, default 3600).
- Preconditions & Authorization: valid access credential AND system role `admin`; only permitted outside the production environment.
- Behavior: mints one service credential per descriptor.
- Success Output (HTTP 200): the array of issued credentials, the count, `expiresIn`, the issuing administrator's identifier, and a warning that these are development-only.
- Error Conditions: non-admin → HTTP 403. Production environment → HTTP 403. Empty or non-array `users` → HTTP 400. More than 10 entries → HTTP 400.

#### Inspect Credential — POST /phase2-auth/verify-token
- Invocation: unauthenticated client request.
- Inputs (JSON body): `token` (string, required).
- Preconditions & Authorization: none.
- Behavior: verifies the supplied credential and reports its status. Does not consult revocation state.
- Success Output (HTTP 200): when valid, `valid: true` plus a sanitized payload view (`userId`, `displayName`, `role`, primary team, system-credential flag), an absolute expiry, remaining seconds, and an "expiring soon" flag (remaining under one hour). When invalid, HTTP 200 with `valid: false` and an error description.
- Error Conditions: missing `token` → HTTP 400. An invalid credential does not raise an HTTP error — it returns `valid: false`.

#### Renew Service Credential — POST /phase2-auth/refresh-token
- Invocation: unauthenticated client request.
- Inputs (JSON body): `token` (string, required).
- Preconditions & Authorization: none beyond a verifiable credential.
- Behavior: verifies the supplied credential; if it is a monitoring/system credential a fresh monitoring credential is minted, otherwise a fresh user service credential is minted from the existing claims.
- Success Output (HTTP 200): the new credential and its type, with a confirmation message.
- Error Conditions: missing `token` → HTTP 400. Unverifiable/expired credential → HTTP 400.

#### Authentication Status — GET /phase2-auth/status
- Invocation: authenticated client request.
- Inputs: none.
- Preconditions & Authorization: valid access credential.
- Behavior: reports that the caller is authenticated and summarizes their capabilities.
- Success Output (HTTP 200): `authenticated: true`, a user summary (`id`, `displayName`, `role`, primary team, team name), and a permissions map indicating which administrative capabilities the role grants (monitoring-credential issuance, user-credential issuance, analytics access, alert triggering — all true only for administrators).
- Error Conditions: missing/invalid credential → HTTP 401.

### Access-Control Behavior at Protected Endpoints (applies to all credential-guarded operations)
- A protected request must present an access credential in the `Authorization: Bearer …` header. Missing or malformed header → HTTP 401.
- The credential's signature and expiry are validated; an expired or tampered credential → HTTP 401.
- A renewal-type credential presented as an access credential is rejected → HTTP 401 (renewal credentials cannot directly access protected resources).
- A credential bearing a unique identifier that has been revoked → HTTP 401. If revocation state cannot be read, the request is denied with HTTP 503 (`REVOCATION_CHECK_FAILED`) rather than risk honoring a revoked credential. Credentials without a unique identifier (legacy) skip this check.
- The account is loaded; if it is inactive → HTTP 401. Team membership is re-derived from authoritative storage (with a brief cache) so role/team changes take effect within about a minute rather than at credential expiry.
- An optional `X-Context-Team-ID` header may select an active team context; it must be a team the user may access.
- Role enforcement: administrators bypass all role checks. For an exact-role requirement, a non-matching role → HTTP 403 with the required vs. current role. For a role-level requirement, insufficient level → HTTP 403. For team-scoped access, a non-member of the requested team → HTTP 403; a non-numeric team identifier → HTTP 400.
- Activity is debounced so a user's last-active time is persisted at most periodically, not on every request.

### Data Concepts (neutral)
- Staff Account: a unique identifier (opaque string), an email (unique among active accounts), a securely hashed password, a display name, a system role (`admin` or `agent`), an active/inactive flag, an optional password policy, and created/updated/last-login/last-active timestamps. Accounts are soft-deleted (marked deleted, retained); a soft-deleted account's email can be reclaimed by reactivating that record. Each account participates in zero or more team memberships, exactly one of which may be marked primary; each membership carries a team-scoped role (member < lead < supervisor).
- Access Credential: short-lived (2 hours), bearer-presented, carries the user identity, role, primary team, cached team membership, a type marker, and a unique identifier enabling individual revocation.
- Renewal Credential: long-lived (7 days), used only to mint fresh credentials, carries a unique identifier, and is valid for one successful redemption to support replay detection.
- Forced-Change Credential: very short-lived (~30 minutes), single-purpose, issued in place of normal sign-in when the account's policy requires a password change.
- Session Record: server-side record keyed by an opaque session identifier, holding the user identity and basic profile snapshot, with an expiry (default 24 hours); deleted on sign-out.
- Service/Monitoring Credential: administrator-minted credentials for internal/monitoring use, with configurable lifetimes and a flag distinguishing system-monitoring credentials from user-scoped ones.
- Platform Credential (related, administrator-only): encrypted secrets per messaging platform (`line`, `facebook`) and secret type; see Real-time/Side-effect notes below.

### State & Lifecycle
- Account state: active ↔ inactive (an inactive account cannot sign in or pass protected-endpoint checks); active → soft-deleted (terminal unless reactivated by reachability refresh with the same email). Reactivation restores an account to active and clears prior team memberships.
- Password policy state machine: `changeable` (default) — normal sign-in; `unchangeable` — set by administrators; `must_change` — next sign-in is diverted to the forced-change flow and yields only a short-lived change credential until the password is changed. Administrators set the policy via the member reset operation.
- Credential lifecycle: access credentials are valid until expiry or until revoked at sign-out; renewal credentials are valid until expiry, until consumed by a successful renewal (replaced by a fresh one), or until revoked on sign-out or upon reuse detection. Reuse of a consumed renewal credential is a terminal event for that credential (blocklisted) and forces a new sign-in.

### Real-time / Event Behavior
- This area emits no WebSocket/real-time client events. It produces audit/activity-log records observable later through the activity area: account sign-in (with method, IP, user-agent), sign-out, account creation (reversible), self-service profile update (reversible, with old/new values), self-service password change, failed password-change attempt (security event), and administrator platform-credential store/delete/clear/backup actions.
- Security signals are emitted to logs (not to clients) on rate-limit breaches and on detected renewal-credential reuse (including user, IP, and user-agent), intended for operational/incident-response follow-up.
- Side effect on the related platform-credential operations (administrator-only, under a separate credentials path): secrets are protected at rest and can be stored, removed, and read back as plaintext by authorized administrators; these operations require the `admin` role and are recorded in the activity log.

## Sessions & Session Persistence

### Purpose
This area provides two related but distinct capabilities. (1) An **authentication-session persistence layer** that keeps per-login session records available so a signed-in human user (admin/agent) or an inbound channel customer can be re-identified across requests; these records auto-expire and can be explicitly invalidated. (2) A **conversation-session management** capability that segments an ongoing customer conversation into discrete bounded "sessions" (one continuous exchange around a topic/time window), tracks their lifecycle (open, close, reopen), associates messages, computes statistics, and detects when a new conversation segment should begin. The two share the word "session" but are independent: the first is identity/login state; the second is conversation-segmentation state.

---

## Part A — Authentication-Session Persistence

### Behavioral Contract (observable)

Authentication-session persistence is internal infrastructure (no client-reachable session endpoints); it manifests only as side effects of sign-in, sign-out, and per-request identity checks. The specification states only the observable behavior and guarantees, not the internal helpers that implement them.

- **Issuance (on sign-in / first inbound channel message).** On a successful human sign-in the system establishes a session and returns an opaque session identifier to the client. When an inbound message first arrives from a messaging platform, a session is likewise established for that platform user. Identifiers are long, cryptographically random, and unguessable, and each is unique.
- **Re-identification (per request).** Presenting a valid session identifier re-identifies the bearer (user identity, display label, role, team, and — for channel customers — platform and platform-side user identity). Re-identification never extends the session's lifetime.
- **Expiry.** A session is valid until its expiration window elapses, after which it is automatically treated as not-found. Default windows: about 24 hours for the human sign-in path and about 30 days for the general/extended path; callers may supply a different window. Updating session attributes or extending a session resets the window.
- **Invalidation (on sign-out / revoke).** Sign-out invalidates the session; afterward validation reports not-found. Invalidation is idempotent. A security event (for example a password change) can revoke every session belonging to a user at once.
- **Bounded invalidation propagation.** Immediately after invalidation a previously successful validation may remain effective for at most about 5 minutes, until the invalidation has fully propagated; once propagated the session is rejected.
- **Resume (channel customers).** The most recent session for a given platform user can be located so an ongoing customer interaction resumes rather than starting fresh.
- **Maintenance.** Administrative facilities exist to enumerate a user's sessions and to remove records that can no longer be read; well-formed expired sessions are left to automatic expiry.

**Validation outcomes (observable):**
- A present, well-formed, unexpired session -> accepted, with the bearer's identity attached.
- An empty or blank identifier -> invalid-format rejection.
- An identifier with no live session (never created, expired, or invalidated-and-propagated) -> not-found rejection.
- A stored session that cannot be read -> invalid-format rejection.

### Data Concepts (neutral) — authentication session
A persisted **login-session record** carries: the owning user identifier; a human display label; optional contact email; a role (administrator, agent, or channel customer); an optional team association; for channel customers a platform name (line / facebook / whatsapp), a platform-side user id, and an internal customer id; a creation timestamp; optional client context (originating IP, user-agent string); and an optional free-form metadata bag. Records are addressed by an opaque random session identifier. Channel-customer records additionally encode platform + platform-user + creation-time within their identifier so the latest one can be located by prefix. A session has no separately observable last-activity field; its lifetime is governed entirely by the expiration window set when it is created, updated, or extended.

### State & Lifecycle — authentication session
- Created (on sign-in or first inbound channel message) -> Valid (re-identifies the bearer on each request, read-only) -> terminal removal by either (a) automatic expiration when the window elapses, or (b) explicit invalidation on sign-out / revoke. There are no other states; a session is either resolvable or not. Updating or extending re-persists and resets the expiration window.

---

## Part B — Conversation-Session Management (HTTP module, base path `/api/sessions`)

All conversation-session endpoints require a valid bearer token (JWT in `Authorization: Bearer <token>`); a missing/malformed header or invalid/expired token yields **401** with a generic auth-failure message. The role recognized is a two-tier system: **admin** and **agent** (a "team"/customer notion appears in documentation but enforcement is admin-vs-agent). Mutating and creating endpoints additionally enforce a per-client rate limit of **60 requests / 60 seconds** under a "session" namespace; exceeding it returns **429** with `Retry-After` and `X-RateLimit-*` headers. Request bodies larger than **1 MB** (by declared content length) return **413**. Standard success envelope: `{ success: true, data, [message], [count], timestamp }`. Standard failure envelope: `{ success: false, error, timestamp }`. Identifiers for sessions and conversations are validated as UUID (v1–v5) format where they appear as path/query/body inputs; a malformed id returns **400**.

### Operations

#### Module health — GET /api/sessions/health
- Invocation: open (no auth middleware on this route).
- Success Output (200): static health payload (status, module name, version).

#### Module info — GET /api/sessions/info
- Invocation: open.
- Success Output (200): module descriptor including a feature list, an endpoint catalog, and a permission summary.

#### Create conversation session — POST /api/sessions
- Authorization: admin or agent (any other role -> 403). Rate-limited; size-limited.
- Inputs (body): `conversationId` (UUID, required); `senderType` (required, one of customer/agent/system); optional `sessionType` (continuous/scheduled/support/marketing, defaults to continuous); optional `topic` (sanitized, max 200 chars); optional `messageContent` (sanitized, max 2000 chars; if present and no topic given, a topic is auto-derived from it); optional `priority` (low/medium/high/urgent); optional `tags` (array, max 10 items, sanitized, empties dropped); optional `metadata` object.
- Behavior: a new active session is created for the conversation with start time and last-activity set to now, zero message count, and a derived or supplied topic.
- Success Output (201): the created session object.
- Error Conditions: missing/invalid `conversationId` -> 400; invalid `senderType` -> 400; invalid `sessionType`/`priority` -> 400; topic/messageContent over length -> 400; tags not an array or >10 -> 400; unparseable JSON -> 400; create failure -> standard error response.

#### List conversation sessions — GET /api/sessions
- Authorization: admin or agent (else 403).
- Inputs (query, all optional): `conversationId` (UUID); `isActive` (true/false); `sessionType`; `priority`; `sentiment` (positive/negative/neutral); `startDate`/`endDate` (ISO-8601); `topic`, `tag` (sanitized); `page` (1–1000, default 1); `pageSize` (1–100, default 20).
- Behavior: returns a filtered, paginated list of sessions with pagination metadata (page, page size, total, total pages, has-next/has-prev) and an aggregate summary (totals, active/inactive counts, breakdown by type and priority).
- Success Output (200): list + pagination + summary.
- Error Conditions: any invalid filter value (bad UUID, bad enum, bad date, out-of-range page/pageSize) -> 400.

#### Search conversation sessions — GET /api/sessions/search
- Authorization: admin or agent (else 403).
- Inputs (query): `query` (required, sanitized, min 2 chars); optional `conversationId` (UUID); optional `sessionType`; optional `limit` (1–100, default 20).
- Behavior: returns sessions matching the search term subject to optional filters.
- Success Output (200): array of sessions plus a `count`.
- Error Conditions: missing query / query under 2 chars -> 400; invalid conversationId or sessionType or limit -> 400.

#### Get session details — GET /api/sessions/:sessionId
- Authorization: admin or agent. Access is additionally scoped at the service level: admins may view any session; an agent may view a session only if the underlying conversation is assigned to a team the agent belongs to.
- Inputs (path): `sessionId` (UUID).
- Behavior: returns the session if it exists and the caller is permitted.
- Success Output (200): the session object.
- Error Conditions: invalid id -> 400; not found OR access denied -> **404** (the two cases are deliberately indistinguishable to the caller).

#### Update session — PUT /api/sessions/:sessionId
- Authorization: admin may update any; an agent may update only sessions whose conversation belongs to one of the agent's teams (otherwise 403). Rate-limited; size-limited.
- Inputs (path) `sessionId` (UUID); (body) at least one of: `topic` (sanitized, max 200, nullable), `sessionType` (enum), `endTime` (ISO date or null), `isActive` (boolean), `priority` (enum), `sentiment` (enum), `tags` (array max 10), `metadata`.
- Behavior: applies the provided fields to the session and returns the updated record.
- Success Output (200): updated session, success message.
- Error Conditions: empty body -> 400; invalid enum/date/boolean/tags -> 400; session not found -> error (not-found semantics); unparseable JSON -> 400.

#### Delete session — DELETE /api/sessions/:sessionId
- Authorization: **admin only** (agents -> 403 with an admin-only message). Rate-limited.
- Inputs (path): `sessionId` (UUID).
- Behavior: removes the session permanently (hard delete of the conversation-session record).
- Success Output (200): `{ deleted: true, sessionId }`, success message.
- Error Conditions: invalid id -> 400; not found / could not delete -> **404**.

#### Close session — POST /api/sessions/:sessionId/close
- Authorization: admin or agent-with-team-access (else 403). Rate-limited.
- Inputs (path): `sessionId` (UUID).
- Behavior: marks the session inactive and records an end time of now.
- Success Output (200): `{ closed: true, sessionId }`, success message.
- Error Conditions: invalid id -> 400; not found / not closable -> **404**.

#### Reopen session — POST /api/sessions/:sessionId/reopen
- Authorization: admin or agent-with-team-access (else 403). Rate-limited.
- Inputs (path): `sessionId` (UUID).
- Behavior: marks the session active again, clears the end time, and refreshes last-activity to now.
- Success Output (200): `{ reopened: true, sessionId }`, success message.
- Error Conditions: invalid id -> 400; not found / not reopenable -> **404**.

#### Get session messages — GET /api/sessions/:sessionId/messages
- Authorization: admin or agent (else 403).
- Inputs (path) `sessionId` (UUID); (query) `page` (default 1), `pageSize` (default 20).
- Behavior: returns the messages associated with the session, paginated, with message count and pagination metadata.
- Success Output (200): session id, messages array, message count, pagination block.
- Error Conditions: invalid id -> 400.

#### Session health check — GET /api/sessions/:sessionId/health
- Authorization: admin or agent (else 403).
- Inputs (path): `sessionId` (UUID).
- Behavior: returns a health assessment of the session: a healthy flag plus lists of detected issues and suggestions. Issues are raised when an active session has run very long (beyond ~48 hours), has an excessive message count (over ~100), or has been inactive beyond the configured inactivity threshold (default 60 minutes).
- Success Output (200): `{ healthy, issues[], suggestions[] }`.
- Error Conditions: invalid id -> 400; session not found -> error (not-found semantics).

#### Update session topic — PUT /api/sessions/:sessionId/topic
- Authorization: admin or agent-with-team-access (else 403). Size-limited.
- Inputs (path) `sessionId` (UUID); (body) `topic`.
- Behavior: sets the session's topic.
- Success Output (200): success message.
- Error Conditions: invalid id -> 400; session not found -> **404**.

#### Overall session statistics — GET /api/sessions/stats
- Authorization: **admin only** (else 403).
- Inputs (query): optional `conversation_id` to scope stats to one conversation.
- Behavior: returns aggregate statistics (totals, active/inactive, average messages per session, average duration in minutes, breakdowns by type/priority/sentiment, topic distribution with percentages, and per-day stats).
- Success Output (200): statistics object.

#### Per-conversation session statistics — GET /api/sessions/stats/:conversation_id
- Authorization: admin only (else 403).
- Inputs (path): `conversation_id` (UUID).
- Behavior: same statistics scoped to one conversation.
- Success Output (200): statistics object plus the conversation id.
- Error Conditions: invalid id -> 400.

#### Activity statistics — GET /api/sessions/activity
- Authorization: admin only (else 403).
- Inputs (query): optional `conversation_id`; `timeRange` one of day/week/month/year (default week).
- Behavior: returns time-bucketed activity (sessions created/ended, messages sent, active time per bucket) plus a summary (totals, averages, peak/least activity hour).
- Success Output (200): activity statistics object.
- Error Conditions: `timeRange` not in the allowed set -> 400.

#### Batch operation — POST /api/sessions/batch
- Authorization: **admin only** (else 403). Rate-limited; size-limited.
- Inputs (body): `sessionIds` (non-empty array, max 100, each UUID); `action` one of close / reopen / update_priority / add_tags / remove_tags / delete; `data` object required for priority/tag actions (must contain `priority` for update_priority, `tags` for tag actions; may contain endTime/isActive).
- Behavior: applies the chosen action to each listed session; per-item success/failure is collected.
- Success Output (200): result with total requested, success count, failed count, and a per-session results array; message noting the action.
- Error Conditions: empty/oversized sessionIds, any invalid id, invalid action, or missing required `data` -> 400.

#### Get-or-create (boundary-aware) — POST /api/sessions/get-or-create
- Authorization: admin or agent (create permission). Rate-limited; size-limited.
- Inputs (body): `conversation_id`, `messageContent`, `senderType` — all required.
- Behavior: finds the conversation's most-recently-active session and runs boundary detection against the incoming message. If there is no active session, or detection says a new segment should start, the prior active session (if any) is closed and a fresh session is created (carrying any suggested topic); otherwise the existing session's last-activity is refreshed and it is returned.
- Success Output (200): the resulting session object.
- Error Conditions: any of the three required fields missing -> 400.

#### Detect boundary (analysis only) — POST /api/sessions/detect-boundary
- Authorization: admin or agent (view permission). Size-limited.
- Inputs (body): optional `currentSessionId`; `messageContent` (required); `senderType` (required).
- Behavior: evaluates whether the incoming message should start a new conversation segment relative to the given current session (if any), without creating anything.
- Success Output (200): a detection result: a should-create-new flag, a reason code, a confidence value (0–1), an optional suggested topic, and optional supporting metadata.
- Error Conditions: missing messageContent or senderType -> 400.

#### Topic statistics — GET /api/sessions/topics/stats
- Authorization: admin or agent (view). Inputs: optional `conversation_id`. Output (200): topic statistics.

#### Analyze message topic — POST /api/sessions/topics/analyze
- Authorization: admin or agent (view). Size-limited. Inputs (body): `messageContent` (required). Behavior: derives a topic from the message text. Output (200): topic result. Error: missing content -> 400.

#### Suggest topics — POST /api/sessions/topics/suggest
- Authorization: admin or agent (view). Size-limited. Inputs (body): `messageContent` (required); optional `limit` (default 3). Output (200): suggestions array plus `count`. Error: missing content -> 400.

#### Unmatched route handling
- Any unknown path under the module returns **404** with a list of available endpoints. Unhandled internal errors return **500** with a generic module error message.

### Data Concepts (neutral) — conversation session
A **conversation session** is a bounded segment of one conversation. It carries: a unique session identifier; the parent conversation identifier; a session type (continuous / scheduled / support / marketing); an optional topic label; a start time; an optional end time; a last-activity time; a message count; an active/inactive flag; a creation time; optional update time; optional sentiment (positive / negative / neutral); optional priority (low / medium / high / urgent); an optional tag list; and an optional metadata bag. An associated **session message** carries: its own identifier; the owning session and conversation identifiers; sender identity and sender type (customer/agent/system); content; a message type (text/image/video/file/system); a per-session ordering position; an optional platform-side message id; a creation time; and optional metadata. Deletion of a conversation session is a hard removal (no soft-delete marker on this entity), distinct from "close" which only flips the active flag and stamps an end time.

### State & Lifecycle — conversation session
- **Active** (created via create or get-or-create): the live segment for its conversation; last-activity advances as messages arrive.
- **Closed/Inactive** (via close, or automatically when get-or-create starts a new segment, or via batch close): inactive flag set, end time stamped. May be returned to Active via reopen (clears end time, refreshes last-activity).
- **Deleted** (terminal): hard-removed via delete or batch delete; no longer retrievable.
- Boundary-driven transition: when a new message arrives for a conversation, the system decides whether to keep extending the current active session or close it and open a new one. The decision reasons are: first-session (no current session, confidence ~1.0), time-gap (current session idle beyond the configured threshold, default 30 minutes), message-limit (current session reached the configured max messages, default 50), duration-limit (current session older than the configured max duration, default 24 hours), and topic-change (a customer message containing configured topic-change cue phrases while topic detection is enabled). If none apply, the current session continues.

### Real-time / Event Behavior
This area itself does not broadcast WebSocket events; its observable side effects are state changes (session created/closed/reopened/deleted/updated, last-activity advanced, message count incremented) that other areas may surface. Authentication-session creation/deletion are the persisted side effects of sign-in/sign-out and of channel-customer onboarding. Login additionally records an audit activity entry referencing the new session identifier.

## Authorization, Roles & Permission Model

### Purpose
This area governs who may invoke each operation in the system and which resources they may touch. It defines a two-tier system-wide role model (Administrator and Agent) layered on top of a three-tier per-team role model (Member, Lead, Supervisor). It enforces authentication of API and real-time connections, resolves a caller's effective team memberships and team roles, scopes resource visibility to the caller's teams, and produces consistent denial responses. All checks are designed to fail closed: when required authorization information (credential, membership, or permission data) is missing or unavailable, access is denied or the request is rejected.

### Operations

#### Authenticated API request gate (bearer-token middleware) — applied to protected HTTP routes
- Invocation: Runs ahead of any protected HTTP handler for every non-preflight request.
- Inputs:
  - An `Authorization` request header carrying a bearer credential (required). Preflight (OPTIONS) requests bypass the gate entirely and pass through.
  - Optional `X-Context-Team-ID` request header: an integer identifying which of the caller's teams the request should operate within.
- Preconditions & Authorization: A syntactically valid bearer credential must be present and must be of the access-credential kind. The associated account must exist and be active.
- Behavior (observable order):
  1. Requests without a bearer credential, or with a malformed authorization header, are rejected.
  2. The credential is cryptographically verified. A credential whose declared kind is "refresh" is rejected outright — refresh credentials cannot be used to call protected APIs. Credentials minted without a declared kind are accepted (legacy compatibility).
  3. A credential bearing a unique token identifier that has been revoked is rejected. If revocation state cannot be read, the request is rejected with a service-unavailable result (fail-closed) so revocation cannot be silently bypassed during an outage. Credentials with no unique identifier skip this check.
  4. The account record is loaded; an inactive account is rejected.
  5. The caller's current team memberships, per-team team-roles, and primary team are re-resolved from authoritative storage (short-lived cached, ~60 seconds) rather than trusting whatever was embedded in the credential at sign time. This causes membership/role changes by an Administrator to take effect within about a minute. If authoritative storage shows no team rows but the credential carried a primary team, that primary team is used as a last-resort fallback (covers Administrators who may belong to no team).
  6. The requested team context is resolved: if `X-Context-Team-ID` is supplied and parses to an integer, the caller must be an Administrator or a member of that team for it to take effect; otherwise the supplied context is ignored (not an error) and the request proceeds. With no valid context supplied, the caller's primary team becomes the context.
  7. The resolved account, verified credential claims, and resolved team context are made available to downstream handlers. A best-effort, deduplicated "last active" update is scheduled for the account; its failure never affects the request.
- Success Output: No direct response; control passes to the protected handler with caller identity and team context resolved.
- Error Conditions:
  - Missing/malformed authorization header -> 401 with a missing-authorization message.
  - Refresh-kind credential used as an access credential -> 401.
  - Revoked credential -> 401 with a revoked message.
  - Revocation-state read failure (for identifier-bearing credentials) -> 503 with error code `REVOCATION_CHECK_FAILED`.
  - Inactive account -> 401.
  - Invalid/expired/unverifiable credential or any other failure -> 401 with an invalid-or-expired message including a reason string.
- Invariants & Guarantees: Preflight always bypasses. Team membership/roles are eventually consistent within ~60 seconds. The credential-kind and revocation checks fail closed. Supplying a team context the caller cannot access is silently ignored, never fatal.

#### Session-credential request gate (alternate session-based middleware) — applied where used
- Invocation: Runs ahead of handlers that use session-identifier authentication instead of bearer credentials.
- Inputs: A session identifier supplied either as the `X-Session-ID` header or the `sessionId` query parameter (required).
- Preconditions & Authorization: The session identifier must resolve to a live session in the session store, and its account must be active.
- Behavior: Missing identifier is rejected; an unresolved/expired session is rejected; an inactive account is rejected; otherwise the account and session data are exposed to the handler.
- Error Conditions: Missing identifier -> 401 (missing session). Unknown/expired session -> 401 (invalid/expired session). Inactive account -> 401. Other failures -> 401 with a reason string.

#### Optional authentication gate — applied to mixed public/authenticated routes
- Invocation: Runs ahead of handlers that behave differently for signed-in vs anonymous callers.
- Inputs: Optional `Authorization` bearer header.
- Behavior: If a valid bearer credential is present and its account is active, the account is attached to the request; any verification failure is ignored and the request continues anonymously. Never blocks.
- Success Output: Control always passes to the handler; identity may or may not be present.
- Error Conditions: None surfaced to the caller; all authentication failures are swallowed.

#### System-to-system key gate — applied to protected system routes
- Invocation: Runs ahead of handlers intended for system-to-system calls.
- Inputs: An `X-API-Key` request header (required).
- Preconditions: A server-side configured key must exist.
- Behavior: Missing key is rejected; if no key is configured server-side the request is rejected as misconfigured; a mismatched key is rejected; a matching key passes.
- Error Conditions: Missing key -> 401. No key configured server-side -> 500 (fail-closed, "not configured"). Mismatched key -> 401. Other failures -> 401.

#### Require exact system role — guard composed onto routes
- Invocation: Guard placed after authentication on routes restricted to a specific system role.
- Inputs: A required system role value chosen at route-definition time.
- Behavior: No authenticated caller -> rejected. Administrator always passes (Administrator satisfies any required role). Otherwise the caller's system role must equal the required role exactly.
- Error Conditions: No caller -> 401 (authentication required). Role mismatch -> 403 with payload fields naming the required role and the caller's current role.

#### Require minimum system role level — guard composed onto routes
- Invocation: Guard placed after authentication on routes that require at least a given system role level.
- Inputs: A required minimum system role.
- Behavior: Reads the caller from either the standard identity slot or an alternate identity slot (to support different upstream gates). No caller -> rejected. Otherwise the caller's system role level must be greater than or equal to the required level (Administrator outranks Agent). The convenience guards for "manager-or-administrator" and "administrator-only" both currently require the Administrator level.
- Error Conditions: No caller -> 401 (authentication required), with diagnostic fields. Insufficient level -> 403 with fields naming required role, current role, and caller diagnostics.

#### Require team access — guard composed onto team-scoped routes
- Invocation: Guard placed after authentication on routes addressing a specific team via a path parameter (default parameter name "teamId").
- Inputs: The team identifier from the named path parameter.
- Behavior: No caller -> rejected. Administrator passes immediately (access to all teams). The path team identifier must parse to an integer. The caller must be a member of the named team — determined from the caller's resolved set of accessible team identifiers, with a fallback lookup against authoritative membership storage only for callers whose accessible-team set is empty (legacy credentials); if no fallback store is available, access is denied.
- Error Conditions: No caller -> 401. Non-integer team identifier -> 400 (invalid team). Caller not a member -> 403 with fields naming the caller's primary team and the requested team.
- Invariants: Membership is determined from the freshly-resolved accessible-team set; admins bypass.

#### Require minimum team role — guard composed onto team-management routes
- Invocation: Guard placed after authentication on routes that require a minimum per-team role within a specific team (path parameter, default name "id").
- Inputs: A required minimum team role (Member, Lead, or Supervisor) and the team identifier from the named path parameter.
- Behavior: No caller -> rejected. Administrator bypasses all team-role checks. The path team identifier must parse to an integer. The caller's role within that specific team must rank at or above the required team role (ordering: Member < Lead < Supervisor). A caller who is not a member of that team is treated as having no team role and is denied.
- Error Conditions: No caller -> 401. Non-integer team identifier -> 400 (invalid team). Insufficient team role -> 403 with a human-readable message plus fields naming the team identifier, required team role, and the caller's current team role (or "none").

#### Require team operation permission — guard composed onto team-management routes
- Invocation: A convenience wrapper around the minimum-team-role guard that maps a named team operation to its required minimum team role.
- Inputs: A named operation drawn from the team-operation permission matrix, plus the team identifier path parameter.
- Behavior: Resolves the named operation to its required minimum team role, then applies the minimum-team-role check. Operation-to-role mapping:
  - View team / view members / view statistics -> requires Member or higher.
  - Add member / update member / remove member -> requires Lead or higher.
  - Update team settings / delete team / manage team QR codes / transfer members -> requires Supervisor or higher.
- Error Conditions: Same as the minimum-team-role guard.

#### Authorization decision: capability check (which caller may perform which action on which resource)
- Invocation: Applied whenever a caller attempts an action on a resource (optionally a specific resource instance); the decision determines whether the request proceeds or is rejected.
- Inputs: A caller identifier (string or numeric); a resource name; an action name; and an optional context carrying a target team identifier, a target resource identifier, and metadata such as a resource owner identifier.
- Preconditions: Caller identifier, resource, and action must all be present; a numeric caller identifier must be positive. The caller account must exist and be active; otherwise the action is denied (fail-closed).
- Behavior (observable outcomes):
  - Administrator callers are granted every action on every resource unconditionally.
  - Agent callers are granted only the capabilities in their fixed capability set, each subject to a condition:
    - View a conversation — only if the conversation is unassigned (shared pool) or assigned to one of the caller's teams.
    - Reply to a conversation — same team-assignment condition as viewing.
    - Send a message — same team-assignment condition.
    - Recall a message — only if the caller owns the target item (owner identifier in context must match the caller).
    - Add a tag — only when the target team in context equals the caller's primary team.
  - An action not present in the capability set is denied.
  - For conditioned capabilities, if the required context is absent, the check fails. For the conversation-assignment condition specifically: if the conversation does not exist or has no team assignment, access is granted (shared pool); if it is assigned, access is granted only when the assignment matches the caller's primary team; storage errors during this lookup result in denial.
- Output: An allow or deny decision. A denial causes the attempted operation to be rejected — typically HTTP 403 for a direct action, or omission of the affected items for a listing.
- Error Conditions: Denies on invalid inputs, unknown/inactive account, unknown role, missing capability, or unsatisfied condition; a storage error while evaluating a condition also results in denial. An unexpected storage error during the account lookup surfaces as an error to the caller.
- Invariants: Fail-closed throughout. The view condition deliberately mirrors the reply/send team condition so that an Agent cannot read arbitrary conversations outside their team scope (unassigned conversations remain a shared pool visible to all Agents).

#### Authorization decision: visible-conversation resolution (which conversations a caller may see)
- Invocation: Applied by any conversation-scoped listing to determine the set a caller is permitted to see.
- Inputs: A caller identifier.
- Behavior: Unknown caller -> empty result. Administrator -> all conversations. Agent -> the union of: every unassigned conversation (shared pool) and every conversation assigned to any team the caller belongs to (multi-team aware). Results are ordered by most-recently-updated first.
- Success Output: A list of conversation identifiers the caller may see.
- Invariants: Multi-team membership is honored — an Agent sees conversations for all of their teams, not only their primary team.

#### Real-time connection gate (WebSocket upgrade) — applied to the real-time connect path
- Invocation: Runs at WebSocket upgrade time, before the socket is accepted. Because browsers cannot send custom headers on upgrade, the credential travels as a query parameter.
- Inputs (query parameters): `token` (the bearer credential, required); optional `conversationId` (the conversation the client wants to join); optional `deviceId`.
- Preconditions & Authorization: A present, well-formed, verified, non-expired credential carrying a valid account identifier and a valid system role. For Agents requesting a specific conversation, that conversation must be within the Agent's accessible set.
- Behavior (observable order):
  1. Missing `token` -> rejected.
  2. Credential that is not three dot-separated segments -> rejected (format error).
  3. Credential fails cryptographic verification -> rejected.
  4. Credential already expired -> rejected.
  5. Credential expiring within 30 seconds -> rejected (client must refresh first). Credential expiring within 5 minutes is allowed but logged as a soon-to-expire warning.
  6. Account identifier missing/zero -> rejected (invalid user data).
  7. System role not one of the two valid roles -> rejected (invalid role). When the credential omits a role it defaults to Agent.
  8. The verified identity (identifier, contact email, display label, role, primary team, team label) and verified claims are attached to the connection.
  9. If a target conversation is requested and the caller is an Agent, conversation access is authorized: Administrators are always allowed; Agents are allowed only if the conversation is in their accessible set (their team-assigned conversations plus the unassigned shared pool, cached ~5 minutes). Denied access -> rejected.
  10. On success the upgrade proceeds; a connection-quality analytics record is written best-effort (its failure does not affect the connection).
- Success Output: The upgrade proceeds and the socket is accepted; no JSON body.
- Error Conditions (each returns a JSON body with an `error` message, a numeric `code`, an `X-Error-Code` header, and an `X-WebSocket-Close-Code` header):
  - Missing token -> HTTP 401, code 4401, `NO_TOKEN`.
  - Malformed token shape -> HTTP 401, code 4402, `INVALID_TOKEN_FORMAT`.
  - Invalid/unverifiable token -> HTTP 401, code 4403, `INVALID_TOKEN`.
  - Expired token -> HTTP 401, code 4404, `TOKEN_EXPIRED`.
  - Token expiring within 30 seconds -> HTTP 401, code 4405, `TOKEN_EXPIRING_SOON`.
  - Invalid/zero account identifier -> HTTP 401, code 4406, `INVALID_USER_DATA`.
  - Invalid role -> HTTP 401, code 4407, `INVALID_ROLE`.
  - Conversation access denied (Agent) -> HTTP 403, code 4403, `CONVERSATION_ACCESS_DENIED`.
  - Unexpected server failure -> HTTP 500, code 4500, `AUTH_SYSTEM_ERROR`.
  - All error bodies also include a timestamp and a machine-readable suggested next action (e.g. provide token, refresh token, contact administrator, retry with new token).
- Invariants: Conversation-level authorization applies only to Agents; Administrators bypass it. The accessible-conversation set is cached briefly and must be invalidated when assignments change for the cache to reflect new access.

### Data Concepts (neutral)
- **Account / agent**: A user of the system. Carries a unique identifier, a contact email, a display label, a system role (Administrator or Agent), an active/inactive flag, and timestamps. Inactive accounts are denied authentication everywhere.
- **System role**: Exactly two values — Administrator (rank 2) and Agent (rank 1). Administrator is a wildcard: it satisfies every system-role requirement, every team requirement, every team-role requirement, and every capability check. A formerly-existing intermediate "team" role has been removed; only these two remain. A role manager can only manage roles strictly below its own rank (an Administrator can manage Agent; an Agent can manage none).
- **Team**: A grouping identified by an integer. Conversations may be assigned to a team or left unassigned.
- **Team membership**: The association between an account and a team, carrying the account's per-team role and a flag marking exactly one membership as the account's primary team. An account may belong to multiple teams (multi-team). The primary team is the default operating context.
- **Per-team (team) role**: Three ranked values — Member (rank 1, view-only), Lead (rank 2, may manage members), Supervisor (rank 3, may manage team settings). Determined per team; an account can hold different team roles in different teams. A non-member is treated as having no team role.
- **Resolved caller context**: For each authenticated request the system derives the accessible-team set, a per-team role map, and a resolved current-team context (chosen team or primary team).
- **Credential**: A signed token carrying the account identifier, system role, primary team, optional team label, an expiry, an optional kind ("access" vs "refresh" vs other), and an optional unique token identifier used for revocation. Only access-kind (or kind-less legacy) credentials may call protected APIs.
- **Revocation state**: a credential bearing a unique token identifier may be marked revoked; while marked, it is rejected on every subsequent call. A revocation marking need only persist until the credential would have expired; sign-out marks the credential revoked for its remaining life.
- **Team-operation permission matrix**: A fixed mapping from named team operations to the minimum team role required (view operations -> Member; member-management operations -> Lead; team-settings/destructive/QR/transfer operations -> Supervisor).

### State & Lifecycle
- **System role ranking**: Agent (1) < Administrator (2). A "minimum role" requirement is satisfied when the caller's rank is greater than or equal to the requirement; Administrator satisfies all. "Manageable roles" are strictly-lower-ranked roles.
- **Team role ranking**: Member (1) < Lead (2) < Supervisor (3). A "minimum team role" requirement is satisfied when the caller's rank within that specific team meets or exceeds it. Administrator bypasses team-role ranking entirely.
- **Account active state**: Active accounts authenticate; inactive accounts are denied at every gate. Deactivation takes effect on the next authentication.
- **Membership freshness**: Membership and team-role changes made by an Administrator are reflected in authorization within roughly 60 seconds (a short membership cache), independent of credential lifetime.
- **Credential lifecycle**: A credential is honored until it expires unless it has been revoked, at which point it is rejected immediately on subsequent calls. Refresh-kind credentials are never accepted on protected APIs regardless of validity.

### Real-time / Event Behavior
- The real-time connection gate consumes a credential (as a query parameter) and a requested conversation identifier, and either accepts the upgrade or returns a structured rejection (HTTP status plus a numeric WebSocket close code 4401–4500, an `X-Error-Code`, an `X-WebSocket-Close-Code`, a timestamp, and a suggested next action). These codes are the observable contract a client uses to decide whether to refresh its credential, retry, or escalate.
- On a successful real-time authentication, a connection-quality analytics record is emitted; on an internal authentication failure, an error analytics record (type AUTH_SYSTEM_ERROR, code 4500) is emitted. Both are best-effort and never alter the connection result observed by the client.
- The Agent accessible-conversation set used for real-time authorization is cached briefly; an explicit cache-invalidation action exists and must be triggered when conversation/team assignments change so that newly-granted or newly-revoked conversation access becomes effective for real-time connections.


---

# 2. Conversations & Messaging

## Conversations (Agent Side)

### Purpose
This area gives authenticated support staff the operational interface for working with customer conversations: discovering and filtering the conversations they are allowed to see, reading conversation detail and message history, routing conversations between support teams (assign / unassign / transfer), marking a conversation as read, managing the descriptive labels attached to a conversation, sending outbound replies (including file attachments) that are delivered asynchronously to the customer's messaging platform, and performing the same routing/labeling actions across many conversations at once. All operations are scoped by a team-based visibility model so an agent only acts on conversations in their own team(s) or in the unassigned shared pool.

### Behavioral Boundary (Under-specified)
The operations documented in this area constitute the entire conversation-routing contract. The following lies outside the current observable behavior and induces no state change within this boundary; a conforming implementation must reproduce the documented operations but is not required to provide it:
- **Automatic conversation assignment.** A conversation is routed to a team only through the explicit, caller-initiated assign / unassign / transfer operations described below. Within the current system boundary there is no automatic, rule-based, skill-based, load-balanced, or round-robin assignment behavior: an unassigned conversation remains in the shared pool until a caller acts on it.

### Operations

#### List conversations — GET /
- Invocation: authenticated client request.
- Inputs (all query parameters, all optional):
  - `tagIds`: comma-separated list of integer label identifiers. Non-numeric entries are ignored. When present, results are restricted to conversations matched by any of those labels (matched either directly on the conversation, or indirectly because the conversation's customer carries one of those labels).
  - `search`: free-text string, trimmed; restricts results to conversations whose customer display name contains the text (case-insensitive substring).
  - `customerName`: free-text string, trimmed; additional customer-display-name substring filter applied during the main fetch.
  - `updatedAfter`: timestamp string lower bound (inclusive) on the conversation's last-updated time.
  - `updatedBefore`: timestamp string upper bound (inclusive) on the conversation's last-updated time.
- Preconditions & Authorization: requires valid authentication. No explicit per-conversation permission call; instead the caller's visible-conversation set is computed and used as the universe. Admins see all conversations; agents see the union of (a) conversations assigned to none (shared/unassigned pool) and (b) conversations assigned to any team the agent belongs to.
- Behavior: computes the caller's visible set, applies the label filter, then the search filter, then the customer-name / updated-window filters, and returns the surviving conversations ordered by most-recently-updated first. For each returned conversation it also computes a preview of the single most recent message and a count of unread customer messages. If the visible set is empty, or any filter stage eliminates all candidates, an empty list is returned (still a success).
- Success Output: `200` with `{ success: true, data: [ ... ], timestamp }`. Each list item carries: the conversation's own fields (identifier, assigned-team identifier, status, priority, timestamps, last-message time, last-read time, etc.); a nested `customer` object (identifier, `name` and `displayName` duplicates, platform, platform user identifier, avatar URL, creation time) when a customer is linked; a nested `assignedTeam` object (identifier, name, description) when assigned; flattened backward-compatible fields `customerName`, `platform`, `platformUserId`; a `lastMessage` object (`id`, `content`, `createdAt`, `senderType`, `messageType`) or null; `lastMessageContent`, `lastMessageAtActual`, `lastMessageType`; and `unreadCount`.
- Side Effects: none (read-only).
- Error Conditions: unhandled failures return the standard error envelope via the global error handler. Failure to compute message previews or unread counts is swallowed per-conversation (those fields degrade to null / 0) rather than failing the whole request.
- Invariants & Guarantees: ordering is strictly by last-updated descending. "Unread" is defined as customer-sent, non-deleted messages newer than the later of the most recent agent/system reply and the conversation's last-read marker. Large visible sets are processed without exceeding the underlying datastore's bound-parameter limits; partial preview failures yield partial (never wholesale-dropped) previews.

#### Get conversation detail — GET /:id
- Invocation: authenticated client request.
- Inputs: path parameter `id` (conversation identifier).
- Preconditions & Authorization: requires authentication and a "view" permission check on this conversation. Admins always pass. Agents pass only if the conversation is unassigned or assigned to the agent's primary team; otherwise denied.
- Behavior: loads the conversation joined with its team and customer, computes the latest-message preview and the unread customer-message count, and returns the assembled object.
- Success Output: `200` with `{ success: true, data: {...}, timestamp }`. `data` contains the conversation's own fields plus a nested `assignedTeam` (when assigned), a nested `customer` (identifier, `name`/`displayName`, platform user identifier, platform, avatar URL, email, phone, source-team identifier, metadata, timestamps) when linked, a `lastMessage` object or null, `lastMessageContent`, `lastMessageAtActual`, `lastMessageType`, and `unreadCount`.
- Error Conditions: `403` `{ error: "Permission denied" }` if the view check fails; `404` `{ success: false, error: "Conversation not found" }` if no such conversation; otherwise standard error envelope.
- Invariants & Guarantees: returns the same shape used by the assignment endpoints for UI consistency. Preview/unread failures are swallowed (degrade to null/0).

#### Mark conversation as read — PUT /:id/read
- Invocation: authenticated client request.
- Inputs: path parameter `id`.
- Preconditions & Authorization: authentication plus the same "view" permission check as detail retrieval (admin always; agent only for unassigned or own-team conversations).
- Behavior: records the current time as the conversation's last-read marker.
- Success Output: `200` with `{ success: true, data: { lastReadAt }, timestamp }`.
- Side Effects: persists an updated last-read timestamp on the conversation; observable later as a reduced unread count.
- Error Conditions: `403` `{ error: "Permission denied" }`; otherwise standard error envelope. Note: the update is issued even if the conversation does not exist (no existence check), so a missing conversation still returns success.
- Invariants & Guarantees: idempotent in effect; no real-time event is emitted.

#### Assign conversation to a team — POST /:id/assign
- Invocation: authenticated client request.
- Inputs: path parameter `id`; JSON body `{ teamId (required, integer team identifier), reason (optional string) }`. (Individual-agent assignment is not supported; only team assignment.)
- Preconditions & Authorization: authentication plus an "assign" permission check (admin always passes; agent passes per role rules). Conversation must exist.
- Behavior: in order — verifies permission; requires `teamId`; reads prior assignment state; resolves the target team's display name; sets the conversation's assigned team and sets status to the assigned state with a fresh update time; if a reason was supplied, records a routing-history entry capturing the prior team, the new team, the reason, and the actor; commits these as one atomic batch; then broadcasts the assignment in real time; then re-reads and returns the full updated conversation.
- Success Output: `200` with `{ success: true, message: "Conversation assigned successfully", data: {...}, timestamp }`. `data` is the conversation plus nested `assignedTeam` and nested `customer` (the customer's `name` falls back to platform user identifier, then to "Unknown customer").
- Side Effects: persists new assigned team, status, and update time; optionally appends one routing-history record; emits a real-time `conversation_assigned` event; writes a reversible audit entry for the assignment action.
- Error Conditions: `403` `{ error: "Permission denied" }`; `400` `{ error: "Team ID is required for assignment" }` if `teamId` missing; `404` `{ error: "Conversation not found" }`; `500` `{ success: false, error: "Failed to retrieve updated conversation" }` if the post-update re-read fails; otherwise standard error envelope. Real-time broadcast failure is non-fatal (logged, request still succeeds).
- Invariants & Guarantees: the assignment write and audit/history writes are atomic together. Routing history is only written when a reason is provided.

#### Unassign conversation — POST /:id/unassign
- Invocation: authenticated client request.
- Inputs: path parameter `id`; JSON body optional `{ reason (optional string) }` (a missing/invalid body is tolerated).
- Preconditions & Authorization: authentication plus the "assign" permission check. Conversation must exist and must currently be assigned to a team.
- Behavior: loads the conversation and its current team; clears the assigned team and resets status to the active state with a fresh update time; if a reason was supplied, records a routing-history entry (from current team, to none) defaulting the reason to a generic "unassign" label; commits atomically; broadcasts the unassignment; re-reads and returns the updated conversation.
- Success Output: `200` with `{ success: true, message: "Conversation unassigned successfully", data: {...}, timestamp }`, where `data` includes nested `assignedTeam` (now absent) and nested `customer`.
- Side Effects: clears assigned team, sets active status, updates time; optionally appends one routing-history record; emits a real-time `conversation_unassigned` event (high priority); writes a reversible audit entry.
- Error Conditions: `403` `{ error: "Permission denied" }`; `404` `{ error: "Conversation not found" }`; `400` `{ error: "Conversation is not assigned" }` when there is no current team assignment; otherwise standard error envelope. Broadcast failure is non-fatal.
- Invariants & Guarantees: state write plus audit/history writes are atomic.

#### Transfer conversation between teams — POST /:id/transfer
- Invocation: authenticated client request.
- Inputs: path parameter `id`; JSON body `{ fromTeamId (optional integer source team), toTeamId (required integer target team), reason (optional string) }`. (Individual-agent transfer is not supported.)
- Preconditions & Authorization: authentication. Admins bypass the per-conversation check. Non-admins must pass an "assign" permission check. Conversation must exist.
- Behavior: in order — for non-admins, verifies the assign permission; requires `toTeamId`; reads prior state; resolves source and target team display names (source name omitted when no source given); sets the conversation's assigned team to the target and status to the active state with a fresh update time; always records a routing-history entry capturing source team (may be empty), target team, reason, and actor; commits atomically; then broadcasts a dual-team transfer notification (so both the losing and gaining teams are informed); returns success.
- Success Output: `200` with `{ success: true, message: "Conversation transferred successfully", timestamp }`. (The full conversation object is not returned by this endpoint.)
- Side Effects: persists new assigned team, active status, update time; always appends one routing-history record; emits a real-time transfer event to both source and target audiences; writes a reversible audit entry.
- Error Conditions: `403` `{ error: "Permission denied" }` (non-admin without permission); `400` `{ error: "Target team ID is required for transfer" }` if `toTeamId` missing; `404` `{ error: "Conversation not found" }`; otherwise standard error envelope. Broadcast failure is non-fatal.
- Invariants & Guarantees: state write, history write, and audit write are atomic. Routing history is always written for transfers (unlike assign/unassign where it is reason-gated).

#### Get conversation labels — GET /:id/tags
- Invocation: authenticated client request.
- Inputs: path parameter `id`.
- Preconditions & Authorization: authentication only (no per-conversation team check). Conversation must exist.
- Behavior: returns the active labels currently attached to the conversation.
- Success Output: `200` with `{ success: true, data: [ { id, name, color, description, assignedBy, assignedAt } ], message }`. Only labels that are still active are included.
- Error Conditions: `404` `{ success: false, error: "Conversation not found" }`; otherwise standard error envelope.

#### Add conversation labels — POST /:id/tags
- Invocation: authenticated client request.
- Inputs: path parameter `id`; JSON body `{ tagIds (required, non-empty array of label identifiers; numeric strings accepted and coerced to integers) }`.
- Preconditions & Authorization: authentication only. Conversation must exist.
- Behavior: associates each provided label with the conversation, recording the acting user (or a system marker when no user id is present) as the assigner; duplicate associations are ignored. Then broadcasts a label-update event.
- Success Output: `200` with `{ success: true, message: "Tags added to conversation successfully" }`.
- Side Effects: persists new conversation-label associations (idempotent on conflict); emits a real-time `conversation_tags_updated` event with operation "add".
- Error Conditions: validation error response when `tagIds` is missing/empty/not an array; `404` `{ success: false, error: "Conversation not found" }`; otherwise standard error envelope. Broadcast failure is non-fatal.
- Invariants & Guarantees: adding an already-present label is a no-op (no duplicate created).

#### Remove conversation labels — DELETE /:id/tags
- Invocation: authenticated client request.
- Inputs: path parameter `id`; JSON body `{ tagIds (required, non-empty array; numeric strings coerced) }`.
- Preconditions & Authorization: authentication only. Conversation must exist.
- Behavior: removes the specified label associations from the conversation, then broadcasts a label-update event.
- Success Output: `200` with `{ success: true, message: "Tags removed from conversation successfully" }`.
- Side Effects: deletes matching conversation-label associations; emits a real-time `conversation_tags_updated` event with operation "remove".
- Error Conditions: validation error when `tagIds` invalid/empty; `404` if conversation not found; otherwise standard error envelope. Broadcast failure is non-fatal.

#### List conversation messages — GET /:id/messages
- Invocation: authenticated client request.
- Inputs: path parameter `id`; query `page` (integer, default 1, floored at 1) and `pageSize` (integer, default 30, clamped to 1..100).
- Preconditions & Authorization: authentication plus the conversation "view" permission check (admin always; agent only for unassigned or own-team). Conversation must exist.
- Behavior: returns one page of the conversation's messages, newest first, including the sender's display name (customer name for customer messages, agent name for agent messages) and any file attachments associated with each message. For each attachment it produces a signed inline-viewable URL and, where the underlying stored object exists, a separate signed force-download URL.
- Success Output: `200` with `{ success: true, data: { items, page, pageSize, total, totalPages, hasMore }, timestamp }`. Each item carries: identifier, conversation identifier, sender type (customer mapped to "user"; otherwise "agent"/"system"), sender id, sender name, content, media placeholders, media type, platform marker, created time (epoch ms), platform message id, sent flag, delivery status, metadata, sent time, recall flags and deadlines, and a list of file attachments (each with id, filename, mime type, size, inline URL, optional download URL).
- Side Effects: none (read-only).
- Error Conditions: `403` `{ success: false, error: "Permission denied", timestamp }`; `404` `{ success: false, error: "Conversation not found", timestamp }`; otherwise standard error envelope.
- Invariants & Guarantees: messages are returned newest-first; `hasMore` is true when the current page is not the last. Attachment URL signing failures degrade gracefully (download URL becomes absent, inline URL falls back to the stored value).

#### Send a message (asynchronous delivery) — POST /:id/messages
- Invocation: authenticated client request.
- Inputs: path parameter `id`; JSON body `{ content (string; trimmed; required unless attachments are present), senderId (required non-empty string identifying the sending agent), messageType (optional; one of text/image/file/quick_reply, defaults to text), metadata (optional object), attachmentIds (optional array of previously-uploaded attachment identifiers) }`.
- Preconditions & Authorization: authentication plus a "message send" permission check on the conversation. Admins always pass. Agents pass only when the conversation is unassigned or assigned to their primary team; otherwise denied with a role-specific explanation. The linked customer must exist.
- Behavior: validates the request; checks permission; immediately persists the outbound message in a "pending" delivery state, links any referenced attachments to it, and advances the conversation's last-message and update times; then broadcasts the pending message to real-time listeners (both the detail view and the conversation-list preview, scoped to the assigned team); then schedules background delivery to the customer's messaging platform; and returns immediately without waiting for delivery. Background delivery later attempts to push the text and/or attachment content to the platform (batched to the platform's per-call message cap), then updates the stored message's sent flag, delivery status (sent / partial / failed), and platform message id, and broadcasts a message-update event reflecting the final outcome.
- Success Output: `200` (standard success envelope) with `message: "Message queued for delivery"` and a formatted message object reflecting the pending state: identifier, conversation identifier, sender type "agent", sender id and name, content, media fields, platform marker, created/timestamp (epoch ms), delivery status "pending", sent flag false, null platform message id, and parsed metadata.
- Side Effects: persists a new outbound message (initially pending); links attachments to the message; updates conversation last-message/update times; emits a real-time `message_sent` event (pending), a unified new-message broadcast for list/detail updates, and later a `message_updated` event carrying the final delivery status; ultimately attempts outbound delivery to the customer's platform.
- Error Conditions: validation failures (missing content and no attachments, missing sender id, missing conversation id) surface as errors from the validator; `403` with a role-specific message (agents) or "Permission denied" (others) when the send check fails; missing conversation/customer causes message creation to throw and surfaces via the standard error envelope. Background-delivery failures do not change the synchronous success response; they are reflected only via the later `message_updated` event and the persisted "failed"/"partial" status.
- Invariants & Guarantees: the send response is returned before delivery is confirmed (fire-and-forget). Delivery status transitions pending -> sent | partial | failed are observable via real-time updates and on subsequent message reads. Partial success is reported when some but not all platform batches succeed. Only the platform with full outbound support actually delivers; others remain effectively undelivered.

#### Upload a message attachment — POST /:id/attachments
- Invocation: authenticated client request (multipart form upload), performed before sending a message that references the attachment.
- Inputs: path parameter `id`; multipart form field `file` (the binary file).
- Preconditions & Authorization: authentication. Conversation must exist. Team-scope gate: admins always allowed; if the conversation is unassigned, allowed; otherwise the caller's allowed-team set must include the conversation's assigned team. Otherwise forbidden.
- Behavior: validates the file is present and within the size cap (10 MB), makes the binary retrievable through a per-conversation pending reference, records an attachment entry (not yet linked to any message, marked upload-complete), and returns the attachment's identifier and URL for later inclusion in a send call.
- Success Output: `200` with `{ success: true, data: { attachmentId, url, filename, mimeType, size } }`.
- Side Effects: stores a file object; persists an attachment record associated with the conversation and no message (until a later send links it).
- Error Conditions: `400` `{ success: false, error: "Conversation ID is required" }` if id missing; `404` `{ success: false, error: "Conversation not found" }`; `403` `{ success: false, error: "You do not have access to this conversation" }` when the team-scope gate fails; `400` `{ success: false, error: "No file provided" }` for missing/empty file; `400` `{ success: false, error: "File too large (max 10MB)" }` when over the cap; `500` `{ success: false, error: "Failed to upload file to storage" }` on storage failure; otherwise standard error envelope.
- Invariants & Guarantees: an uploaded attachment exists independently of any message until a subsequent send links it.

#### Bulk operations — POST /bulk
- Invocation: authenticated client request.
- Inputs: JSON body `{ operation (required), conversationIds (required non-empty array of conversation identifiers), data (operation-specific object) }`. Supported `operation` values and their required `data`:
  - `assign`: `data.teamId` (required) — assigns all listed conversations to the team and sets them to the assigned state.
  - `set_priority`: `data.priority` (required) — sets priority on all listed conversations.
  - `add_tags`: `data.tagIds` (required array) — associates each label with each conversation (duplicates ignored), recording the actor as assigner.
  - `remove_tags`: `data.tagIds` (required array) — removes those label associations from each conversation.
  - `close` / `reopen`: explicitly rejected as no-longer-supported.
- Preconditions & Authorization: authentication. Every conversation in the list must be within the caller's visible set; any conversation outside it causes the whole request to be denied. (Visibility: admin = all; agent = unassigned pool plus own team(s).)
- Behavior: validates the id list and per-operation data, enforces visibility on every id, performs the requested change across all conversations, and for label operations broadcasts a per-conversation status-change event afterward.
- Success Output: `200` (standard success envelope) with `{ operation, affectedCount, conversationIds }` and message "Bulk <operation> completed successfully".
- Side Effects: depending on operation — bulk team assignment + status change, bulk priority change, bulk label add (idempotent), or bulk label removal. Label operations emit one real-time `conversation_status_changed` event per affected conversation (change type "tags_updated").
- Error Conditions: validation error when `conversationIds` is not a non-empty array; `403` with a message naming how many conversations were unauthorized when any id is outside the caller's visible set; validation error for missing `data.teamId` / `data.priority` / `data.tagIds`; validation error for `close`/`reopen`; validation error listing valid operations for any unrecognized operation; `500` with the error message on unexpected failure. Label-broadcast failures are non-fatal.
- Invariants & Guarantees: a single unauthorized conversation blocks the entire batch (all-or-nothing on the authorization gate). Bulk label addition is idempotent. Operations are chunked internally to respect datastore bound-parameter limits.

### Data Concepts (neutral)
- Conversation: the central record of an ongoing customer dialogue. Carries a unique identifier, an optional assigned-team reference, a lifecycle status, a priority, a customer reference, creation/update timestamps, a last-message time, and a last-read marker. May be linked to a customer and to an assigned team. Supports soft-delete semantics at the platform level (deleted records are excluded from message-related counts).
- Customer: the external party in the conversation. Exposes a display name (surfaced as both `name` and `displayName`), a messaging-platform identifier, the platform itself, an avatar URL, and optionally email, phone, source-team, and metadata.
- Team: a support group that a conversation can be assigned to. Exposes identifier, name, and description.
- Message: an individual entry in a conversation. Carries identifier, conversation reference, sender type (customer / agent / system), sender id and resolved name, content, message type (text/image/video/file/sticker/audio/location), platform message id, sent flag, delivery status (pending/sent/partial/failed/delivered), recall flags and deadlines, metadata, and timestamps. Customer message senders are surfaced to clients as type "user".
- File attachment: a stored file optionally linked to a message and associated with a conversation. Carries identifier, filename, mime type, size, an inline-viewable URL, an optional force-download URL, and an upload-completion state. Exists before linkage; linked to a message at send time.
- Conversation label: a descriptive tag attachable to a conversation, carrying identifier, name, color, description, and association metadata (who assigned, when). Labels can also be inherited indirectly through the conversation's customer for filtering purposes. Only active labels are surfaced.
- Routing-history entry: an immutable record of an assignment/unassignment/transfer, capturing source team (optional), destination team (optional), a reason, the acting user, and a timestamp.
- Reversible audit entry: a record of each assignment/unassignment/transfer that captures prior and new conversation state so the action can later be reversed.

### State & Lifecycle
Conversation status transitions observable through this area:
- Assigning a conversation sets it to the assigned state and attaches a team.
- Unassigning a conversation clears the team and sets it to the active state.
- Transferring a conversation attaches the target team and sets it to the active state.
- Priority is an independent attribute changed only via bulk `set_priority`.
- There is no terminal closed state for a conversation; close and reopen requests are explicitly rejected.
- The last-read marker is advanced by the mark-as-read operation and influences unread counts but is not a status.

### Real-time / Event Behavior
- `conversation_assigned` — emitted after a successful single assignment; payload includes the conversation id, assigning actor, assigned team id and name, reason, and timestamp; delivered to relevant team audiences.
- `conversation_unassigned` — emitted after a successful single unassignment (high priority); payload includes prior team id and name, unassigning actor, reason, and timestamp.
- Conversation transfer event — emitted after a successful transfer; a dual-team notification informing both the losing and gaining teams; payload includes conversation summary (id, customer id and name, platform user id, avatar, platform, status, last-message time, target team), source/target team ids and names, the transferring actor, and reason.
- `conversation_tags_updated` — emitted after single-conversation label add/remove; payload includes the operation ("add"/"remove"), affected label ids, the updating actor, and timestamp.
- `conversation_status_changed` (change type "tags_updated") — emitted once per conversation after bulk label add/remove; payload includes the label operation, affected label ids, the updating actor, and timestamp.
- `message_sent` — emitted immediately when an agent message is created in pending state; payload includes content, message type, sender info, delivery status "pending", and timestamp.
- Unified new-message broadcast — emitted alongside `message_sent` to drive both conversation-detail and conversation-list previews; scoped to the conversation's assigned team to prevent cross-team leakage.
- `message_updated` — emitted by the background delivery process once outbound delivery resolves; payload includes the final delivery status (sent/partial/failed), sent flag, platform message id, optional error, and timestamp. Allows clients to transition a message out of the pending state.
- All real-time broadcasts are best-effort: a broadcast failure is logged and never fails the originating request.

## Messaging

### Purpose
This area provides the agent-facing message lifecycle for a multi-channel customer-support product. It lets authenticated staff create outbound messages into conversations, read individual messages and paginated conversation transcripts, edit and recall their own messages, perform bulk create/recall, attach files, forward messages, tag messages, search/export transcripts, schedule delayed sends, and recall/cancel messages. It also defines the persisted message record (sender, content, type, delivery status, recall state, reply linkage) and the rules governing team-scoped visibility, recall deadlines, and downstream platform notification.

All HTTP endpoints below are mounted under a common base path of `/api/messages`. All require a valid bearer authentication token unless noted (the two informational endpoints do not enforce auth). Identifiers in this area come in two observable shapes: a UUID, or a prefixed token of the form `msg_<digits>_<alphanumeric>`; both are accepted as message identifiers depending on creation path.

### Operations

#### Module health probe — GET /api/messages/health
- Invocation: any client request; no authentication enforced.
- Behavior: returns a static liveness payload.
- Success Output: 200 with `status` ("healthy"), a module label, `version`, and a server `timestamp`.

#### Module capability descriptor — GET /api/messages/info
- Invocation: any client request; no authentication enforced.
- Behavior: returns static descriptive metadata.
- Success Output: 200 with `success: true` and a `data` object listing module name, version, status, a feature list, and a human-readable endpoint list, plus `timestamp`.

#### Create message — POST /api/messages
- Invocation: authenticated client request.
- Inputs (JSON body): `conversationId` (string, required), `content` (string, required, must be non-empty after trimming), `messageType` (string, optional, defaults to a text type), `replyToMessageId` (string, optional — must reference an existing non-deleted message in the same conversation), `metadata` (arbitrary object, optional), `attachmentIds` (array of strings, optional — pre-uploaded attachment identifiers to associate).
- Preconditions & Authorization: caller must be authenticated. The target conversation must exist and not be soft-deleted. Team scope is enforced: an administrator may post to any conversation; any caller may post to a conversation that has no assigned team (shared pool); otherwise the conversation's assigned team must be among the caller's allowed teams.
- Behavior (observable order): validate body; verify conversation exists and is not deleted; enforce team scope; if a reply target is supplied, verify it exists in the same conversation; persist a new message authored by the caller with sender role "agent", marked as sent with delivery status "sent" and the caller's display name captured; bump the conversation's last-activity and updated timestamps; associate any supplied attachment identifiers that are not yet linked to a message; detect @-mentions in the content and fire mention notifications to each mentioned user except the author; record an audit activity entry.
- Success Output: 201 with the new message identifier, conversation identifier, content, message type, sender role, author identifier, sent timestamp, created timestamp, the list of associated attachments, and the list of mentioned user identifiers when any were found.
- Side Effects: new persisted message; conversation timestamps updated; attachment records re-linked; mention notifications dispatched (best-effort, non-blocking); audit log entry (best-effort).
- Error Conditions: malformed JSON or missing/empty `conversationId`/`content` → 400; conversation missing or soft-deleted → 404; team scope violation → 403; invalid reply target → 400; unexpected failure → 500.

#### Get message by identifier — GET /api/messages/:id
- Invocation: authenticated client request.
- Inputs: path identifier of the message.
- Preconditions & Authorization: authenticated. Soft-deleted messages are treated as nonexistent. Team-scoped read: administrators see all; other callers may read only messages whose conversation is unassigned or belongs to one of their allowed teams. A scope violation is deliberately reported as "not found" to avoid disclosing existence.
- Behavior: fetch the message joined with conversation, author (agent) and customer context; enforce team scope; return a composed detail view.
- Success Output: 200 with message fields (identifier, conversation identifier, sender role, content, message type, platform message identifier, recall flag, recall deadline, recalled timestamp, sent flag, sent timestamp, delivery status, reply target, thread/session/sequence markers, parsed metadata, created timestamp), a sender-info sub-object (agent: identifier/name/role; customer: identifier/name/platform; otherwise null), and a conversation-info sub-object (status, priority).
- Error Conditions: missing path identifier → 400; not found or scope-hidden → 404; unexpected failure → 500.

#### Update message — PUT /api/messages/:id
- Invocation: authenticated client request.
- Inputs (JSON body): `content` (string, optional — if present must be non-empty after trimming), `messageType` (string, optional), `metadata` (object, optional).
- Preconditions & Authorization: caller must be the original agent author of the message OR an administrator; customer-origin and system-origin messages are never editable through this endpoint. The message must not already be recalled.
- Behavior: verify existence; positive authorization check; reject if already recalled; apply provided fields and refresh the updated timestamp; return the updated record.
- Success Output: 200 with identifier, conversation identifier, content, message type, parsed metadata, created timestamp, and a success message.
- Error Conditions: missing identifier or malformed JSON → 400; empty content when provided → 400; not found → 404; not author/admin → 403; message already recalled → 400; unexpected failure → 500.

#### Recall (soft-delete) message — DELETE /api/messages/:id
- Invocation: authenticated client request.
- Inputs: path identifier.
- Preconditions & Authorization: caller must be the original agent author OR an administrator; customer/system messages cannot be recalled here. Message must not already be recalled. If a recall deadline is set on the message, the current time must not be past it.
- Behavior: verify existence and authorization; check not-already-recalled and deadline; mark the message recalled, stamp a recall timestamp, and overwrite its content with a fixed "recalled" placeholder marker; record an audit activity entry.
- Success Output: 200 with identifier, conversation identifier, recall flag (true), recall timestamp, and a recalled-by sub-object (caller identifier and display name).
- Side Effects: message content replaced with placeholder and flagged recalled (record retained, not hard-deleted); audit log entry (best-effort).
- Error Conditions: missing identifier → 400; not found → 404; not author/admin → 403; already recalled → 400; deadline passed → 400; unexpected failure → 500.

#### List conversation messages — GET /api/messages/conversation/:conversationId
- Invocation: authenticated client request.
- Inputs: path conversation identifier; query `page` (integer ≥ 1, default 1), `pageSize` (integer 1–100, default 20, clamped), `messageType` (filter), `senderType` (filter), `includeRecalled` ("true" to include recalled messages; otherwise recalled messages are excluded).
- Preconditions & Authorization: authenticated; conversation must exist (no explicit team-scope filter applied on this listing endpoint beyond authentication).
- Behavior: verify conversation exists; count matching messages; return a page ordered by creation time descending (newest first), with optional type/sender filters and recall inclusion.
- Success Output: 200 with `messages` (each: identifier, conversation identifier, sender role, resolved sender name (persisted snapshot preferred, falling back to joined agent/customer name), sender-info sub-object, content, message type, recall flag, recalled timestamp, sent flag, sent timestamp, delivery status, reply target, thread/session/sequence markers, parsed metadata, created timestamp), a `pagination` object (page, pageSize, total, totalPages, hasMore), and an echo of the applied `filters`.
- Error Conditions: missing conversation identifier → 400; conversation not found → 404; unexpected failure → 500.

#### Search messages — GET /api/messages/search
- Invocation: authenticated client request.
- Inputs: query `q` (content substring), `conversationId`, `messageType`, `senderType`, `dateFrom`, `dateTo`, `isRecalled` ("true"/"false"), `limit` (default 50), `offset` (default 0).
- Behavior: build filters from supplied parameters and return matching messages ordered newest-first with their sender names and attachments resolved. Content matching is a safe substring/contains match.
- Success Output: 200 with `success: true`, a `data` object containing `messages` (detailed view with sender name/avatar, attachments, reactions derived from metadata, read receipts derived from stored read markers), a `total` count, and a `pagination` object (limit, offset, hasMore), plus an echo of the effective query and a `timestamp`.
- Error Conditions: unexpected failure → 500 with `success: false` and error details.

#### Message statistics — GET /api/messages/stats
- Invocation: authenticated client request.
- Behavior: returns a global statistics payload (total message count; several breakdown fields are reported as zero within the current behavioral boundary; and an averaged-per-day figure derived from the total).
- Success Output: 200 with `success: true` and a `data` object containing an `overview` (totalMessages, todayMessages, activeConversations, averagePerDay, recalledMessages), a `breakdown` object, a scope label, a note, and generation timestamp.
- Error Conditions: unexpected failure → 500.

#### List available message tags — GET /api/messages/tags
- Invocation: authenticated client request.
- Behavior: aggregates tag labels found in non-recalled messages' metadata and returns each label with an occurrence count, ordered by count descending.
- Success Output: 200 with `success: true`, `data.tags` (array of {name, count}) and `data.total`, plus `timestamp`.
- Error Conditions: unexpected failure → 500.

#### Export filter option — customers — GET /api/messages/export/customers
- Invocation: authenticated client request.
- Behavior: returns up to a capped number of non-deleted customers (identifier, display name, platform, platform user identifier) ordered by display name, for use as export filter choices.
- Success Output: 200 with `success: true`, `data` array, `timestamp`.

#### Export filter option — agents — GET /api/messages/export/agents
- Invocation: authenticated client request.
- Behavior: returns up to a capped number of active agents (identifier, display name, role) ordered by display name.
- Success Output: 200 with `success: true`, `data` array, `timestamp`.

#### Export pre-count — GET /api/messages/export/count
- Invocation: authenticated client request.
- Inputs: query `conversationId`, `dateFrom`, `dateTo`, `customerId`, `agentId` (all optional). Recalled messages are always excluded from the count.
- Behavior: counts messages matching filters and reports whether the result would be truncated by the configured export ceiling.
- Success Output: 200 with `success: true`, `data` (count, limit ceiling, willBeTruncated flag), `timestamp`.

#### Export messages — GET /api/messages/export
- Invocation: authenticated client request.
- Inputs: query `format` ("json", "csv", or "txt"; default "json"), `conversationId`, `dateFrom`, `dateTo`, `customerId`, `agentId`, `limit` (clamped to a configured maximum, default 100). Recalled messages are always excluded.
- Behavior: select matching messages newest-first up to the limit, resolving sender names (persisted snapshot preferred). For JSON: structured envelope. For CSV: a downloadable file with a fixed column set. For TXT: a downloadable human-readable transcript grouped by conversation, each group sorted oldest-first, lines formatted as localized-time, sender name, content.
- Success Output: JSON → 200 envelope with `data.messages` and `data.exportInfo` (format, totalRecords, exportedAt, exportedBy, echoed filters), plus `timestamp`. CSV/TXT → 200 with a downloadable attachment payload and the appropriate content type and filename.
- Error Conditions: invalid `format` → 400; unexpected failure → 500.

#### Bulk create messages — POST /api/messages/bulk-create
- Invocation: authenticated client request.
- Inputs (JSON body): `messages` (array, required, non-empty, at most 100 entries). Each entry: `conversationId` (required), `content` (required, non-empty after trimming), `messageType` (optional, defaults to text), `metadata` (optional).
- Behavior: validate each entry's required fields; verify referenced conversations exist; insert all valid messages (authored by the caller as agent, marked sent with delivery status "sent" and caller display name captured); bump affected conversations' timestamps. Invalid entries are collected as per-item errors and do not abort the batch.
- Success Output: 201 with totalRequested, successCount, failureCount, a `results` list (per success: index, new identifier, conversation identifier, status), and an `errors` list when any item failed.
- Error Conditions: malformed JSON or empty/invalid `messages` → 400; batch over 100 → 400; per-item validation/missing-conversation captured as item errors; unexpected failure → 500.

#### Bulk recall messages — POST /api/messages/bulk-delete
- Invocation: authenticated client request.
- Inputs (JSON body): `messageIds` (array, required, non-empty, at most 100).
- Behavior: fetch all referenced messages; for each, validate it exists, that the caller is the agent author or an administrator (for agent-origin messages), that it is not already recalled, and that any recall deadline has not passed; recall all eligible messages (set recalled flag, recall timestamp, overwrite content with the placeholder marker) in one batch. Ineligible items are returned as per-item errors.
- Success Output: 200 with totalRequested, successCount, failureCount, a `results` list (per success: identifier, conversation identifier, recalled timestamp, status), and an `errors` list when present.
- Error Conditions: malformed JSON or empty/invalid `messageIds` → 400; over 100 → 400; per-item failures (not found / permission denied / already recalled / deadline passed) captured as item errors; unexpected failure → 500.

#### List message attachments — GET /api/messages/:id/attachments
- Invocation: authenticated client request.
- Inputs: path message identifier.
- Behavior: verify message exists; return its attachment records.
- Success Output: 200 with message identifier, conversation identifier, an `attachments` array (each: identifier, message identifier, filename, MIME type, file size, file URL, storage key, created timestamp), and a `count`.
- Error Conditions: missing identifier → 400; message not found → 404; unexpected failure → 500.

#### Upload message attachment — POST /api/messages/:id/attachments
- Invocation: authenticated multipart form request (field name `file`).
- Inputs: path message identifier; multipart file part.
- Preconditions & Authorization: message must exist. For agent-origin messages, only the author or an administrator may add attachments. File size must be at most 10 MB. The file MIME type must be in the allowed set (common image, video, audio, PDF, plain text, and Word document types).
- Behavior: validate ownership, size, and type; make the binary retrievable through a public or signed URL; persist an attachment record linked to the message.
- Success Output: 201 with new attachment identifier, message identifier, filename, MIME type, file size, URL, and created timestamp.
- Error Conditions: missing identifier → 400; message not found → 404; not author/admin → 403; missing file → 400; oversize → 400; disallowed type → 400; storage failure → 500; unexpected failure → 500.

#### Forward message — POST /api/messages/:id/forward
- Invocation: authenticated client request.
- Inputs (JSON body): `targetConversationIds` (array, required, non-empty, at most 20), `comment` (string, optional).
- Behavior: fetch the original message; verify which target conversations exist; for each valid target, create a new agent-authored message whose content is the original content prefixed with a forwarded marker (and appended with the optional comment), preserving the original message type and recording forwarding provenance in metadata (source message/conversation, original sender role, forwarder identity, forward time); messages are marked sent with delivery status "sent"; affected conversations' timestamps are bumped in one batch; an audit entry is recorded. Missing targets are reported as per-item errors.
- Success Output: 201 with originalMessageId, totalTargets, successCount, failureCount, a `results` list (per success: target conversation identifier, new message identifier, status) and an `errors` list when present.
- Error Conditions: missing identifier or malformed JSON → 400; empty/missing target list → 400; more than 20 targets → 400; original message not found → 404; per-target missing-conversation captured as item errors; unexpected failure → 500.

#### Set/replace message tags — PUT /api/messages/:id/tags
- Invocation: authenticated client request.
- Inputs (JSON body): `tags` (array of strings, required, at most 10 entries; every entry must be a non-empty string).
- Behavior: verify message exists; merge a trimmed tag list plus a tags-updated timestamp and updater identity into the message's metadata, replacing any prior tags.
- Success Output: 200 with message identifier, conversation identifier, the new tags, the previous tags, updated timestamp, and updater identifier.
- Error Conditions: missing identifier or malformed JSON → 400; tags not an array → 400; over 10 tags → 400; any blank/non-string entry → 400; message not found → 404; unexpected failure → 500.

#### Remove all message tags — DELETE /api/messages/:id/tags
- Invocation: authenticated client request.
- Inputs: path message identifier.
- Behavior: verify message exists; remove the tag collection from metadata and record a removal timestamp and remover identity.
- Success Output: 200 with message identifier, conversation identifier, the removed tags, and removal timestamp.
- Error Conditions: missing identifier → 400; message not found → 404; unexpected failure → 500.

#### Schedule delayed message
- Invocation: delayed-messaging behavior requested on behalf of an authenticated agent.
- Inputs: target conversation identifier, content, `delaySeconds` (integer, 1–120 inclusive), optional message type (defaults to text), optional recipient platform identifier, optional platform ("line"/"facebook"/"webchat", default webchat), optional media URL, optional metadata.
- Preconditions: delay within 1–120 seconds; target conversation must exist.
- Behavior: persist a pending delayed-message record carrying a scheduled send time computed from now plus the delay; mark it recallable in a fast lookup store with an expiry slightly beyond the delay; the scheduled time also serves as the recall deadline for the pending item.
- Success Output: a result object with success flag, the delayed-message identifier, the scheduled send time, and the recall deadline.
- Error Conditions: delay out of range → validation error; conversation missing → validation error; other failures → success: false with an error message.

#### Process / dispatch a scheduled delayed message
- Invocation: triggered when a scheduled send becomes due.
- Behavior: load the pending delayed item; if its status is not pending, or if it is too early relative to the scheduled time, do not send (re-schedule signal returned for too-early). For external platforms (LINE/Facebook), dispatch via the platform-specific processing path. For the in-app webchat platform, create the actual persisted message (agent-authored, marked sent, delivery status "sent", with a recall window set to 30 minutes after send) and mark the delayed item as sent. Unsupported platforms cause the item to be marked failed. The recallable lookup marker is cleared on successful send. On unexpected error the item is marked failed with a reason.
- Side Effects: a real persisted message may be created; delayed item status transitions; fast-lookup recall marker removed.

#### Cancel / recall a pending delayed message
- Invocation: delayed-message cancel/recall capability.
- Preconditions: the delayed item must exist and still be pending; the current time must be before the scheduled send time.
- Behavior: transition the item to a cancelled state, store the cancellation/recall reason in its metadata, clear the recallable lookup marker, and record a recall-log entry on the recall path.
- Error Conditions: item missing → failure ("not found"); item not pending → failure with status reason; past scheduled time → failure ("cannot recall after scheduled send time").

#### Recall an already-sent message (service capability)
- Invocation: delayed-message recall capability.
- Preconditions: message must exist, not already recalled, and within any recall deadline.
- Behavior (observable order): mark the message recalled with a recall timestamp; write a recall-log entry indicating success; then best-effort notify the originating external platform — for LINE, send a customer-facing "this message has been recalled" notice (LINE has no native unsend via API); for Facebook, attempt a platform-side delete of the message using the stored platform message identifier; other platforms are skipped. Platform notification failure does NOT revert the database recall. On a recall failure a failure recall-log entry is written.
- Success Output: success flag, message identifier, recall timestamp, and a canRecall indicator. Failure returns success: false with an error and canRecall: false; "already recalled" and "deadline exceeded" are distinct error reasons.

### Data Concepts (neutral)

- Message record: a unique message identifier; the owning conversation identifier; a sender role (one of customer / agent / system); a customer-author reference and an agent-author reference (one populated per message); textual content; a message type (text / image / video / audio / file / sticker / location); an optional external-platform message identifier; a recall flag, an optional recall deadline, and an optional recalled timestamp; a sent flag and a sent timestamp; a delivery status (pending / sent / delivered / failed / recalled); an optional reply-to reference to another message; optional thread, session, and per-session sequence markers; a persisted sender-name snapshot (so historical messages display correctly even if the author record changes); an arbitrary metadata object (which also carries per-message tags, tag audit stamps, reactions, forwarding provenance, and read markers); created and updated timestamps; and a soft-delete marker. Messages are never hard-deleted by this area; recall overwrites content with a placeholder and flags the record, while soft-deleted messages are hidden from reads.

- Attachment record: a unique attachment identifier; the linked message identifier (may initially be unlinked, then associated at message creation time); filename; MIME type; file size in bytes; a retrieval URL; an underlying storage key; and a created timestamp. Allowed types and a 10 MB ceiling apply on upload.

- Delayed-message record: a unique identifier; the owning conversation identifier; the scheduling agent identifier; content and message type; a scheduled send time; a lifecycle status (pending / sent / failed / cancelled, plus an archived terminal state used to retire stale failed items); and a metadata object carrying recipient platform identifier, platform, configured delay seconds, optional media URL, and any failure/cancellation reason. A parallel fast-lookup recallability marker exists while an item is cancellable, expiring shortly after the scheduled time.

- Recall-log entry: an auto-assigned identifier; the message identifier the action targeted; the requesting user identifier; an action outcome ("successful" or "failed"); and a created timestamp. Used for recall history and statistics.

- Offline-buffer entry (persistence capability): per-recipient buffered copies of real-time messages for users who are offline, each carrying a message identifier, recipient identifier, conversation identifier, the message payload, a buffered timestamp, a delivered flag, a retry counter, and an expiry. Buffered entries default to a roughly seven-day retention; once marked delivered they are retained only briefly; entries past expiry are purged; retries beyond a maximum cause the entry to be dropped.

- Message-validation rule set (neutral defaults): content length between 1 and 5000 characters; delayed-send delay between 1 and 120 seconds; attachment ceiling 10 MB with a per-message attachment count limit; batch operations capped at 100 items with a concurrency ceiling.

### State & Lifecycle

- Persisted message: created in a "sent" delivery state. Permitted observable transitions of delivery status: pending → sent → delivered, with a failed outcome possible, and a recalled outcome when recalled. Recall is a one-way terminal transition (a recalled message cannot be edited or recalled again; content becomes the placeholder marker). Edits are permitted only while not recalled and only by the author or an administrator.

- Recall eligibility: a message is recallable only if it exists, is not already recalled, and (if a recall deadline is set) the deadline has not passed. Eligibility for delayed items is governed instead by their pending status and not-yet-reached scheduled time.

- Delayed message: pending → sent (on successful dispatch), pending → cancelled (on cancel/recall before the scheduled time), pending → failed (on dispatch error or unsupported platform), and failed → archived (when retiring stale failed items older than a day). Once not pending, it can neither be cancelled nor reprocessed.

### Real-time / Event Behavior

- Mention notifications: when a created message's content includes @-mentions, a notification is dispatched to each mentioned user other than the author, carrying the mentioner's name and identifier, the conversation identifier, and a truncated content preview. Dispatch is best-effort and does not block or fail the create operation.

- Audit activity events: message send, message recall, and message forward operations each emit an audit activity record (best-effort) capturing actor identity and role, the target message identifier, and contextual details (conversation identifier, message type, or forward target count). These do not affect the operation's success.

- Platform recall propagation: a successful recall of an already-sent message triggers a best-effort downstream notification to the customer's originating channel (LINE: a customer-facing recall notice; Facebook: an attempted platform-side message delete). Failure of this downstream step is swallowed and never reverts the recall.

- Offline message buffering and replay: the persistence capability buffers real-time messages for offline recipients and can later replay undelivered (and optionally already-delivered) buffered messages on reconnect, mark individual or batched messages as delivered, retry undelivered messages up to a maximum, and report per-recipient buffer statistics (total, delivered, pending, expired). This guarantees at-least-once redelivery intent for offline recipients within the retention window, with idempotent delivery marking.

- Batching/optimization (real-time fan-out): outbound real-time events may be grouped per target into batches that flush when a size threshold is reached, after a short delay window elapses, or immediately when an urgent-priority event is present; duplicate transient events (such as repeated typing or join/leave signals for the same user in the same conversation) are collapsed before fan-out. This affects only delivery efficiency, not message persistence or ordering of stored messages.

## Customer-Facing Conversations

### Purpose
This area provides the per-conversation interface used to view a single conversation's message history, send a reply into that conversation, upload a file attachment, and subscribe to a live real-time channel for that conversation. Access to every operation is gated by a single shared access-control rule that admits administrators, the conversation's own owner, members of the team the conversation is assigned to, and (when the conversation has no assigned team) anyone with a valid session. When a reply is sent, the system persists it, advances the conversation's recency markers, relays it to the originating external messaging platform when applicable, and pushes it in real time to all connected subscribers of that conversation.

### Operations

#### Retrieve conversation message history — GET /api/customer-conversations/{conversationId}/messages
- Invocation: authenticated client request.
- Inputs:
  - `conversationId` (path segment, required): the unique conversation identifier.
  - Session credential (required): supplied in any one of three ways — an `X-Session-Id` request header (case-insensitive variants accepted), or an `Authorization` header of the form `Bearer <token>`. The credential is a signed session token.
  - `limit` (query, optional, integer, default 50): maximum number of history entries to return in this page.
  - `before` (query, optional, string): a message identifier used as a pagination cursor; results are restricted to entries that precede that referenced entry in time (older entries). If the referenced entry cannot be located, the most recent page is returned instead.
- Preconditions & Authorization: a valid, non-expired session is required, and the caller must satisfy the shared conversation access rule (see Preconditions in the WebSocket operation and the State & Lifecycle section). The conversation must exist.
- Behavior: returns a page of message-history entries for the conversation, ordered most-recent-first. Each returned entry is enriched with a unified sender identifier (resolved from whichever of the agent-side or customer-side sender reference is present) and with its associated file attachments. For each attachment that has stored binary content, the system additionally mints a time-limited signed download link that forces the browser to save the file rather than render it inline; if minting that link fails, the attachment is still returned but without the extra download link, and clients fall back to the inline file link.
- Success Output: HTTP 200 with a JSON body containing `success: true`, a `messages` array (each item: the message fields plus a unified sender identifier field and a list of file attachments, where each attachment carries its inline file link and, when available, a separate force-download link), and `hasMore` (boolean) which is true when the returned page is exactly the requested size (indicating more entries may exist).
- Side Effects: none (read-only).
- Error Conditions:
  - Missing `conversationId` -> HTTP 400, `success: false` with a missing-identifier message.
  - Missing session credential -> HTTP 401, `success: false`, authentication-required message.
  - Invalid or expired session -> HTTP 401, `success: false`, invalid-or-expired-session message.
  - Caller not permitted for this conversation -> HTTP 403, `success: false`, access-denied message.
  - Conversation does not exist -> HTTP 404, `success: false`, not-found message.
  - Server credential/secret not configured -> HTTP 500, `success: false`, server-configuration message.
  - Server retrieval failure -> HTTP 500, `success: false`, failure-to-fetch message.
- Invariants & Guarantees: read-only; ordering is strictly most-recent-first; pagination is timestamp-anchored relative to the cursor entry.

#### Send a reply into a conversation — POST /api/customer-conversations/{conversationId}/messages
- Invocation: authenticated client request.
- Inputs:
  - `conversationId` (path segment, required): the unique conversation identifier.
  - Session credential (required): same three supply methods as the history retrieval operation.
  - JSON body fields:
    - `content` (optional string): message text. Must be non-empty (after trimming) unless attachments are provided.
    - `attachmentIds` (optional array): identifiers of previously uploaded files to attach to this message.
    - `assets` (optional array): supplementary references associated with the message (retained alongside the message metadata).
    - `messageType` (optional string): the message kind; defaults to a text kind. If attachments are present, the effective kind is forced to a file kind regardless of this value.
    - `platform` (optional string): originating platform label retained in metadata; defaults to a system label.
    - `correlationId` (optional string): a client-supplied correlation token retained and echoed back to support real-time de-duplication on the client.
- Preconditions & Authorization: valid non-expired session; caller must satisfy the shared conversation access rule; conversation must exist. At least one of non-empty `content` or a non-empty `attachmentIds` list must be present.
- Behavior (observable order):
  1. The reply is recorded as an agent-originated message in the conversation, with a newly generated unique identifier and a creation timestamp. The persisted sender is derived from the session credential's embedded user identity (and a display-name snapshot is stored). The reply is recorded as already sent and in a delivered state.
  2. Any referenced uploaded attachments are linked to the new message.
  3. The conversation's recency markers (last-activity and last-message timestamps) are advanced to the new message's creation time, so the conversation re-sorts to the top of any recency-ordered listings.
  4. If the conversation's customer belongs to an external LINE-type channel, the message text and any attachments are relayed outbound to that external user. Images are relayed as image content; other files as file content. Outbound relay is chunked when the number of outbound items exceeds the platform's per-call limit. Failure of the external relay does NOT fail the request — the message remains stored and is still delivered in real time.
  5. The new message is pushed in real time to all live subscribers of this conversation (see Real-time section).
  6. A global notification is emitted so that conversation-list views elsewhere update their latest-message preview and ordering in real time. Failure of this global notification is non-fatal.
- Success Output: HTTP 200 with `success: true` and a `message` object representing the created message, including its identifier, content, kind, sender type, unified sender identifier, the linked file attachments, and the echoed `correlationId`.
- Side Effects: a new conversation message is persisted; referenced attachments are associated with it; the conversation's recency markers are updated; an outbound external-platform message may be sent; a real-time per-conversation event and a global conversation-list event are broadcast.
- Error Conditions:
  - Missing `conversationId` -> HTTP 400, missing-identifier message.
  - Missing session credential -> HTTP 401, authentication-required message.
  - Invalid/expired session -> HTTP 401.
  - Not permitted for this conversation -> HTTP 403.
  - Conversation not found -> HTTP 404.
  - Both `content` empty and no attachments -> HTTP 400, content-or-attachments-required message.
  - Server secret unconfigured -> HTTP 500.
  - Server creation failure -> HTTP 500, failure-to-create message.
- Invariants & Guarantees: each accepted reply produces a distinct, uniquely identified message. The optional client `correlationId` is preserved end-to-end (in the response and the real-time broadcast) to let clients suppress duplicate rendering of their own optimistic message. External-platform relay and global broadcasting are best-effort and never block successful persistence. The request body is consumed exactly once.

#### Upload a file attachment for a conversation — POST /api/customer-conversations/{conversationId}/upload
- Invocation: authenticated client request with multipart form data.
- Inputs:
  - `conversationId` (path segment, required).
  - Session credential (required): same three supply methods as above.
  - Multipart form field `file` (required): the binary file to store.
- Preconditions & Authorization: valid non-expired session; caller must satisfy the shared conversation access rule; conversation must exist. Additionally, at the storage layer the session is re-validated against the live session store (must exist and be unexpired).
- Behavior: makes the uploaded binary retrievable through a generated, conversation-scoped, unique public link that preserves the original file extension and remains cacheable for a long period. This operation only uploads the file; it does not by itself create a message — the returned reference is meant to be passed later to a reply via `attachmentIds`/`assets`.
- Success Output: HTTP 200 with `success: true`, `url` (public link to the stored file), `filename` (original file name), `size` (byte size), and `contentType` (MIME type).
- Side Effects: a new stored object is created in file storage. No message or conversation change occurs.
- Error Conditions:
  - Missing `conversationId` -> HTTP 400.
  - Missing session credential at the entry layer -> HTTP 401, authentication-required.
  - Invalid/expired session (entry-layer access check) -> HTTP 401.
  - Not permitted for this conversation -> HTTP 403.
  - Conversation not found -> HTTP 404.
  - Session invalid/expired/not-found at the storage layer re-check -> HTTP 401 with the specific reason.
  - No `file` field present -> HTTP 400, no-file-provided message.
  - Server storage failure -> HTTP 500, failure-to-upload message.
- Invariants & Guarantees: each upload yields a unique storage key, so uploads never overwrite one another; uploads are namespaced per conversation.

#### Subscribe to a conversation's real-time channel — GET /api/customer-ws (WebSocket upgrade)
- Invocation: authenticated client WebSocket upgrade request.
- Inputs (query parameters):
  - `conversationId` (required): the conversation to subscribe to.
  - `sessionId` (required): the signed session credential.
  - The request must carry a WebSocket upgrade header.
- Preconditions & Authorization: the session credential must be valid and non-expired and the caller must satisfy the shared four-way access rule for the target conversation:
  1. An administrator role is always admitted.
  2. The conversation's owner (the customer the conversation belongs to) is admitted.
  3. If the conversation has no assigned team, any valid session is admitted (open pool).
  4. Otherwise, the caller is admitted only if they are a member of the conversation's assigned team.
  A caller satisfying none of these is rejected.
- Behavior: on success the connection is upgraded and the subscriber is registered against the conversation's real-time channel using the credential's validated identity (user identifier, role, display name). Multiple simultaneous connections by the same user (e.g., several tabs) are all tracked independently. Upon joining, a presence event is broadcast to the other existing subscribers. The server does not act on inbound WebSocket messages from clients (reserved for future use); all message delivery is server-to-client.
- Success Output: a successful WebSocket upgrade (HTTP 101). The live channel then delivers server-pushed events (see Real-time section).
- Side Effects: registers a live subscriber; broadcasts a presence "connected" event to other subscribers; on disconnect, deregisters and (only when the user has no remaining connections) broadcasts a presence "disconnected" event.
- Error Conditions:
  - Missing `conversationId` or `sessionId` -> HTTP 400, missing-required-parameters message.
  - Invalid/expired session -> HTTP 401.
  - Not permitted for this conversation -> HTTP 403.
  - Conversation not found -> HTTP 404.
  - Server secret unconfigured -> HTTP 500.
  - Non-WebSocket request to the upgrade target -> HTTP 400, expected-WebSocket message.
- Invariants & Guarantees: connections are tracked per individual connection, not per user, so one user may hold many concurrent subscriptions; a "disconnected" presence event fires only when a user's last connection closes. Dead or unwritable connections are pruned during broadcast. Each conversation has an isolated channel keyed by its identifier.

### Data Concepts (neutral)
- **Conversation**: a thread between a customer and the support side. Carries a unique identifier, a reference to its owning customer, and an optional reference to the team it is assigned to (absent = open/unassigned pool). Also carries recency markers (last-activity time and last-message time) used for ordering. Conversations follow system-wide soft-delete semantics (a deleted conversation is excluded from valid references elsewhere).
- **Message**: an entry in a conversation. Carries a unique identifier, the conversation it belongs to, a sender type (customer-side vs. agent-side), separate customer-side and agent-side sender references (only one populated), a unified sender identifier exposed on the wire, text content, a kind (text vs. file), a sent flag, a delivery state, a stored snapshot of the sender's display name, free-form metadata (which may retain supplementary asset references, an originating-platform label, and a client correlation token), associated file attachments, and a creation timestamp.
- **File attachment**: a stored file linked to a message. Carries an identifier, the message it belongs to (assigned when the file is attached to a reply), a storage key for its binary content, an inline file link, a file name, a byte size, and a MIME type. On read it may also be presented with a separate time-limited force-download link.
- **Session credential**: a signed token identifying the caller, embedding a user identity, a role (administrator / agent / customer), and a display name, plus an expiry. A live session record is also kept in the session store with an explicit expiry used for re-validation at the storage layer.
- **Team membership**: an association between a user and a team, consulted to decide team-scoped access to assigned conversations.
- **Customer / external channel**: a customer record carries the external messaging platform type (e.g., a LINE-type channel) and the external platform user identifier, used to relay outbound replies to that external service.

### State & Lifecycle
- **Conversation assignment state** governs access: `unassigned` (no team) -> open to any valid session; `assigned to a team` -> open only to administrators, the owner, and members of that team. The owner and administrators always have access regardless of assignment.
- **Message delivery state**: agent replies created here are recorded as already sent and in a delivered state at creation; this area does not transition messages through pending/failed states.
- **Subscriber lifecycle**: a subscriber is `connected` on successful upgrade and `disconnected` on socket close or error; the user is considered offline (and a disconnected presence event emitted) only when their final connection ends.
- **Soft delete**: conversations and messages participate in the system soft-delete model; soft-deleted conversations are not treated as valid targets.

### Real-time / Event Behavior
The per-conversation channel emits the following server-to-client events to subscribers of that conversation:
- **New message event** (`new_message`): emitted whenever a reply is created in the conversation. Payload includes the conversation identifier, a nested data object (conversation identifier, content, message kind, sender type, unified sender identifier, originating-platform label defaulting to the LINE-type label, and a timestamp), the full message object (carrying its identifier, attachments, and any client correlation token for de-duplication), and a timestamp. Delivered to every live connection of the conversation, including multiple tabs of the same user.
- **Message updated event** (`message_updated`): emitted when a previously created message's media/attachment data becomes available after deferred processing. Payload includes the conversation identifier and a data object with the conversation identifier, the affected message identifier, and the updated attachment list, plus a timestamp. Triggered by an internal notification rather than a client request.
- **Presence connected event** (`USER_CONNECTED`): emitted to the conversation's other subscribers when a user joins. Payload carries the user identifier and a timestamp.
- **Presence disconnected event** (`USER_DISCONNECTED`): emitted to the conversation's remaining subscribers when a user's last connection ends. Payload carries the user identifier and a timestamp.

Separately, a **global conversation-list event** (a new-message notification) is emitted on the system-wide broadcast channel whenever a reply is created, so conversation-list views update their latest-message preview and recency ordering. Its payload carries an event identifier, a new-message type, a source label, a timestamp, the conversation identifier, and a data object (conversation identifier, message identifier, content, message kind, sender type, unified sender identifier, a platform label, and a timestamp). This global emission is best-effort and non-blocking.

## Delayed / Scheduled Messages

### Purpose
This area lets an authenticated support agent compose an outbound message to an end customer (over LINE or Facebook Messenger) that is held back for a short configurable delay (1 to 120 seconds) before it is actually delivered to the messaging platform. During the delay window the agent can cancel ("recall") the message so it is never sent, query its remaining countdown, list pending scheduled messages, or reschedule it. Two parallel implementations exist: a legacy flow and a newer real-time path that offers near-instant cancellation and precise firing. Both surface the same business capability and emit real-time countdown / sent / recalled / failed events so connected clients can show a live countdown UI.

### Operations

There are two route families. The legacy family is mounted under a base path of `/api/delayed-messages`. The newer real-time family is mounted under `/api/delayed-messages-v2`. The newer family is the primary path used by clients.

---

#### Schedule a delayed message (real-time buffer) — POST /api/delayed-messages-v2/send
- Invocation: authenticated client request (bearer auth required).
- Inputs (JSON body):
  - conversation identifier — string, required.
  - message text content — string, required.
  - target platform — string, required; only "line" or "facebook" are meaningful.
  - recipient's platform-specific user identifier — string, required.
  - delay in seconds — integer, optional, default 5; must be between 1 and 120 inclusive.
  - message type — string, optional, default "text" (other recognized values: image, video, audio, file).
- Preconditions & Authorization: valid session token; the caller must pass a permission check for sending a message scoped to the target conversation (role-based; failure yields a forbidden result). The scheduling capability must be available.
- Behavior: validates required fields and the delay range, checks send permission, then records the message as pending for that conversation. A new unique message identifier is generated server-side. An absolute fire time of now plus the delay is computed, and the message is queued for delivery at that time. The call returns immediately without waiting for delivery. An audit/activity log entry for "delayed message scheduled" is written best-effort (failure is ignored).
- Success Output: success envelope containing the generated message identifier, the absolute scheduled fire time (epoch milliseconds), a "can cancel until" time equal to the fire time, the effective delay in seconds, and the conversation identifier. HTTP 200.
- Side Effects: the pending scheduled message is recorded so that it survives restarts and will be delivered at its fire time; an activity log record is attempted. A real-time "countdown" event may be emitted (see Real-time section); sent/failed events are emitted at fire time.
- Error Conditions:
  - missing any required field -> 400 with a message naming the required fields.
  - delay outside 1-120 -> 400.
  - permission check fails -> 403 forbidden.
  - scheduling capability unavailable -> 503 service unavailable.
  - scheduler reports failure -> 500 server error with the underlying reason.
  - unexpected exception -> standard API error response.
- Invariants & Guarantees: each scheduled message gets a unique identifier; scheduling for one conversation is isolated from other conversations; the next delivery always reflects the earliest pending fire time. Idempotency is enforced at fire time (a message already recorded as delivered will not be re-sent).

#### Cancel (recall) a delayed message (real-time buffer) — DELETE /api/delayed-messages-v2/cancel/:messageId
- Invocation: authenticated client request.
- Inputs: message identifier in the path (required); JSON body containing the conversation identifier (required) and an optional free-text cancellation reason (defaults to "User cancelled").
- Preconditions & Authorization: valid session token. The scheduling capability must be available. Cancellation only succeeds while the message is still pending and its fire time has not yet passed.
- Behavior: requests cancellation of the identified message. If the message exists, is still pending, and its fire time is in the future, it is removed from the pending set, the next scheduled delivery for that conversation is re-evaluated, and the message will never be delivered. Best-effort audit log for "delayed message cancelled".
- Success Output: success envelope with the message identifier, the cancellation timestamp (epoch ms), and the display name of the cancelling user. HTTP 200.
- Side Effects: the pending message is removed so it will not be delivered; the next scheduled delivery is re-evaluated (or cleared if none remain); activity log attempted.
- Error Conditions:
  - missing message identifier -> 400.
  - missing conversation identifier -> 400.
  - message not found / already processed -> 400 with reason "Message not found or already processed".
  - message no longer pending (already sent/cancelled/failed) -> 400 with reason "Message already <state>".
  - fire time already passed -> 400 with reason "Message send time has passed".
  - scheduler not configured -> 503.
- Invariants & Guarantees: cancellation takes effect immediately; it is only valid strictly before the fire time; once fired the message can no longer be cancelled.

#### Query countdown / status of a delayed message — GET /api/delayed-messages-v2/status/:messageId
- Invocation: authenticated client request (used by the countdown UI).
- Inputs: message identifier in path (required); conversation identifier as a query parameter (required).
- Preconditions & Authorization: valid session token; the scheduling capability available.
- Behavior: looks up the identified message for that conversation and reports its current state.
- Success Output: success envelope with: existence flag; current state ("pending" / "sent" / "cancelled" / "failed", or "not_found" when absent); remaining time in whole seconds (rounded up, floored at 0); a boolean indicating whether it can still be cancelled (true only when pending and remaining time is positive); and the absolute scheduled fire time. HTTP 200. When the message is unknown, returns existence=false and state "not_found".
- Error Conditions: missing message or conversation identifier -> 400; scheduler not configured -> 503.

#### List pending delayed messages for a conversation — GET /api/delayed-messages-v2/pending
- Invocation: authenticated client request.
- Inputs: conversation identifier as a query parameter (required).
- Preconditions & Authorization: valid session token; the scheduling capability available.
- Behavior: returns all currently pending (not yet fired, not cancelled) messages for that conversation.
- Success Output: success envelope with the conversation identifier, a count, and an array where each entry carries the message identifier, a content preview (truncated to the first 100 characters), the absolute fire time, and remaining time in milliseconds (floored at 0). HTTP 200.
- Error Conditions: missing conversation identifier -> 400; scheduler not configured -> 503.

#### Service health (real-time buffer) — GET /api/delayed-messages-v2/health
- Invocation: public, no authentication.
- Behavior: returns a static health descriptor.
- Success Output: success envelope reporting service name, healthy status, and feature flags (instant cancel, precise scheduling, durable persistence of pending messages) all true. HTTP 200. (An equivalent public health endpoint is also exposed directly at the top-level routing layer.)

---

The following operations belong to the legacy family mounted under `/api/delayed-messages`. Observable behavior differs from the real-time scheduler family by using short-lived recall and cancellation markers that can affect later delivery attempts.

#### Schedule a delayed message (legacy) — POST /api/delayed-messages/send
- Invocation: authenticated client request.
- Inputs (JSON body): conversation identifier (required); content (required, non-empty, max 5000 characters); delay in seconds (required, integer, 1-120); sender identifier (required); recipient platform identifier (required); platform (required, "line" or "facebook"); optional message type (text/image/video/audio/file); optional media URL (must be a valid HTTPS URL when present).
- Preconditions & Authorization: valid session; the caller must pass a send-message permission check scoped to the conversation.
- Behavior: validates the request against all field rules, checks permission, then creates a pending scheduled record with a generated identifier, computes the absolute fire time (now + delay) and a recall deadline equal to the fire time, persists the record, and writes a short-lived "recallable" marker that expires shortly after the fire time. Then broadcasts a real-time countdown event (best-effort; broadcast failure does not fail the request).
- Success Output: success envelope with the message identifier, the scheduled fire time (ISO timestamp), and the recall deadline (ISO timestamp).
- Error Conditions: validation failure -> failure result describing the invalid fields; permission denied -> failure result; persistence/scheduling failure -> failure result; any failure surfaces to the caller as a 400-style bad-request response with the error text.
- Invariants: recall deadline equals the fire time, so a message can only be recalled before it is due to send.

#### Recall (cancel) a delayed message (legacy) — POST /api/delayed-messages/recall/:messageId
- Invocation: authenticated client request.
- Inputs: message identifier in path (required, non-empty).
- Preconditions & Authorization: valid session. A short-lived "recallable" marker must still exist for the message; the requesting user must be the original sender; the recall deadline must not have passed.
- Behavior: validates recall eligibility (existence of marker, sender ownership, deadline not passed). On success, writes an immediate "cancelled" marker (short-lived) so the firing path will skip delivery, and asynchronously updates the stored record's state to cancelled and logs the operation. Then broadcasts a recall-success event (best-effort). On ineligibility it broadcasts a recall-failed event.
- Success Output: success envelope with the message identifier.
- Error Conditions: missing message identifier -> failure "Message ID is required"; marker missing -> "Message not found or already processed"; requester is not the sender -> "Permission denied: only the sender can recall the message"; deadline passed -> "Recall deadline has passed". All surfaced as a bad-request response.
- Invariants: only the original sender may recall; only valid before the deadline; cancellation marker makes the later firing step a no-op.

#### List pending delayed messages for the caller (legacy) — GET /api/delayed-messages/pending
- Invocation: authenticated client request.
- Inputs: page number (query, default 1) and page size (query, default 20); page must be >= 1, page size between 1 and 100.
- Preconditions & Authorization: valid session; results are scoped to the authenticated user as the sender.
- Behavior: returns the caller's own pending scheduled messages ordered by fire time, paginated.
- Success Output: success envelope with an array of pending message records (identifier, conversation identifier, sender identifier, content, message type, fire time, state, metadata, timestamps, plus a customer display label and a per-row "can still recall" indicator) and pagination totals (total count, page, page size).
- Error Conditions: invalid pagination parameters -> failure with code indicating invalid parameters; surfaced as bad-request.

#### Reschedule a delayed message (legacy) — POST /api/delayed-messages/reschedule/:messageId
- Invocation: authenticated client request.
- Inputs: message identifier in path (required); JSON body with a new delay in seconds (required, integer, 1-120).
- Preconditions & Authorization: valid session; the message must exist, must still be pending, and the requesting user must be its sender.
- Behavior: validates the new delay, verifies the message is still pending and owned by the requester, recomputes the fire time and recall deadline from now plus the new delay, updates the stored record (state stays pending) and refreshes the recall marker. Requires a recognized platform on the record (line/facebook) or it fails. Broadcasts a fresh countdown event (best-effort).
- Success Output: success envelope with the message identifier and the new fire time.
- Error Conditions: invalid new delay -> failure with the validation message; message not found -> "Message not found"; not pending -> "Message cannot be rescheduled"; not the sender -> "Permission denied"; unsupported platform on record -> scheduling failure. Surfaced as bad-request.

---

#### Due-message firing behavior
- Invocation: triggered when a message reaches its scheduled fire time.
- Behavior (observable outcome): when due, the system first checks whether the message was already cancelled / already delivered. If a cancellation marker exists or the message is no longer pending, delivery is skipped (treated as a successful no-op skip). Otherwise it attempts delivery to the target platform (LINE push API or Facebook Messenger send API) with a 10-second per-attempt timeout. On success the message is marked delivered, removed from the pending set, recorded as a real outbound conversation message (flagged as having been delayed, with original scheduled time and retry count retained), and the conversation's last-activity timestamp is advanced. Short-lived recall/cancel markers are cleaned up.
- Retry behavior: on a failed or timed-out send the system retries with escalating waits of approximately 1s, then 2s, then 4s (up to a small fixed number of total attempts). Idempotency is re-checked so an already-recorded message is never double-sent.
- Permanent failure: if all attempts fail, the message is marked failed, persisted with a failure reason, and copied into a dead-letter holding area (with its own retry on the dead-letter write). A failed event may be broadcast.
- Side Effects: a permanent outbound message record is created on success; conversation last-activity timestamp updated; counters/metrics updated; dead-letter entry written on permanent failure; real-time sent / recalled(skipped) / failed events emitted.
- Invariants & Guarantees: at-most-once delivery via an idempotency check against existing delivered records; cancellation always wins if performed before firing; a message that already passed its fire time can no longer be cancelled; due messages for a conversation are processed together and each is handled independently (one failure does not block others).

#### Failed scheduled-message inspection
- Invocation: operational inspection of permanently failed scheduled messages.
- Behavior: returns the list of permanently-failed messages held for that conversation, newest-failure-first, each summarized with identifier, content preview (first 100 chars), platform, failure time, failure reason, retry count, original fire time, and conversation identifier, plus a total count.

#### Scheduled-message operational metrics
- Invocation: operational inspection of scheduled-message delivery metrics.
- Behavior: returns operational counters and derived rates for that conversation's scheduler: totals for scheduled / sent / failed / cancelled / retried, dead-letter write successes and failures, timer trigger count, idempotency-prevented duplicate count; per-platform success/failure counts and success-rate percentages; current pending count, dead-letter size, and next armed timer time; latency and retry-count distribution summaries; and overall success rate, total processed, and retry rate.

### Data Concepts (neutral)

- Scheduled message: the central entity. Carries a unique message identifier, the owning conversation identifier, the sending agent identifier, the text content, a message type (text / image / video / audio / file), the target platform (line or facebook), the recipient's platform-specific user identifier, an absolute scheduled fire time, a lifecycle state, optional structured metadata (e.g. recipient platform id, platform, chosen delay, media URL, original-vs-rescheduled times, who rescheduled, retry count, last retry time), and creation / update timestamps. After successful delivery it also gains a delivered timestamp; after cancellation a cancelled timestamp; after failure a failure time and failure reason.
- Recall marker: a short-lived cache record indicating a scheduled message is still recallable, carrying its recall deadline, the owning sender identifier, and platform; it auto-expires shortly after the fire time. Its presence/ownership/deadline govern whether a recall is allowed.
- Cancellation marker: a short-lived cache record indicating a message has been cancelled (with who cancelled it and when); its presence causes the firing step to skip delivery. It auto-expires after a few minutes.
- Delivered message record: when a delayed message is actually sent, a permanent outbound conversation message is created, attributed to the agent, marked sent, annotated to indicate it was a delayed send (retaining original scheduled time and retry count).
- Dead-letter entry: a copy of a permanently-failed message retained for inspection, enriched with failure time, failure reason, failure detail, retry count, the write attempt number, and environment/timestamp context.
- Operation log entry: an audit record of lifecycle actions (e.g. cancelled, sent, failed) tied to a message, an acting user, an action label, and a timestamp.
- Activity log entry: a separate higher-level audit trail recording "delayed message scheduled" and "delayed message cancelled" actions with acting user, role, resource references, and request context (IP, user agent), written best-effort.
- Recipient/conversation context: delivery resolves the conversation's customer to obtain the platform and platform user identifier when not supplied directly.

Soft-delete semantics: lifecycle is modeled via state transitions and short-lived markers rather than hard deletion at the business layer; cancelled and delivered messages are removed from the active pending set, while failed messages are retained (as failed records and dead-letter copies).

### State & Lifecycle

A scheduled message moves through these states:
- pending: created and awaiting its fire time. This is the only state from which cancellation, reschedule, and firing may occur.
- cancelled: terminal. Reached only from pending and only strictly before the fire time, triggered by the original sender (legacy) or any authorized caller on the real-time buffer. A cancelled message is never delivered.
- sent: terminal. Reached from pending when platform delivery succeeds (possibly after retries). Produces a permanent outbound message record.
- failed: terminal. Reached from pending when all delivery attempts are exhausted; produces a dead-letter copy.

Additional rules:
- Reschedule keeps the message in pending while moving its fire time and recall deadline forward/back within the 1-120 second bound; only the sender may reschedule and only while pending.
- The recall deadline equals the fire time; cancellation/recall is invalid once the fire time is reached.
- A skip (delivery suppressed because a cancellation marker exists or the record is no longer pending) is treated as a successful no-op rather than a failure.

### Real-time / Event Behavior

The area emits four real-time event types to clients. Each event carries an event type, the conversation identifier, the message identifier, the acting agent identifier, a data payload, and a priority. Events are delivered both to subscribers of the conversation and to the acting user; countdown events are transient (short retention) while the others are persisted with longer retention.

- countdown event ("delayed_message_countdown"): emitted when a message is scheduled or rescheduled (and optionally for periodic countdown ticks). Payload includes a content preview (truncated), message type, platform, chosen delay, scheduled fire time, recall deadline, a flag that the countdown has started, remaining seconds, a "can recall" flag, and who scheduled it. Priority normal (or low for periodic ticks). Audience: the conversation subscribers and the scheduling agent.
- sent event ("delayed_message_sent"): emitted when a delayed message is successfully delivered at fire time. Payload includes a content preview, message type, platform, processed/actual sent time, delivery status "sent", a delay-completed flag, the original scheduled time, and a processing reference. Priority normal.
- recalled event ("delayed_message_recalled"): emitted when a message is successfully recalled, and also used to signal a skipped (cancelled-before-processing) delivery. Payload for a recall includes who recalled it, the recall time, an (optionally truncated) original content snapshot, original message type, a success flag, and a reason such as manual recall; the skip variant carries a skip reason and a skipped flag. Priority high for recall, low for skip.
- failed event ("delayed_message_failed"): emitted when a send permanently fails, a recall attempt fails, or queue processing fails. Payload includes the failure reason, the operation that failed (send / recall / queue processing), the failure time, and delivery status "failed". Priority high.

All event broadcasts are best-effort: a broadcast failure is logged but never aborts or rolls back the underlying scheduling, cancellation, reschedule, or delivery operation.

## Auto-Reply

### Purpose
This area provides automated, rule-driven responses to inbound customer messages on messaging channels (primarily the LINE channel). Operators define reply rules that fire on configurable triggers — a greeting when a customer first follows/contacts, a keyword/pattern match in a text message, an outside-business-hours condition, or an unconditional catch-all fallback. Each rule carries one or more trigger conditions and one or more reply actions (text, image, or rich/structured message). The area also manages per-team business-hours schedules (used to decide "off hours"), exposes a read-only audit log of every automated reply that was sent, and guarantees a customer message does not receive duplicate automated replies on webhook redelivery. Management endpoints are authenticated operator/agent actions; rule evaluation and dispatch are triggered internally by inbound-message and follow-event processing.

### Operations

#### List reply rules — GET /api/auto-reply/rules
- Invocation: Authenticated client request.
- Inputs (query): `scope` (optional; the only meaningful value is `global` — selects organization-wide rules not bound to a team); `teamId` (optional integer; selects rules owned by that team); `page` (optional integer, default 1, floored at 1); `pageSize` (optional integer, default 50, capped at 100).
- Preconditions & Authorization: Requires a valid authenticated session. When `scope` is not `global`, a team must be resolvable — explicit `teamId`, else a request-context team, else the caller's primary team. If none resolves, the request is rejected.
- Behavior: Returns the matching, non-deleted rules ordered by priority (ascending; lower number = higher priority), paginated. Each returned rule includes its full set of conditions and its actions (actions ordered by their sort order). Global listing returns only rules with no owning team; team listing returns only that team's rules.
- Success Output: 200. A paginated envelope: an items array plus pagination metadata (current page, page size, total count). Each rule item carries: rule identifier; owning-team identifier (null for global); display name; trigger type; priority; active flag; push-fallback-allowed flag; created-by identifier; created/updated/deleted timestamps; an array of conditions (each with: identifier, condition type, match value, case-sensitive flag, match mode); an array of actions (each with: identifier, action type, content payload as a JSON string, sort order).
- Error Conditions: Missing resolvable team when not global -> 400 with a message indicating a team identifier is required (or to use global scope). Unexpected failure -> standardized error response.
- Invariants: Soft-deleted rules are never returned. Ordering is by priority ascending.

#### Create reply rule — POST /api/auto-reply/rules
- Invocation: Authenticated client request.
- Inputs (query): `scope` (optional; `global` creates an org-wide rule with no owning team); `teamId` (optional integer). Inputs (body): `name` (required, non-blank string; trimmed); `triggerType` (required; one of greeting/welcome, keyword, off-hours, fallback); `priority` (optional integer, default 100); `isActive` (optional boolean, default true); `allowPushFallback` (optional boolean, default false); `conditions` (optional array; each item: condition type one of exact/contains/regex/message-type, match value string, optional case-sensitive flag default false, optional match mode one of any/all default any); `actions` (optional array; each item: action type one of reply-text/reply-image/reply-flex, content as a string, optional sort order — defaults to the item's position in the array).
- Preconditions & Authorization: Valid authenticated session. When not global, a team must resolve (explicit, context, or primary team), else rejected.
- Behavior: Creates the rule, then its conditions, then its actions, recording the creator and timestamps. Validates trigger type, every condition type, and every action type against fixed allowed sets before persisting the dependent rows. After persistence, the cached rule set for the affected scope (the specific team, or the global set) is invalidated so subsequent evaluations see the new rule. Re-reads and returns the created rule with its conditions and actions.
- Success Output: 201. The created rule object in the same shape as a list item (rule fields plus conditions and actions arrays).
- Side Effects: New rule, conditions, and actions persisted; cached rule set for the scope invalidated.
- Error Conditions: Blank/missing name -> 400. Invalid trigger type -> 400 naming allowed values. Invalid condition type -> 400 naming the bad value. Invalid action type -> 400 naming the bad value. Missing resolvable team when not global -> 400. Malformed JSON body -> 400 "Invalid JSON". Other failures -> standardized error.
- Invariants: Default priority is 100; lower priority value evaluates first. Default push-fallback is off.

#### Update reply rule — PUT /api/auto-reply/rules/{id}
- Invocation: Authenticated client request.
- Inputs (path): `id` (rule identifier, integer). Inputs (body, all optional — partial update): `name`, `triggerType`, `priority`, `isActive`, `allowPushFallback`, `conditions`, `actions` (same item shapes as create).
- Preconditions & Authorization: Valid authenticated session. The target rule must exist and not be soft-deleted.
- Behavior: Updates only the scalar fields present in the body and refreshes the updated timestamp. If a `conditions` array is supplied, the rule's entire existing condition set is replaced wholesale with the supplied set (an empty array clears all conditions). If an `actions` array is supplied, the entire action set is likewise replaced. Fields not present in the body are left unchanged. The cached rule set for the rule's owning scope is invalidated. Re-reads and returns the updated rule with its conditions and actions.
- Success Output: 200. The updated rule object (rule fields plus conditions and actions).
- Side Effects: Rule row updated; conditions/actions replaced if those arrays were provided; cached rule set invalidated.
- Error Conditions: Non-numeric id -> 400 "Invalid rule ID". Rule not found / soft-deleted -> 404. Malformed JSON -> 400 "Invalid JSON". Other failures -> standardized error.
- Invariants: Supplying a conditions or actions array is a full replace, not a merge. Note: this update does not re-validate trigger/condition/action types the way create does.

#### Delete reply rule — DELETE /api/auto-reply/rules/{id}
- Invocation: Authenticated client request.
- Inputs (path): `id` (rule identifier, integer).
- Preconditions & Authorization: Valid authenticated session. The target rule must exist and not already be soft-deleted.
- Behavior: Soft-deletes the rule (marks a deletion timestamp and sets the rule inactive). The rule is retained but excluded from all future listings and evaluations. The cached rule set for the rule's scope is invalidated.
- Success Output: 200. An object echoing the deleted rule's identifier.
- Side Effects: Rule marked deleted and inactive; cached rule set invalidated. Audit log rows referencing the rule remain (the log listing left-joins to the rule's name, so a deleted rule's past logs still display).
- Error Conditions: Non-numeric id -> 400 "Invalid rule ID". Rule not found / already deleted -> 404. Other failures -> standardized error.
- Invariants: Deletion is soft (never physical removal).

#### Get business-hours schedule — GET /api/auto-reply/schedules
- Invocation: Authenticated client request.
- Inputs (query): `teamId` (optional integer; falls back to request-context team, then caller's primary team).
- Preconditions & Authorization: Valid authenticated session and a resolvable team.
- Behavior: Returns all schedule entries for the team, ordered by day-of-week. A schedule entry represents one weekday's open window.
- Success Output: 200. An array of schedule entries, each with: identifier; team identifier; day-of-week (0–6, 0 = Sunday); start time (24-hour HH:mm); end time (HH:mm); timezone; active flag.
- Error Conditions: No resolvable team -> 400 "teamId is required". Other failures -> standardized error.

#### Replace business-hours schedule — POST /api/auto-reply/schedules
- Invocation: Authenticated client request.
- Inputs (query): `teamId` (optional integer; same fallback chain). Inputs (body): `timezone` (optional string, default "Asia/Taipei"); `schedules` (required non-empty array; each item: `dayOfWeek` integer 0–6, `startTime` HH:mm, `endTime` HH:mm, optional `isActive` default true).
- Preconditions & Authorization: Valid authenticated session and resolvable team.
- Behavior: Validates every entry, then replaces the team's entire schedule set with the supplied entries (the prior set for that team is removed and the new set inserted). All inserted entries share the one supplied/default timezone. The cached schedule for the team is invalidated.
- Success Output: 200. The array of newly stored schedule entries (same shape as the GET response).
- Side Effects: Team's schedule set fully replaced; cached schedule invalidated.
- Error Conditions: Missing/empty/non-array `schedules` -> 400 "schedules array is required". `dayOfWeek` outside 0–6 -> 400 naming the bad value. `startTime` or `endTime` not matching strict 24-hour HH:mm (00–23 hours, 00–59 minutes) -> 400 naming the bad value. No resolvable team -> 400. Malformed JSON -> 400 "Invalid JSON". Other failures -> standardized error.
- Invariants: This is a wholesale replace per team, not an upsert of individual days. A day not included in the payload becomes absent (treated as closed). End time earlier than or equal to start time is accepted and interpreted as an overnight window that crosses midnight.

#### List automated-reply audit logs — GET /api/auto-reply/logs
- Invocation: Authenticated client request (read-only).
- Inputs (query): `teamId` (optional integer; falls back to context team, then primary team); `page` (optional, default 1, floored at 1); `pageSize` (optional, default 50, capped at 100); `ruleId` (optional integer filter); `platform` (optional filter; must be one of the allowed channel values — line, facebook, whatsapp); `dateFrom` (optional ISO-8601 date/time lower bound on log creation).
- Preconditions & Authorization: Valid authenticated session and resolvable team.
- Behavior: Returns audit log entries for automated replies whose underlying rule is either owned by the caller's team OR is a global rule (team-unbound), newest first, paginated. Applies the optional rule, platform, and date-from filters. Also computes a count of entries created since the start of the current day. Note: the underlying values are bound as parameters, so the filters cannot be used for injection; the allowlist/format checks reject malformed input with a 400.
- Success Output: 200. A data object containing: an items array; current page; page size (returned under two keys for compatibility); total matching count; today's count; total pages; and next/previous-page boolean flags. Each item carries: log identifier; rule identifier; rule display name (from the joined rule, may be present even for a deleted rule); conversation identifier; customer identifier; the trigger content that caused the reply; a summary of the response content; the matched-condition descriptor; platform; reply delivery method; creation timestamp.
- Error Conditions: No resolvable team -> 400 "teamId is required". `ruleId` not an integer -> 400. `platform` not in the allowed set -> 400 listing allowed values. `dateFrom` not a valid ISO-8601 date -> 400. Other failures -> standardized error.
- Invariants: Read-only; no log creation/mutation endpoint is exposed to clients (logs are written internally during dispatch). Team scope includes global-rule logs.

#### Health checks — GET /api/auto-reply/rules/health, GET /api/auto-reply/schedules/health, GET /api/auto-reply/logs/health
- Invocation: Authenticated client request.
- Behavior: Each returns a static healthy indicator with a component label and a current timestamp.
- Success Output: 200 with a success envelope containing status "healthy".

#### Auto-reply evaluation for inbound messages
- Invocation: triggered as a side effect of processing an inbound text/message webhook event for a conversation. Inputs are derived from the inbound message: message content, message type, channel/platform, conversation identifier, the conversation's assigned team (may be null), an optional reply token, the customer identifier, the customer's platform user identifier, and the platform message identifier.
- Behavior (observable): Loads the applicable active, non-deleted rules — the global rule set plus the team's rule set if a team is known — merged and evaluated in priority order (ascending; at equal priority a team-specific rule is considered before a global one). The first rule whose trigger is eligible AND (for keyword rules) whose conditions match wins; remaining rules are not considered. Eligibility per trigger type:
  - Greeting/welcome rules are never eligible here (they fire only via the follow-event path below).
  - Keyword rules are eligible only for non-empty text messages, then must satisfy their conditions; a keyword rule with no conditions can never match.
  - Off-hours rules are eligible only when the current time is OUTSIDE the configured business hours (schedule looked up by the rule's own team, falling back to the conversation's team; if no schedule exists at all, the system is treated as always within business hours, so off-hours rules do not fire).
  - Fallback rules are always eligible (catch-all).
  When a rule matches, the system first ensures this exact inbound platform message has not already been auto-replied (duplicate guard, see Invariants). If clear to proceed, it executes the rule's reply actions toward the channel, records the delivery outcome, and — on success — persists the reply as a system-authored conversation message, writes an audit log entry, and broadcasts the reply in real time. If no rule matches, nothing is sent.
- Side Effects: On a successful send: a channel message is dispatched to the customer; a delivery-ledger entry is marked succeeded; a system/auto-reply message is stored in the conversation; an audit log entry is created; a real-time new-message event is broadcast. Persisting the conversation message, the audit log, and the broadcast each fail independently and non-fatally — a failure in one does not block the others or the overall result.
- Error/edge results: Evaluation never throws to the caller; any server error yields a non-matched result carrying an error description. If actions cannot be built or there are no valid actions, the result is a failure with an explanatory error and no send.
- Invariants: First-match-wins by priority. Duplicate-send protection: keyed by channel + platform message identifier — if a prior attempt already succeeded, the rule is reported as matched but no second send occurs; if a prior attempt is still pending, the duplicate is skipped with a "pending" notice; otherwise an attempt is reserved (attempt counter incremented) before sending. This makes webhook redelivery safe (at-most-once successful auto-reply per inbound message). When invoked without a platform message identifier (e.g. internal flows), the duplicate guard is bypassed and the send proceeds.

#### Auto-reply evaluation for follow/greeting events
- Invocation: triggered as a side effect when a customer follows or first connects. Inputs: the conversation's team (may be null), a reply token, the conversation identifier, the customer identifier, and the customer's platform user identifier.
- Behavior (observable): Loads applicable active rules and considers only greeting/welcome-type rules. If none exist, nothing happens. Otherwise the highest-priority welcome rule is selected and its actions are sent. On success, the same post-send side effects occur as in message evaluation (stored system message, audit log with trigger marked as a follow event and condition descriptor noting the welcome trigger, real-time broadcast).
- Side Effects: Same success side effects as message evaluation. This path does not consult the duplicate-delivery ledger.
- Error results: Errors are caught and reported as a non-matched/error result without throwing.

#### Auto-reply retry for redelivered messages
- Invocation: triggered when a webhook redelivers a platform message already saved as a duplicate, to recover a missed auto-reply. Inputs: channel, platform message identifier, optional reply token, platform user identifier.
- Behavior (observable): Looks up the previously stored original message and its conversation/customer/team, then runs the standard message evaluation for it. The delivery-ledger duplicate guard ensures it will not produce a second successful send if the original already succeeded or is pending.
- Error results: If the original message cannot be found, returns a non-matched result with an explanatory error.

### Data Concepts (neutral)
- Reply rule: An automated-response definition. Carries a unique identifier; an optional owning-team reference (absent = global/org-wide); a human-readable display name; a trigger category (greeting/welcome, keyword, off-hours, or fallback); a priority number (lower runs first; default 100); an active flag; a push-fallback-allowed flag; the identity of who created it; creation/update timestamps; and a soft-deletion timestamp. Owns a set of conditions and a set of actions. Soft-deleted rules persist but are invisible to listing and evaluation.
- Condition: A single trigger test belonging to a rule. Carries an identifier, the owning rule reference, a condition kind (exact-equality, substring-contains, regular-expression, or message-type equality), a comparison value, a case-sensitivity flag, and a match mode (any/all). The match mode is taken from the rule's conditions collectively (all conditions of one rule share the same mode): "any" requires at least one condition to pass, "all" requires every condition to pass. Exact matching compares against the trimmed message text; contains/regex run against the message text; message-type checks the message kind (e.g. text, image, sticker). An invalid regular expression simply never matches rather than erroring.
- Action: A reply step belonging to a rule. Carries an identifier, the owning rule reference, an action kind (plain text, image, or rich/structured message), a content payload stored as a JSON string, and a sort order. Actions execute in ascending sort order. The content payload's shape depends on the kind: text carries a text string; image carries an image URL and an optional preview URL (preview defaults to the main URL); the rich/structured kind carries channel-native structured content with an optional alternative-text label (defaulting to a generic label).
- Business-hours schedule entry: One weekday's open window for a team. Carries an identifier, the team reference, a day-of-week number (0–6, Sunday = 0), an open time and a close time (24-hour HH:mm), a timezone, and an active flag. A team's entries collectively define its weekly business hours and share a single timezone.
- Audit log entry: An immutable record of one automated reply that was sent. Carries an identifier, the rule reference, the conversation reference, the customer reference, the triggering message content, a human-readable summary of the response, a descriptor of what matched (for keyword rules, the list of condition kinds and values; for other triggers, the trigger type), the channel, the delivery method used, and a creation timestamp.
- Delivery ledger entry: A per-inbound-message dispatch record used to prevent duplicate sends. Keyed by channel plus the platform message identifier. Tracks a status (pending, succeeded, or failed), the associated rule/conversation/customer, an attempt counter, the last error, and attempt/sent/update timestamps.

### State & Lifecycle
- Reply rule lifecycle: created (active by default) -> may be toggled active/inactive and re-prioritized via update -> soft-deleted (terminal: marked deleted and forced inactive, retained for historical logs but excluded from all listing and evaluation). Only active, non-deleted rules participate in evaluation.
- Delivery ledger status machine (per inbound message + channel): no entry -> pending (a send attempt is reserved) -> succeeded (terminal for the purpose of dedup; further attempts are no-ops) OR failed. A pending or succeeded state blocks a concurrent or repeat attempt from sending again; a failed state allows a new attempt (incrementing the attempt counter).
- Business hours: derived state, not stored as a status. "Within business hours" is true when the current time in the schedule's timezone falls inside an active entry for the current weekday (windows where close time is at or before open time are interpreted as crossing midnight). If no schedule entries exist, the system is treated as always within business hours, which suppresses off-hours rules.

### Real-time / Event Behavior
- On a successful automated reply (from message, welcome, or retry dispatch), the area broadcasts a real-time new-message event for the affected conversation. The broadcast payload presents the reply as a delivered message authored by the system (labeled as an auto-reply, surfaced with an agent-type sender for client compatibility), including content, message kind (text), sender label, channel, a timestamp, and a delivered delivery status; it is scoped to the conversation and, when known, the team audience. Recipients are the real-time subscribers of that conversation/team (operators viewing the conversation).
- Broadcast emission is best-effort: a broadcast failure is non-fatal and does not affect the send result, the stored message, or the audit log.
- This area consumes (is triggered by) inbound-message and follow webhook events from the channel-integration area; it does not consume any client-facing events. Management operations (create/update/delete rule, replace schedule) emit no real-time events but do invalidate the corresponding cached rule/schedule sets so that subsequent evaluations reflect the change within at most the cache lifetime.

## Tags & Labeling

### Purpose
This area lets support staff define a shared set of reusable labels ("tags") and apply them to customers, and view how those labels relate to conversations. Each label carries a name, an optional color and description, and is globally visible to all authenticated staff. The area provides full label lifecycle management (create, read, update, soft-delete, bulk activation/color changes), label-to-customer assignment management, usage statistics, listings of the customers and conversations associated with a label, and a separate facility for attaching labels directly to individual conversations. Label assignment changes are broadcast in real time so other connected clients stay in sync.

### Operations

All operations below require an authenticated request bearing a valid session token unless noted. Two route families exist:
- A label-management family rooted at `/api/tags`.
- A customer-label-association family rooted at `/api/customers` (plus one label-discovery endpoint).
- A conversation-label family rooted at `/api/conversations`.

Standard envelope: success responses are JSON objects carrying `success: true`, a `data` payload, a `message` string, a server `timestamp`, and a generated `requestId`. Error responses carry `success: false`, an `error` message string, `timestamp`, and `requestId`; validation errors additionally embed a `data` object with a machine-readable `code` of `VALIDATION_ERROR` and an `errors` array of `{ field, message }`.

#### Label-management health probe — GET /api/tags/health
- Invocation: authenticated client request, but this path is explicitly exempt from auth enforcement.
- Inputs: none.
- Preconditions & Authorization: none; publicly reachable.
- Behavior: returns a static operational marker.
- Success Output: 200 with `data` of `{ status: "healthy", handler, timestamp }` and a message indicating the handler is operational.
- Error Conditions: none expected.

#### List labels — GET /api/tags
- Invocation: authenticated client request.
- Inputs (query): `page` (positive integer, default 1, values below 1 coerced up to 1); `pageSize` (positive integer, default 50, capped at 100); `search` (optional free text matched as a case-insensitive substring against label name or description).
- Preconditions & Authorization: any authenticated user. No team scoping — all active, non-deleted labels are visible to everyone.
- Behavior: returns one page of active labels (excluding soft-deleted ones) ordered alphabetically by name ascending. Each label includes a count of distinct non-deleted customers carrying it and a count of distinct non-deleted conversations belonging to those customers.
- Success Output: 200. `data` is a paginated container: `items` (array of label objects), plus `page`, `pageSize`, `limit`, `total`, `totalPages`, `hasNext`, `hasPrev`. Each label object: `id` (number), `name`, `color`, `description`, `teamId` (always null in this listing), `teamName` (always null here), `isActive` (boolean), `createdBy`, `createdByName` (null here), `customerCount` (number), `conversationCount` (number), `createdAt`, `updatedAt`.
- Error Conditions: server failures return 500 with a generic error message.

#### Create label — POST /api/tags
- Invocation: authenticated client request.
- Inputs (body): `name` (string, required, must be non-empty after trimming); `color` (optional string, default a standard blue; must be a 3- or 6-digit HEX color with leading `#`); `description` (optional string); a `teamId` field is accepted but ignored.
- Preconditions & Authorization: any authenticated user.
- Behavior: creates a new globally-scoped label. The color is normalized to uppercase 6-digit HEX form. The name is checked for uniqueness against other active labels. The creating user's identifier is recorded as the creator.
- Success Output: 201. `data` is the created label: `id`, `name`, `color`, `description`, `teamId` (null), `isActive`, `createdBy`, `customerCount` (0), `conversationCount` (0), `createdAt`, `updatedAt`.
- Side Effects: persists a new label; records a reversible audit entry describing the creation (action "tag create"), capturing prior absence and the new label state so the action can later be undone.
- Error Conditions: missing/blank name -> 400 with message that name is required. Invalid color format -> 422 validation error on the `color` field. Duplicate active name -> 409 conflict. Malformed JSON body -> 400 "Invalid JSON". Insert failure -> 500.

#### Get single label — GET /api/tags/:id
- Invocation: authenticated client request.
- Inputs (path): `id` label identifier.
- Preconditions & Authorization: any authenticated user.
- Behavior: returns the label detail including its associated team display name (if any), creator display name, distinct customer count, and distinct non-deleted conversation count.
- Success Output: 200. `data`: `id`, `name`, `color`, `description`, `teamId`, `teamName`, `isActive` (boolean), `createdBy`, `createdByName`, `customerCount`, `conversationCount`, `createdAt`, `updatedAt`. (Note: this endpoint can return a label even if soft-deleted, since it does not exclude deleted records.)
- Error Conditions: nonexistent id -> 404 not found. Server failure -> 500.

#### Update label — PUT /api/tags/:id
- Invocation: authenticated client request.
- Inputs (path): `id` (must be a positive integer). Inputs (body, all optional): `name`, `color` (HEX-validated when provided), `description`, `isActive` (boolean).
- Preconditions & Authorization: any authenticated user.
- Behavior: applies only the fields that differ from current values. Color, when supplied, is normalized to uppercase 6-digit HEX. If a new name is supplied and differs, it is checked for uniqueness among other active labels. If no field actually changes, the existing record is returned unchanged with a message indicating no change (no write, no audit entry). Otherwise the changed fields plus an updated timestamp are persisted atomically alongside an audit entry.
- Success Output: 200. `data` is the full updated label with refreshed counts: `id`, `name`, `color`, `description`, `teamId`, `isActive` (boolean), `createdBy`, `customerCount`, `conversationCount`, `createdAt`, `updatedAt`. Unchanged case returns the same shape (counts reported as 0).
- Side Effects: persists changed fields; records a reversible audit entry (action "tag update") capturing the prior and new values of only the changed fields, enabling rollback.
- Error Conditions: non-integer or non-positive id -> 400 "Invalid tag id". Label missing or already soft-deleted -> 404. Invalid color -> 422 on `color`. Duplicate name -> 422 on `name`. Post-update re-read failure -> 500.

#### Soft-delete label — DELETE /api/tags/:id
- Invocation: authenticated client request.
- Inputs (path): `id` (must be a positive integer).
- Preconditions & Authorization: any authenticated user.
- Behavior: marks the label inactive and stamps it as deleted (soft delete) rather than removing it. Customer/conversation associations are not removed by this call.
- Success Output: 200 with `data: null` and a deletion-success message.
- Side Effects: persists the soft-delete state and refreshed timestamp atomically with a reversible audit entry (action "tag delete") capturing prior active/deleted state for rollback.
- Error Conditions: non-integer or non-positive id -> 400 "Invalid tag id". Label missing or already soft-deleted -> 404. Server failure -> 500.

#### Bulk label operation — POST /api/tags/bulk
- Invocation: authenticated client request.
- Inputs (body): `operation` (one of "activate", "deactivate", "update_color"); `tagIds` (non-empty array of numeric identifiers; each element must be a number or an all-digits string); `data` (object; for "update_color" must contain a `color` value).
- Preconditions & Authorization: any authenticated user.
- Behavior: applies the chosen change to every listed label. "activate" sets them active, "deactivate" sets them inactive, "update_color" sets their color to the provided value (note: the bulk color value is stored as supplied and is not HEX-validated or normalized). Each affected label's update timestamp is refreshed.
- Success Output: 200 with `data: null` and a message naming the completed operation.
- Side Effects: persists the bulk state change. (No per-label reversible audit entries are recorded for bulk operations.)
- Error Conditions: empty/non-array `tagIds` -> 422 on `tagIds`. Any identifier failing the numeric format check -> 400 "Invalid tag ID format detected". Missing color for "update_color" -> 422 on `data.color`. Unknown operation -> 422 on `operation`. Malformed JSON -> 400 "Invalid JSON".

#### Label usage statistics — GET /api/tags/:id/stats
- Invocation: authenticated client request.
- Inputs (path): `id`.
- Preconditions & Authorization: any authenticated user.
- Behavior: returns aggregate usage for the label: customer totals broken down by channel, conversation totals broken down by lifecycle status, a daily assignment trend over the trailing 30 days, and the top assigning staff over the trailing 30 days.
- Success Output: 200. `data`: `tagInfo` `{ id, name, color }`; `customers` `{ total, byPlatform: { line, facebook } }`; `conversations` `{ total, active, closed }`; `usageTrend` (array of `{ date, assignments }`, most recent first, up to 30 entries); `topAssigners` (array of `{ name, assignments }`, up to 10 entries, highest first).
- Error Conditions: nonexistent id -> 404. Server failure -> 500.

#### List a label's customers — GET /api/tags/:id/customers
- Invocation: authenticated client request.
- Inputs (path): `id`. Inputs (query): `page` (positive integer, default 1); `limit` (default 50, capped at 100).
- Preconditions & Authorization: any authenticated user. The label must be active/non-deleted to be found here.
- Behavior: returns customers currently carrying the label (excluding soft-deleted customers), ordered by assignment time descending.
- Success Output: 200. `data`: `customers` (array, each: `id`, `platform`, `platform_user_id`, `display_name`, `avatar_url`, `email`, `phone`, `created_at`, `assigned_at`, `assigned_by`) and `pagination` `{ page, limit, total, totalPages }`.
- Error Conditions: label not found (or soft-deleted) -> 404. Server failure -> 500.

#### List a label's conversations — GET /api/tags/:id/conversations
- Invocation: authenticated client request.
- Inputs (path): `id`. Inputs (query): `page` (positive integer, default 1); `limit` (default 20, capped at 100).
- Preconditions & Authorization: any authenticated user. Label must be active/non-deleted.
- Behavior: returns distinct conversations belonging to customers carrying the label (excluding soft-deleted customers and conversations), ordered by assignment time descending. Because labels attach to customers, this surfaces all conversations of any customer holding the label.
- Success Output: 200. `data`: `conversations` (array, each: `id`, `status`, `channel`, `created_at`, `updated_at`, `customer_name`, `customer_avatar`, `customer_platform`, `assigned_at`, `assigned_by`) and `pagination` `{ page, limit, total, totalPages }`.
- Error Conditions: label not found (or soft-deleted) -> 404. Server failure -> 500.

#### Discover available labels (selector) — GET /api/customers/tags/available
- Invocation: authenticated client request; used to populate label-picker UIs.
- Inputs (query): `page` (default 1); `pageSize` (default 100); `search` (optional substring, matched against name or description, with wildcard characters in the search treated literally); `includeGlobal` (default "true").
- Preconditions & Authorization: authenticated user. Team scoping applies here (unlike the `/api/tags` listing): a non-admin caller with an assigned primary team sees labels belonging to their team and, when `includeGlobal` is "true", also global (team-less) labels; with `includeGlobal` "false" they see only their team's labels. An admin sees all labels by default, or only team-scoped labels when `includeGlobal` is "false". Only active labels are returned.
- Behavior: returns one page of matching active labels ordered alphabetically by name, each with distinct customer count and distinct non-deleted conversation count.
- Success Output: 200. Top-level object with `success: true`, `data` (array of label objects: `id`, `name`, `color`, `description`, `teamId`, `isActive`, `createdBy`, `createdAt`, `updatedAt`, `customerCount`, `conversationCount`), `pagination` `{ page, limit, total, totalPages }`, and a message.
- Error Conditions: server failure -> 500.

#### Get a customer's labels — GET /api/customers/:customerId/tags
- Invocation: authenticated client request.
- Inputs (path): `customerId` (validated as an integer by route middleware; non-integer rejected before the handler).
- Preconditions & Authorization: authenticated user.
- Behavior: returns all active labels currently assigned to the customer, ordered by assignment time descending.
- Success Output: 200. `data` is an array, each: `id`, `name`, `color`, `description`, `teamId`, `assignedAt`, `assignedBy`.
- Error Conditions: invalid `customerId` -> rejected by validation middleware (bad-request). Customer not found -> 404. Server failure -> 500.

#### Add labels to a customer — POST /api/customers/:customerId/tags
- Invocation: authenticated client request.
- Inputs (path): `customerId` (integer). Inputs (body): `tagIds` (non-empty array of label identifiers).
- Preconditions & Authorization: authenticated user; the caller's identifier must be present (recorded as assigner).
- Behavior: validates that every supplied label exists and is active; assigns only the labels not already attached to the customer (idempotent — labels already present are skipped). Records the assigning user and assignment timestamp for each newly added association.
- Success Output: 200. `data`: `{ added: <newly added count>, alreadyExists: <count of supplied labels already present> }` plus a message.
- Side Effects: persists new associations atomically with one reversible audit entry per added association (action "tag assign"). On any added label, broadcasts a customer-label-change event (operation "add") to connected staff; broadcast failure is non-fatal and does not affect the response.
- Error Conditions: empty/non-array `tagIds` -> 422 on `tagIds`. Customer not found -> 404. Any supplied label invalid or inactive -> 422 on `tagIds` ("Some tag IDs are invalid or inactive"). Missing caller identity when there are labels to add -> 401. Server failure -> 500.

#### Remove labels from a customer — DELETE /api/customers/:customerId/tags
- Invocation: authenticated client request.
- Inputs (path): `customerId` (integer). Inputs (body): `tagIds` (non-empty array).
- Preconditions & Authorization: authenticated user.
- Behavior: removes the specified label associations from the customer. Each existing association is captured before removal so the action is reversible; associations not present are simply absent (removal is idempotent/no-op for those).
- Success Output: 200 with `data: null` and a message stating how many labels were removed (count equals the size of the requested list).
- Side Effects: deletes associations atomically with one reversible audit entry per previously-existing association (action "tag unassign"). Broadcasts a customer-label-change event (operation "remove"); broadcast failure is non-fatal.
- Error Conditions: empty/non-array `tagIds` -> 422 on `tagIds`. Customer not found -> 404. Server failure -> 500.

#### Replace a customer's labels — PUT /api/customers/:customerId/tags
- Invocation: authenticated client request.
- Inputs (path): `customerId` (integer). Inputs (body): `tagIds` (array; may be empty to clear all labels).
- Preconditions & Authorization: authenticated user; caller identity required when assigning a non-empty set.
- Behavior: replaces the customer's entire label set with the supplied set. All current associations are removed and the new set inserted. An empty `tagIds` clears all labels.
- Success Output: 200. `data`: `{ totalTags: <size of new set> }` plus a message.
- Side Effects: persists the replacement. Records a single (non-reversible, fire-and-forget) audit entry describing the resulting set (action "tag assign", operation "set"), including customer and label names; failure to log is swallowed. Broadcasts a customer-label-change event (operation "set"); broadcast failure is non-fatal.
- Error Conditions: `tagIds` not an array -> 422 on `tagIds`. Customer not found -> 404. Any supplied label invalid or inactive (when set is non-empty) -> 422 on `tagIds`. Missing caller identity when assigning a non-empty set -> 401. Server failure -> 500.

#### Get a conversation's labels — GET /api/conversations/:id/tags
- Invocation: authenticated client request.
- Inputs (path): `id` conversation identifier.
- Preconditions & Authorization: authenticated user.
- Behavior: returns the active labels attached directly to the conversation (a separate association from customer labels), with assignment metadata.
- Success Output: 200. `data` is an array, each: `id`, `name`, `color`, `description`, `assignedBy`, `assignedAt`, plus a message.
- Error Conditions: conversation not found -> 404 with `{ success: false, error: "Conversation not found" }`. Server failure handled by global error handler.

#### Add labels to a conversation — POST /api/conversations/:id/tags
- Invocation: authenticated client request.
- Inputs (path): `id`. Inputs (body): `tagIds` (non-empty array; values coerced to integers).
- Preconditions & Authorization: authenticated user.
- Behavior: attaches the supplied labels directly to the conversation, recording the assigning user; pre-existing identical associations are ignored (no duplicates created).
- Success Output: 200 with `{ success: true, message }`.
- Side Effects: persists the conversation-label associations. Broadcasts a conversation-label-update event (operation "add") to that conversation's audience; broadcast failure is non-fatal.
- Error Conditions: empty/non-array `tagIds` -> 422 on `tagIds`. Conversation not found -> 404. Server failure handled by global error handler.

#### Remove labels from a conversation — DELETE /api/conversations/:id/tags
- Invocation: authenticated client request.
- Inputs (path): `id`. Inputs (body): `tagIds` (non-empty array; coerced to integers).
- Preconditions & Authorization: authenticated user.
- Behavior: detaches the specified labels from the conversation.
- Success Output: 200 with `{ success: true, message }`.
- Side Effects: deletes the associations. Broadcasts a conversation-label-update event (operation "remove"); broadcast failure is non-fatal.
- Error Conditions: empty/non-array `tagIds` -> 422 on `tagIds`. Conversation not found -> 404. Server failure handled by global error handler.

### Data Concepts (neutral)

- **Label**: a reusable classification marker. Carries a numeric identifier, a display name, an optional color (stored as normalized uppercase HEX), an optional description, an active flag, an optional owning-team reference (effectively always global/team-less when created through this area), the identifier of the creating user, and creation/update timestamps. Supports soft delete via a deletion timestamp; a soft-deleted label is also marked inactive. Names are intended to be unique among active labels. Each label exposes derived counts: distinct customers carrying it and distinct non-deleted conversations belonging to those customers.
- **Customer-label association**: a link between a customer and a label, recording who assigned it and when. A customer may carry many labels; a label may be carried by many customers. Uniqueness per customer+label pair is enforced behaviorally (adding an existing pair is skipped). Removed associations are hard-deleted (with reversible audit capture), not soft-deleted.
- **Conversation-label association**: a separate, independent link between a conversation and a label, recording who assigned it and when. Distinct from customer-label associations. Adding duplicates is suppressed.
- **Label usage aggregates** (read-only, derived): per-channel customer totals, per-status conversation totals, daily assignment trend, and top assigning staff.

Relationship note: when listing a label's conversations or computing a label's conversation count, conversations are reached indirectly through the customers that hold the label (customer -> their conversations), not through direct conversation-label associations. The direct conversation-label associations are a separate facility surfaced only by the conversation-label endpoints.

### State & Lifecycle

- **Label lifecycle**: Active (created) -> may be toggled inactive/active via update or bulk operations -> Soft-deleted (terminal for normal listings; sets inactive + deletion timestamp). Soft-deleted labels are excluded from list, available-selector, customer-list, and conversation-list endpoints, but the single-label detail endpoint may still return them. Soft-delete and create/update are individually reversible through recorded audit entries; bulk operations are not.
- **Association lifecycle**: A customer-label or conversation-label association exists once assigned and ceases to exist when removed (or when a customer's set is replaced). There is no soft-delete state for associations. Customer-label add/remove operations are reversible via audit capture; the full-replace operation logs a single non-reversible activity entry.
- Authorization for label management and customer-label association management is uniform: any authenticated user may perform create/update/delete/assign/unassign; the only role-based differentiation is in the available-labels selector, where non-admins are restricted to their team's labels plus optionally global labels.

### Real-time / Event Behavior

- **Customer-label change event** (`customer_tags_updated`): emitted after a successful add, remove, or full-replace of a customer's labels (provided there is an effective change for "add"). Payload carries `customerId`, `operation` ("add" | "remove" | "set"), `tagIds` (the affected label identifiers), and `changedBy` (the acting user's identifier). Audience: broadcast to connected staff (admins and agents). The event is transient (short time-to-live, not persisted). Emission failure never blocks or fails the originating request.
- **Conversation-label change event** (`conversation_tags_updated`): emitted after a successful add or remove of labels on a conversation. Payload carries `operation` ("add" | "remove"), `tagIds`, an `updatedBy` object `{ id, name }`, and a `timestamp`, scoped to the affected conversation. Audience: that conversation's connected subscribers. Emission failure is non-fatal.
- No real-time events are emitted for label create/update/delete or bulk label operations; those changes are observed by re-querying.


---

# 3. Customer & Organization

## Customers

### Purpose
This area provides a directory of customer contacts that originate from external messaging platforms (each customer is a distinct end-user identity on a specific channel). It lets authenticated support staff list customers, look up a single customer (by internal identifier or by platform identity) together with that customer's conversation history, retrieve a selectable catalogue of tags, and manage the set of tags attached to each customer. Visibility is scoped by the caller's team so that staff only see customers belonging to their own team or to a shared (unassigned) pool, while administrators see everything.

All operations in this area are mounted under the base path `/api/customers` and require a valid authenticated session.

### Operations

#### List visible customers — GET /api/customers/
- Invocation: authenticated client request.
- Inputs: none (no query parameters are honored by this endpoint).
- Preconditions & Authorization: caller must present a valid auth token. No specific role required; results are filtered by the caller's team scope (see Invariants).
- Behavior: returns the set of customer records the caller is permitted to see. Administrators receive all customers; other staff receive only customers whose owning-team is the caller's primary team or whose owning-team is empty (shared pool). The visibility trim is applied after retrieval, so the returned count reflects only the visible subset.
- Success Output: HTTP 200 with a body containing `success: true`, a `data` object with `customers` (array of customer records) and `count` (integer number of visible customers), and a `timestamp` (ISO-8601 string). Each customer record carries: internal numeric identifier, originating platform, the platform-side user identifier, an optional display label, an optional avatar URL, optional email, optional phone, the owning-team identifier (nullable), a free-form metadata blob (nullable), and creation/update timestamps.
- Side Effects: none (read-only).
- Error Conditions: missing/invalid auth -> unauthorized. Unexpected server failure -> standardized error response with `success: false`, an `error` message, and a `timestamp`.
- Invariants & Guarantees: read-only; team-scoped visibility as above.

#### Get one customer with conversations — GET /api/customers/{customerId}
- Invocation: authenticated client request.
- Inputs: `customerId` path segment, required, must be a positive integer.
- Preconditions & Authorization: valid auth token. The caller must be permitted to access the target customer under team scope; administrators may access any.
- Behavior: looks up the customer by internal identifier. If found and the caller is permitted, returns the customer plus all of that customer's conversations and a count of them.
- Success Output: HTTP 200 with `success: true`, `data` containing `customer` (the customer record described above), `conversations` (array of that customer's conversations), and `conversationCount` (integer), plus `timestamp`.
- Side Effects: none (read-only).
- Error Conditions:
  - `customerId` not a positive integer -> validation error (rejected before handler logic).
  - Customer does not exist -> HTTP 404 with `success: false`, `error: "Customer not found"`, `timestamp`.
  - Caller not permitted under team scope -> HTTP 404 with the identical "Customer not found" body (existence is deliberately hidden from out-of-scope callers; a 403 is never returned here, to prevent enumeration).
  - Server failure -> standardized error response.
- Invariants & Guarantees: a "not found" and a "not authorized" outcome are indistinguishable to the caller by design.

#### Look up a customer by platform identity — GET /api/customers/platform/{platform}/{platformUserId}
- Invocation: authenticated client request.
- Inputs: `platform` path segment (channel name, required string) and `platformUserId` path segment (the user's identifier on that channel, required string).
- Preconditions & Authorization: valid auth token; team-scope access to the matched customer (administrators unrestricted).
- Behavior: resolves a customer by the combination of platform and platform-side user identifier, then returns the customer together with its conversations and conversation count.
- Success Output: HTTP 200 with the same `data` shape as the single-customer lookup (`customer`, `conversations`, `conversationCount`) plus `timestamp`.
- Side Effects: none (read-only).
- Error Conditions:
  - No matching customer -> HTTP 404 "Customer not found".
  - Match exists but caller is out of team scope -> HTTP 404 with the same "Customer not found" body (existence hidden, prevents probing whether a given platform user belongs to another team's book).
  - Server failure -> standardized error response.

#### Get a customer's tags — GET /api/customers/{customerId}/tags
- Invocation: authenticated client request.
- Inputs: `customerId` path segment, required positive integer.
- Preconditions & Authorization: valid auth token. (No additional team-scope gate is applied beyond authentication for this read.)
- Behavior: confirms the customer exists, then returns the active tags currently attached to that customer, ordered most-recently-assigned first.
- Success Output: HTTP 200 standardized success envelope whose data is an array of tag entries. Each entry carries: tag identifier, name, color, description, the tag's owning-team identifier (nullable), the timestamp the tag was attached to this customer, and the identifier of the actor who attached it.
- Side Effects: none (read-only).
- Error Conditions: customer does not exist -> HTTP 404 not-found envelope (resource named "Customer"). Server failure -> standardized error response.
- Invariants & Guarantees: only tags currently marked active are returned; inactive tags are omitted even if still associated.

#### Get selectable tags catalogue — GET /api/customers/tags/available
- Invocation: authenticated client request.
- Inputs (all query parameters, all optional):
  - `page`: page number, default 1.
  - `pageSize`: page size, default 100.
  - `search`: free-text filter applied to tag name and description. Wildcard characters within the search text are matched literally (treated as ordinary characters, not pattern wildcards).
  - `includeGlobal`: `"true"` (default) or `"false"`, controls whether team-agnostic (shared/global) tags are included.
- Preconditions & Authorization: valid auth token. Team scoping is applied to which tags are returned (see Behavior).
- Behavior: returns a paged list of active tags suitable for a tag-picker. For non-administrators with a team, results are scoped to the caller's team and (unless `includeGlobal` is `"false"`) also include global tags with no team. For administrators, all active tags are returned, except that an administrator passing `includeGlobal=false` excludes the team-agnostic tags. Each tag is annotated with two derived counts: how many distinct customers carry it, and how many distinct non-deleted conversations belong to customers carrying it. Results are ordered alphabetically by tag name.
- Success Output: HTTP 200 with `success: true`, `data` (array of tag entries: identifier, name, color, description, owning-team identifier, active flag, creator identifier, creation/update timestamps, plus `customerCount` and `conversationCount` integers), a `pagination` object (`page`, `limit`, `total`, `totalPages`), and a confirmation `message`.
- Side Effects: none (read-only).
- Error Conditions: server failure -> standardized error response.
- Invariants & Guarantees: only active tags are listed; pagination total reflects the full filtered set.

#### Add tags to a customer — POST /api/customers/{customerId}/tags
- Invocation: authenticated client request.
- Inputs: `customerId` path segment (positive integer, required); request body field `tagIds` (array of tag identifiers, required, non-empty).
- Preconditions & Authorization: valid auth token; the authenticated actor identity must be resolvable from the token (used to record who assigned the tags). The customer must exist. Every supplied tag must exist and be active.
- Behavior: validates input and existence, determines which of the requested tags are not already attached, then attaches only the not-yet-attached tags. Already-attached tags are silently skipped (idempotent for duplicates). Each newly attached tag records the assigning actor and an assignment timestamp. The action is recorded as a reversible audit entry per added tag. After persistence, a real-time tag-change event is emitted to relevant clients.
- Success Output: HTTP 200 standardized success envelope with data `added` (count of tags newly attached) and `alreadyExists` (count of requested tags that were already attached), plus a confirmation message.
- Side Effects: persists new customer-to-tag associations (each with assigning-actor and timestamp); writes reversible audit log entries describing each addition; emits a `customer_tags_updated` real-time event with operation `add`.
- Error Conditions:
  - `tagIds` missing, not an array, or empty -> HTTP 422-style validation error naming field `tagIds`.
  - Customer does not exist -> HTTP 404 not-found ("Customer").
  - Any supplied tag is unknown or inactive -> validation error: "Some tag IDs are invalid or inactive".
  - Actor identity absent from token -> HTTP 401 with message "Unauthorized: User ID not found in token".
  - Server failure -> standardized error response.
- Invariants & Guarantees: re-adding an already-attached tag is a no-op for that tag; the broadcast is best-effort and never blocks or fails the request.

#### Remove tags from a customer — DELETE /api/customers/{customerId}/tags
- Invocation: authenticated client request.
- Inputs: `customerId` path segment (positive integer, required); request body field `tagIds` (array, required, non-empty).
- Preconditions & Authorization: valid auth token; the customer must exist.
- Behavior: validates input and existence, then detaches the specified tags from the customer. For each association that actually existed, a reversible audit entry capturing the prior assignment is recorded; detachment of tags that were not attached is harmless. After persistence, a real-time tag-change event is emitted.
- Success Output: HTTP 200 standardized success envelope (no data payload) with a message confirming the number of tags removed (counted by the size of the requested list).
- Side Effects: removes the specified customer-to-tag associations; writes reversible audit log entries for associations that existed; emits a `customer_tags_updated` real-time event with operation `remove`.
- Error Conditions:
  - `tagIds` missing, not an array, or empty -> validation error naming `tagIds`.
  - Customer does not exist -> HTTP 404 not-found ("Customer").
  - Server failure -> standardized error response.
- Invariants & Guarantees: removing a tag not attached to the customer is a no-op; broadcast is best-effort and non-blocking.

#### Replace a customer's tag set — PUT /api/customers/{customerId}/tags
- Invocation: authenticated client request.
- Inputs: `customerId` path segment (positive integer, required); request body field `tagIds` (array, required; may be empty to clear all tags).
- Preconditions & Authorization: valid auth token; if the resulting set is non-empty, the actor identity must be resolvable from the token. The customer must exist. If `tagIds` is non-empty, every supplied tag must exist and be active.
- Behavior: validates input and existence, removes all of the customer's existing tag associations, then attaches exactly the supplied set (recording the assigning actor for each). Passing an empty array clears all tags. An audit log entry summarizing the replacement is recorded (best-effort). After persistence, a real-time tag-change event is emitted.
- Success Output: HTTP 200 standardized success envelope with data `totalTags` (the size of the new tag set) and a confirmation message.
- Side Effects: clears and re-creates the customer's tag associations to match the supplied set; writes a summary audit entry; emits a `customer_tags_updated` real-time event with operation `set`.
- Error Conditions:
  - `tagIds` not an array -> validation error: "Tag IDs must be an array".
  - Customer does not exist -> HTTP 404 not-found ("Customer").
  - For a non-empty set, any unknown or inactive tag -> validation error: "Some tag IDs are invalid or inactive".
  - For a non-empty set, actor identity absent from token -> HTTP 401 "Unauthorized: User ID not found in token".
  - Server failure -> standardized error response.
- Invariants & Guarantees: the operation is a wholesale replacement (final tag set equals the supplied set); broadcast is best-effort and non-blocking.

### Programmatic customer behavior used by other areas
The following behavior is triggered by other areas rather than by standalone HTTP routes in this area. A conforming implementation must reproduce the observable outcomes below.

- Customer record lifecycle (create / find-or-create / update / soft-delete):
  - Create: produces a new customer for a (platform, platform-user-identifier) pair. The pair is unique: attempting to create a customer when one already exists for the same platform identity fails with an "already exists" condition. Optional fields at creation: display label, avatar URL, email, phone, owning-team, and a metadata object (stored serialized).
  - Find-or-create: resolves an existing customer for a platform identity or creates one if absent. When supplemental info is supplied for an existing customer, it conditionally updates the display label, avatar, email, phone, and merges metadata only where the new value differs from the stored value.
  - Update: modifies any subset of display label, avatar, email, phone, owning-team, and metadata; refreshes the update timestamp. Updating a non-existent customer fails with a "not found" condition.
  - Soft delete: never physically removes a customer. Instead it marks the customer as deleted by setting deletion flags inside the metadata object (a deletion marker plus a deletion timestamp). Soft-deleting a non-existent customer fails with "not found".
- Detailed customer view: assembles a customer with its owning-team name, its active tags, conversation statistics (total / active / closed counts, first and last conversation timestamps), and the most recent few messages across that customer's conversations.

- Search and statistics services (team-scoped; non-administrators are restricted to their primary team plus the shared/unassigned pool):
  - Paged customer list with filters: supports filtering by platform, owning-team, a single tag, a free-text search across display label / email / phone / platform-user-identifier, presence/absence of email, presence/absence of phone, a created-date range, and an active/inactive status derived from the soft-delete marker in metadata. Results include per-customer conversation totals, active-conversation counts, last-conversation time, owning-team name, and attached tag names/colors; ordered most-recently-updated first. Returns pagination metadata. (Advanced search behaves identically to the filtered list.)
  - Quick search / autocomplete: requires a query of at least two characters (otherwise returns an empty result set), optionally constrained to a platform; matches across display label, email, phone, and platform-user-identifier, ranks exact-field priority, and caps the number of results.
  - Search suggestions: requires at least two characters; returns up to a small number of distinct display labels matching the query.
  - Statistics overview: totals plus breakdowns by platform and by team, counts of customers with tags / with email / with phone, and count of recently-active customers (those whose customer-originated messages occurred within the last 7 days). Customers flagged deleted in metadata are excluded from statistics.
  - Activity statistics: count of active customers over a configurable look-back window (default 30 days, customer-originated messages only), a per-day active-customer series over the recent window, and a top list of the most message-active customers.
  - Growth statistics: monthly new-customer counts over a configurable number of months (default 12), the total over that span, and the average per active month.

### Data Concepts (neutral)
- Customer record: a contact identity originating from one external messaging channel. Carries an internal numeric identifier; the originating channel/platform; the user's identifier on that channel; an optional human-readable display label; an optional avatar image URL; optional email and phone; a nullable owning-team reference; a free-form metadata container (serialized); and creation and last-update timestamps. The combination of channel and channel-side user identifier is unique across customers.
- Customer metadata: an open key/value container that may hold channel-specific attributes and arbitrary custom fields. Soft-delete state is associated with the customer (a deletion marker and the time of deletion). A request whose metadata object is excessively large is rejected.
- Tag: a labeled marker with an identifier, name, color, optional description, an active/inactive flag, an optional owning-team reference (a tag with no team is treated as global/shared), a creator reference, and timestamps. Tags can be attached to customers.
- Customer-tag association: a link between a customer and a tag, annotated with the identifier of the actor who created the link and the time it was created. Associations are physically removed when detached (not soft-deleted), but each create/remove is captured as a reversible audit entry.
- Conversation (referenced, not owned here): a customer's interaction thread; this area surfaces counts and recency of conversations and the latest messages, but does not manage them.
- Lifecycle states for a customer: effectively "active" (no active deletion marker) versus "inactive/deleted" (deletion marker present). The status filter on listings maps to these two states.

### State & Lifecycle
- Customer existence: created (via create or find-or-create) -> updatable any number of times -> soft-deleted. Soft delete is logical and reversible: the record is marked as deleted (and excluded from default listings and statistics) without being physically removed, and can later be re-activated. There is no hard-delete path in this area.
- Tag attachment on a customer: a tag is either attached or not attached. Add transitions not-attached -> attached (no-op if already attached). Remove transitions attached -> not-attached (no-op if not attached). Replace sets the entire attached set to an exact target set (including the empty set, which clears all). Only active tags may be newly attached.

### Real-time / Event Behavior
- Event emitted: `customer_tags_updated`. Triggered after a successful add, remove, or replace of a customer's tags. Payload carries the affected customer's identifier, an `operation` discriminator (`add` / `remove` / `set`), the list of tag identifiers involved, and the identifier of the actor who made the change. The event is delivered as a broadcast to staff audiences (administrators and agents) and is non-persistent with a short time-to-live. Emission is best-effort: a failure to broadcast is logged and does not affect the HTTP success of the underlying tag operation.
- Audit/activity records: tag add, tag remove, and tag replace each produce activity-log entries (per-tag reversible entries for add/remove; a summary entry for replace), enabling later inspection and, for add/remove, reversal. These are observable through the activity/audit area rather than returned in the operation's own response.

## Teams

### Purpose
This area manages support teams and the people that belong to them. It provides: creating, editing, listing, searching, and deleting teams; viewing per-team and aggregate statistics; managing agent accounts (the "members"); managing each agent's membership in one or many teams, including their in-team role and which team is their primary one; transferring agents between teams; password reset/self-change for member accounts; and generating/reading per-team join QR codes. It enforces a two-tier global role (administrator vs. agent) overlaid with a three-level in-team role hierarchy, and broadcasts real-time events when team membership changes.

### Behavioral Boundary (Under-specified)
The following lies outside the current observable behavior of this area and induces no state change within this boundary; a conforming implementation must reproduce the documented member-management operations but is not required to provide it:
- **Email-based invitation.** New members are added through the direct member-management operations documented below, and a prospective member may self-onboard via the public team-join token page (documented in the System area). Within the current system boundary there is no email-delivered invitation flow (no invitation token issued to an email address, no acceptance endpoint).

### Operations

All HTTP operations are mounted under a base path of `/api/teams` unless otherwise noted. All operations require a valid bearer authentication token unless explicitly marked otherwise; a missing/invalid token yields `401`. All success responses are JSON objects that include a boolean success indicator and (for most) a server timestamp; most error responses return a success=false flag with a human-readable error message.

Authorization vocabulary used below:
- **Administrator**: global role with unrestricted access to every team and member operation.
- **Agent**: ordinary global role; access is scoped to teams the agent belongs to.
- **In-team role**: per-team rank, lowest-to-highest = base member, lead, supervisor. Higher-ranked operations require that rank *or higher in the specific team*. Administrators bypass all in-team checks.
- **Team-access check**: passes for administrators always; for an agent only if the agent is a member of the target team (primary or secondary).
- **Manager-or-admin / admin-only**: in the current two-tier system these both resolve to "administrator required".

---

#### Team module health probe — GET /health
- Invocation: client request, no authentication required.
- Behavior: returns a static liveness payload.
- Success Output: `200` with status string "healthy", a timestamp, a module label, and a version label.

#### Team module info — GET /info
- Invocation: client request, no authentication required.
- Success Output: `200` with a module descriptor object containing a module label, a version label, and a list of endpoint description strings.

#### List teams — GET /
- Invocation: authenticated client request.
- Inputs (query, all optional): `page` (integer, default 1), `limit` (integer, default 20, effectively capped at 100), `includeInactive` (true/false string, default false), `search` (text; matches team name or description, case-insensitive substring).
- Preconditions & Authorization: any authenticated user. Scoping: an agent who has a primary team receives only that single team (the listing/pagination/search parameters are ignored for that user). Administrators (and any user without a primary team) receive the full paginated list.
- Behavior: returns teams ordered newest-first; by default excludes inactive teams unless `includeInactive` is set.
- Success Output: `200` with an array of team summary records (see Data Concepts; each includes member count, active-member count, and assigned-conversation count) plus a pagination block (current page, page size, total count, total pages). For the single-team agent path, an array of exactly one team with statistics; no pagination block.

#### Get a team — GET /:id
- Inputs: path `id` (must be a positive integer; otherwise validation error).
- Preconditions & Authorization: team-access check.
- Behavior: returns the team augmented with statistics (member count, active-member count, assigned-conversation count, a QR-scan count that is currently always zero).
- Success Output: `200` with the team record.
- Error Conditions: nonexistent team -> `404` (team-not-found); non-integer id -> `400`; agent not a member of the team -> `403` (access-denied, includes the user's team and the requested team).

#### Create a team — POST /
- Inputs (body): `name` (required, non-empty after trimming), `description` (optional text), `qrCode` (optional unique string), `isActive` (optional boolean, default true).
- Preconditions & Authorization: administrator required.
- Behavior (observable order): the team is persisted; an activity audit entry is recorded marked as reversible; then two QR artifacts are generated in parallel (a default scan-to-join QR and a LIFF-based deep-link QR). QR generation failures do not fail the request — the team is still created and returned, simply without the failed artifact attached.
- Success Output: `201` with the created team record, optionally augmented with: a generated QR image plus its join URL, and/or a LIFF QR object (identifier, deep-link URL, QR image URL).
- Error Conditions: empty/missing name -> `400` (name-required); supplying a `qrCode` already used by another team -> `409` (duplicate QR code); malformed JSON body -> `400` (invalid JSON).
- Side Effects: new team persisted; reversible create audit entry; up to two QR artifacts created.
- Invariants: team name uniqueness is NOT enforced; QR-code value uniqueness IS enforced when provided.

#### Update a team — PUT /:id
- Inputs: path `id` (positive integer); body any subset of `name`, `description`, `isActive`.
- Preconditions & Authorization: in-team supervisor rank or administrator.
- Behavior: applies the provided fields; records a reversible update audit entry capturing before/after state.
- Success Output: `200` with the updated team record.
- Error Conditions: nonexistent team -> `404` (team-not-found); insufficient in-team rank -> `403` (includes required role, current role, team id); non-integer id -> `400`.
- Side Effects: persisted field changes; reversible update audit entry.

#### Delete a team — DELETE /:id
- Inputs: path `id` (positive integer).
- Preconditions & Authorization: administrator required.
- Behavior: performs a soft delete (marks the team as deleted with a deletion timestamp) atomically together with recording a reversible delete audit entry. Note: the underlying service also contains a full hard-delete-with-cascade routine (removing memberships, the team's QR records and their scan records, and clearing the team reference from conversations and customers), but the HTTP delete path performs the soft-delete-plus-audit form.
- Success Output: `200` with a success message.
- Error Conditions: nonexistent team -> `404` (team-not-found).
- Invariants: delete is reversible via the recorded audit entry.

#### Search teams — GET /search/:query
- Inputs: path `query` (non-empty after trimming).
- Preconditions & Authorization: any authenticated user (no team scoping applied).
- Behavior: returns up to 20 active teams whose name or description contains the query substring (case-insensitive).
- Success Output: `200` with an array of team records.
- Error Conditions: blank query -> `400` (query-required).

#### Single-team statistics — GET /:id/stats
- Inputs: path `id` (positive integer); query `dateFrom`, `dateTo` (optional ISO date strings), `includeMembers` (optional true/false).
- Preconditions & Authorization: team-access check.
- Behavior: returns team statistics including total members, active members, conversations handled, total message count, an average-response-time figure (reported as zero within the current behavioral boundary), a QR-scan figure (reported as zero within the current behavioral boundary), and the reporting period (defaults to the last 30 days when dates omitted).
- Success Output: `200` with the statistics object.
- Error Conditions: nonexistent team -> surfaced as a server error; access denied for non-member agent -> `403`.

#### All-teams statistics — GET /stats/all
- Inputs: query `dateFrom`, `dateTo`, `includeMembers` (all optional).
- Preconditions & Authorization: administrator required.
- Behavior: returns the statistics object for every active team.
- Success Output: `200` with an array of per-team statistics.

#### Transfer agents between teams — POST /transfer
- Inputs (body): `fromTeamId` (integer, required), `toTeamId` (integer, required), `agentIds` (non-empty array of agent identifiers, required), `reason` (optional text).
- Preconditions & Authorization: administrator required.
- Behavior: for each listed agent that is currently a member of the source team, the source-team membership is removed and an equivalent membership is created in the destination team; an agent's primary-team flag is preserved across the move (if the agent was primary in the source team, they become primary in the destination team). Agents not found in the source team are reported as failures and not moved.
- Success Output: `200` with: an overall success flag (true only if there were no failures), the list of transferred agent identifiers, and a list of failed transfers each with a reason.
- Side Effects: membership records moved from source to destination team.

#### List members of a team (basic) — GET /:id/members
- Inputs: path `id` (positive integer).
- Preconditions & Authorization: team-access check.
- Behavior: returns the agents belonging to the team, sorted by display name, each annotated with their in-team role, active/inactive status, join time, and account timestamps.
- Success Output: `200` with the member array.

#### Add a single member to a team — POST /:id/members
- Inputs: path `id` (positive integer); body `agentId` (required, non-empty), optional `role`.
- Preconditions & Authorization: in-team lead rank or administrator.
- Behavior: creates a membership for the agent in the team if one does not already exist; if the agent currently belongs to no team, this new membership is automatically marked as their primary team. The new in-team role is the base member level. If a membership already exists, no duplicate is created.
- Success Output: `201` with the member record.
- Error Conditions: blank agent id -> `400`.

#### Batch add members to a team — POST /:id/members/batch
- Inputs: path `id` (positive integer); body `agentIds` (non-empty array, max 50), optional `roleInTeam` (one of base member / lead / supervisor; default base member).
- Preconditions & Authorization: in-team lead rank or administrator.
- Behavior: adds each listed agent that is not already a member; already-members are reported as skipped. New memberships are NOT primary. Audit logging and real-time broadcasts run asynchronously (non-blocking).
- Success Output: `201` if at least one member was added, otherwise `200`; payload lists added identifiers, skipped identifiers, per-item errors, and an added count.
- Error Conditions: empty array -> `400`; over 50 -> `400`; invalid role value -> `400`; nonexistent team -> `404`.
- Side Effects: memberships created; for each newly added agent a "member added" real-time event is emitted with refreshed member count.

#### Update a member's in-team attributes (team-scoped) — PUT /:id/members/:agentId
- Inputs: path `id` (positive integer), `agentId` (non-empty); body optional `role`, `isActive`.
- Preconditions & Authorization: in-team lead rank or administrator.
- Behavior: updates the agent's global account role and/or active flag (this endpoint updates the account record, not the per-team role).
- Success Output: `200` with the updated member record.
- Error Conditions: blank agent id -> `400`.

#### Remove a member from a team — DELETE /:id/members/:agentId
- Inputs: path `id` (positive integer), `agentId` (non-empty).
- Preconditions & Authorization: in-team lead rank or administrator.
- Behavior: removes the agent's membership in that team. If the removed membership was the agent's primary team, one of the agent's remaining teams is automatically promoted to primary. A reversible audit entry is recorded that captures the prior membership and any promotion, enabling restoration.
- Success Output: `200` with success flag.
- Error Conditions: blank agent id -> `400`; if removal fails internally -> `500`. Removing an agent not in the team is treated as a no-success outcome.

#### Bulk remove members from a team — POST /:id/members/bulk-remove
- Inputs: path `id` (positive integer); body `agentIds` (non-empty array, max 50).
- Preconditions & Authorization: in-team lead rank or administrator.
- Behavior: removes memberships for listed agents that actually belong to the team; agents not in the team are reported as failures. (This bulk path does not perform primary-team promotion.)
- Success Output: `200` with removed identifiers, failures (each with reason), and a removed count.
- Error Conditions: empty array -> `400`; over 50 -> `400`.

---

#### List all member accounts — GET /members
- Invocation: authenticated client request.
- Preconditions & Authorization: administrator only (agents receive `403`).
- Behavior: returns every non-deleted agent account, newest-first, each enriched with full multi-team membership info: the list of teams the agent belongs to (team id, team name, in-team role, primary flag, join time), a team count, and the agent's primary team id/name.
- Success Output: `200` with the member array and a timestamp.

#### Check if an email is in use — GET /members/check-email
- Inputs: query `email` (required).
- Preconditions & Authorization: administrator required.
- Behavior: reports whether any agent account (active or soft-deleted) uses the email; if found, returns whether it is active or deleted plus basic profile info (display name, role, primary team name, last login, created time, deletion time).
- Success Output: `200` with the existence result.
- Error Conditions: missing email -> `400`.

#### Create a member account — POST /members
- Inputs (body): `email` (required), `password` (required), `displayName` (required); optional `role` (administrator/agent, default agent), `teamId`, `isActive` (default true).
- Preconditions & Authorization: administrator required.
- Behavior: rejects if an active account already uses the email. If a *soft-deleted* account uses the email, that account is reactivated and updated (its prior team memberships are cleared) rather than creating a new one. The password is stored only in hashed form. If `teamId` is supplied, the agent is added to that team as their primary team. An account-create audit entry is recorded.
- Success Output: `201` with the created member record and a confirmation message.
- Error Conditions: missing required fields -> `400`; email already used by an active account -> `409` (already exists).
- Invariants: email is unique among active accounts.

#### Set a member's active status — PUT /members/:memberId/status
- Inputs: path `memberId`; body `isActive` (required boolean), optional `reason`.
- Preconditions & Authorization: administrator required.
- Behavior: activates/deactivates the account; records an account-update audit entry capturing the before/after status and reason.
- Success Output: `200` with the updated member and a message.
- Error Conditions: `isActive` omitted -> `400`; attempting to change one's own status -> `403`; member not found -> `404`.

#### Set a member's global role — PUT /members/:memberId/role
- Inputs: path `memberId`; body `role` (required), optional `reason`.
- Preconditions & Authorization: administrator required.
- Behavior: updates the account's global role; records an audit entry with the before/after role.
- Success Output: `200` with the updated member.
- Error Conditions: `role` omitted -> `400`; changing one's own role -> `403`; member not found -> `404`.

#### Update a member account — PUT /members/:memberId
- Inputs: path `memberId`; body any subset of `email`, `displayName`, `role`, `isActive` (team membership is not changed here).
- Preconditions & Authorization: administrator required.
- Behavior: applies provided fields; records an audit entry diffing all supplied fields.
- Success Output: `200` with the updated member.
- Error Conditions: member not found -> `404`.

#### Permanently delete a member account — DELETE /members/:memberId
- Inputs: path `memberId`.
- Preconditions & Authorization: administrator required.
- Behavior: irreversibly deletes the account after detaching it from all related records — notifications are deleted; pending scheduled outbound messages by the agent are deleted; references in messages/attachments/recall logs/tag assignments/conversation transfers/audit history/reports and report-related records/channel-integration configuration/customer feedback are either nulled or reassigned to a placeholder identity so history is preserved; team memberships and reminders are removed automatically. Audit logging of the deletion is attempted but failure to log does not fail the deletion.
- Success Output: `200` with confirmation and the deleted member id.
- Error Conditions: deleting one's own account -> `403`; member not found -> `404`.
- Invariants: this delete is permanent (no restore path).

#### Bulk permanently delete members — POST /members/bulk-delete
- Inputs (body): `memberIds` (non-empty array, max 50), optional `reason`.
- Preconditions & Authorization: administrator required.
- Behavior: deletes each existing member (same permanent cleanup as the single delete), reporting per-member success/failure; nonexistent ids are failures.
- Success Output: `200` with deleted identifiers, failures, and a deleted count.
- Error Conditions: empty/non-array -> `400`; over 50 -> `400`; list includes one's own id -> `403`.

#### Bulk update members (uniform changes) — POST /members/bulk-update
- Inputs (body): `memberIds` (non-empty array, max 50), `updates` (must include at least one of `role` (administrator/agent) or `isActive`), optional `reason`.
- Preconditions & Authorization: administrator required.
- Behavior: applies the same change set to all listed existing (non-deleted) members, except the caller's own account, which is skipped; nonexistent ids are failures.
- Success Output: `200` with updated identifiers, failures, skipped (with reasons), and an updated count.
- Error Conditions: empty/non-array list -> `400`; over 50 -> `400`; no update field provided -> `400`; invalid role value -> `400`.

#### Batch edit members (per-member changes) — POST /members/batch-edit
- Inputs (body): `members` (non-empty array, max 50). Each entry has `memberId` (required) plus at least one of: a `profile` change (any of display name / email / role) or a `teamChanges` object (`add` and/or `remove` arrays of team ids). Optional top-level `reason`.
- Preconditions & Authorization: administrator required.
- Behavior: applies each member's individual profile and team-membership changes; the caller's own account is skipped; nonexistent members fail individually. On any successful edit, an undo token is issued and the prior state is stored transiently (the token is valid for a short window — advertised ~10 seconds, retained server-side ~60 seconds).
- Success Output: `200` with per-member results (success flag, error, whether profile updated, teams added, teams removed), success/failure counts, skipped list, and — when any edit succeeded — an undo token and its expiry time.
- Error Conditions: empty/non-array list -> `400`; over 50 -> `400`; an entry missing a member id -> `400`; an entry with no actual changes -> `400`.

#### Undo a batch edit — POST /members/batch-edit/undo
- Inputs (body): `undoToken` (required).
- Preconditions & Authorization: administrator required, and only the same user who performed the original edit may undo it.
- Behavior: restores the members to their captured prior state, then invalidates the token.
- Success Output: `200` with the restored count and per-member results.
- Error Conditions: missing token -> `400`; expired/unknown token -> `400`; token belongs to a different user -> `403`.

---

#### Reset a member's password (administrative) — POST /members/:memberId/reset
- Inputs: path `memberId`; body `newPassword` (required), optional password policy (one of: changeable / unchangeable / must-change).
- Preconditions & Authorization: administrator required.
- Behavior: overwrites the member's password (stored hashed); optionally sets a password policy.
- Success Output: `200` with confirmation and the resulting policy.
- Error Conditions: missing new password -> `400`; resetting one's own account here -> `403` (directs to the self-change endpoint); member not found -> `404`.

#### Change one's own password — POST /change-password (mounted at /api/auth/change-password)
- Inputs: body `currentPassword` (required), `newPassword` (required).
- Preconditions & Authorization: any authenticated user; operates on the caller's own account only.
- Behavior: verifies the current password against the stored hash; on success stores the new password hashed and logs a successful-change audit entry; on mismatch logs a failed-attempt security audit entry.
- Success Output: `200` with confirmation.
- Error Conditions: missing either field -> `400`; account not found -> `404`; current password incorrect -> `401`.

---

#### Get an agent's teams — GET /agent-teams/:agentId
- Inputs: path `agentId`.
- Preconditions & Authorization: administrator may view anyone's teams; an agent may view only their own.
- Behavior: returns every membership the agent holds (team id, in-team role, primary flag, join/created times, and joined team name/description/active state).
- Success Output: `200` with the membership array.
- Error Conditions: agent viewing another agent's teams -> `403`.

#### Get a team's members (with multi-team detail) — GET /agent-teams/team/:teamId/members
- Inputs: path `teamId` (positive integer).
- Preconditions & Authorization: any authenticated user.
- Behavior: returns the team's members, each including their full list of team memberships and their primary team id.
- Success Output: `200` with the member array.

#### Add an agent to a team — POST /agent-teams/:agentId/join
- Inputs: path `agentId`; body `teamId` (required), optional `roleInTeam` (default base member), optional `isPrimary` (default false).
- Preconditions & Authorization: administrator required.
- Behavior: rejects if the agent is already in that team. Otherwise creates the membership; if marked primary, any existing primary flag for that agent is cleared first so exactly one team remains primary. Records an audit entry and broadcasts a "member added" real-time event with refreshed member count.
- Success Output: `201` with the new membership and a message.
- Error Conditions: missing `teamId` -> `400`; agent already in team -> `409`.

#### Add an agent to multiple teams — POST /agent-teams/:agentId/join-multiple
- Inputs: path `agentId`; body `teamIds` (non-empty array, required), optional `roleInTeam` (default base member).
- Preconditions & Authorization: administrator required.
- Behavior: adds the agent to each listed team they are not already in (those are skipped); new memberships are not primary. Audit entry recorded; for each newly added team a "member added" real-time event is broadcast asynchronously with that team's refreshed member count.
- Success Output: `200` with the result object (added team ids, skipped, errors) and a summary message.
- Error Conditions: missing/empty/non-array `teamIds` -> `400`.

#### Remove an agent from a team — DELETE /agent-teams/:agentId/leave/:teamId
- Inputs: path `agentId`, `teamId` (positive integer).
- Preconditions & Authorization: in-team lead rank in the target team, or administrator.
- Behavior: removes the membership; if it was the agent's primary team, another remaining team is promoted to primary. Records an audit entry; sends a high-priority personal notification to the removed agent (which prompts their UI to refresh and to force-close any open view of the team's conversations); and broadcasts a team-wide "member removed" event with refreshed member count. The response also reports how many conversations assigned to that team are now affected for the removed agent.
- Success Output: `200` with confirmation, the team name, and the affected-conversation count.

#### Update an agent's role within a team — PUT /agent-teams/:agentId/role/:teamId
- Inputs: path `agentId`, `teamId` (positive integer); body optional `roleInTeam`, optional `isPrimary`.
- Preconditions & Authorization: in-team lead rank in the target team, or administrator.
- Behavior: updates the in-team role and/or primary flag for that membership; if set primary, other primary flags for the agent are cleared first. Records an audit entry.
- Success Output: `200` with the updated membership and a message.
- Error Conditions: membership not found -> surfaced as a server error.

#### Set an agent's primary team — PUT /agent-teams/:agentId/primary/:teamId
- Inputs: path `agentId`, `teamId` (positive integer).
- Preconditions & Authorization: in-team lead rank in the target team, or administrator.
- Behavior: requires the agent to already be a member of the team; clears all of the agent's primary flags then marks this team primary. Records an audit entry.
- Success Output: `200` with confirmation.
- Error Conditions: agent not a member of the team -> surfaced as a server error.

---

#### Generate a team QR code — POST /:id/qr-code
- Inputs: path `id` (positive integer); body optional `campaignName`, `description`, `expiresAt` (date), `maxUses` (integer). Missing/invalid body is tolerated.
- Preconditions & Authorization: in-team supervisor rank or administrator.
- Behavior: creates a new scan-to-join QR code for the team.
- Success Output: `201` with the generated QR record.

#### List a team's QR codes — GET /:id/qr-codes
- Inputs: path `id` (positive integer).
- Preconditions & Authorization: team-access check.
- Success Output: `200` with the team's QR-code records.

#### Get latest team QR code — GET /:id/qr-code/latest
- Inputs: path `id` (positive integer).
- Preconditions & Authorization: team-access check.
- Behavior: returns the team's current QR image plus a derived join URL; if not cached on the team it is sourced from the QR records and asynchronously cached back.
- Success Output: `200` with the QR image, join URL, and a cache-origin flag; if none exists -> `404`.

#### Fast team QR lookup — GET /:id/qr-code/fast
- Inputs: path `id` (positive integer).
- Preconditions & Authorization: team-access check.
- Behavior: returns the cached team QR if present, else the latest active QR record (asynchronously caching it), with a source/performance indicator.
- Success Output: `200` with the QR data; none found -> `404`.

#### Deactivate a QR code — PUT /:id/qr-codes/:qrCodeId/deactivate
- Inputs: path `id` (positive integer), `qrCodeId` (non-empty).
- Preconditions & Authorization: in-team supervisor rank or administrator.
- Behavior: marks the specified QR code inactive.
- Success Output: `200` with confirmation.
- Error Conditions: blank QR id -> `400`.

#### Get team LIFF QR code — GET /:id/qr-code/liff
- Inputs: path `id` (positive integer).
- Preconditions & Authorization: team-access check.
- Behavior: returns the team's LIFF deep-link QR (identifier, deep-link URL, a freshly signed QR image URL, scan count, active flag, timestamps).
- Success Output: `200` with the LIFF QR data; none exists -> `404`.

#### Generate/regenerate team LIFF QR code — POST /:id/qr-code/liff
- Inputs: path `id` (positive integer).
- Preconditions & Authorization: administrator required.
- Behavior: (re)generates the team's LIFF deep-link QR.
- Success Output: `200` with the new LIFF QR data; team not found -> `404`; generation failure -> `500`.

#### LIFF QR statistics — GET /:id/qr-code/liff/stats
- Inputs: path `id` (positive integer).
- Preconditions & Authorization: team-access check.
- Behavior: returns the LIFF QR's scan count, the count of customer-to-team assignments produced through it, creation time, last-scan time, and active flag.
- Success Output: `200` with the stats; no LIFF QR -> `404`.

#### Test QR generation — POST /:id/qr-code-test
- Invocation: client request; this endpoint does NOT require authentication.
- Behavior: generates a throwaway test QR for diagnostics.
- Success Output: `200` with the test QR; on server failure -> `500`.

### Data Concepts (neutral)

- **Team**: a support group identified by a numeric id. Carries a human-readable name, an optional description, an active/inactive flag, an optional join QR value, creation/update timestamps, and a soft-deletion timestamp. Derived/reported attributes include member count, active-member count, and assigned-conversation count. Name is not required to be unique; the join QR value, when set, must be unique across teams. Team deletion via the HTTP layer is a soft delete (and reversible via audit); the service additionally supports a hard delete that cascades to memberships, the team's QR artifacts and their scan logs, and clears the team reference from conversations and customers.
- **Member / Agent account**: an individual identified by a string id, carrying an email (unique among active accounts), a display name, a hashed password, a global role (administrator or agent), an active flag, optional password policy, last-login/last-active markers, account timestamps, and a soft-deletion timestamp. Soft-deleted accounts can be reactivated by re-creating with the same email; explicit deletion through the member endpoints is permanent. Membership of a team is not stored on the account itself but in a separate membership association.
- **Team membership (association)**: links one agent to one team. Carries the agent's in-team role (base member / lead / supervisor), a primary-team flag, and join/created timestamps. Invariants: an agent may belong to many teams; at most one of an agent's memberships is marked primary at any time. When the primary membership is removed, another remaining membership is promoted to primary. Adding the first-ever membership for an agent makes it primary automatically.
- **QR artifacts**: each team may have scan-to-join QR codes (which can be active/inactive, have optional campaign name/description/expiry/usage cap, and accumulate scan records) and a single LIFF deep-link QR (with a deep-link URL, a stored QR image URL, scan count, active flag, and timestamps). Image URLs may be re-signed on read.
- **Statistics**: per-team aggregates (members, active members, conversations handled, message totals) over an optional date range; some fields (average response time, QR scan totals in the team stats object) are present but currently always zero.
- **Undo token**: a short-lived transient credential tied to a specific batch-edit operation and its initiating user, holding the captured prior member state for restoration within a brief window.

### State & Lifecycle

- **Team active state**: active <-> inactive (toggled via update). Default listings show only active teams. Soft-deleted is a terminal state for the listing/access surface but is reversible through the recorded reversible delete audit entry. The agent-account-validation layer also recognizes an "archived" state value, but the live team model exposes active/inactive.
- **Member account state**: active <-> inactive (status toggle). Soft-deleted accounts can be revived by recreating with the same email; explicit hard deletion is terminal and irreversible.
- **In-team role hierarchy** (per team, ascending privilege): base member < lead < supervisor. Operations require a minimum rank in the specific team: viewing requires base membership; adding/updating/removing members requires lead; team edits, deletion, QR management, and transfers require supervisor. Administrators bypass all rank requirements. Self-protection rules: a user cannot change their own status/role, cannot delete their own account, and cannot include their own account in bulk delete/update or batch edit (own account is skipped or rejected).
- **Primary team transition**: setting any membership primary atomically demotes the agent's other primary; removing the primary membership promotes another remaining membership.

### Real-time / Event Behavior

- **Member added event**: emitted when an agent is added to a team (single add via the agent-teams join endpoint, batch add to a team, or join-multiple). Payload conveys an "added" event type, the team id and name, the agent id and name, the team's refreshed member count, and who made the change. Audience: management/administration views that display member counts. Batch/multi additions emit one event per newly added membership and run asynchronously (non-blocking to the response).
- **Member removed event**: emitted when an agent is removed from a team via the agent-teams leave endpoint. Payload conveys a "removed" event type with the same fields as above and refreshed member count. Audience: management/administration views.
- **Agent-removed personal notification**: in addition to the team-wide removed event, the removed agent receives a high-priority personal notification (persisted and pushed in real time) carrying the team id/name, who removed them, and the list of conversation ids assigned to that team that they can no longer access. This prompts the agent's client to show an alert, refresh their conversation list, and force-close any open view of an affected conversation.
- A team-information-update broadcast capability exists (name/description/active/member-count changes with the initiating user) for propagating team edits to management views.

## Agents / Operators

### Purpose
This area manages support-staff accounts ("operators") who handle customer conversations. It provides administrative listing/search of operators, retrieval and editing of an operator's profile, role and team membership management (including bulk transfer between teams), a per-operator skill inventory, and a presence/availability system (online, busy, away, offline, on-break, in-meeting) with status history and aggregate statistics. Status and skill data drive downstream "who can be assigned a conversation" eligibility decisions. All operations require authentication; most are gated by an operator's system role (administrator, team leader, or ordinary operator) and by ownership/team scoping.

### Operations

All routes below are served under a common API base prefix `/api`. Every route first passes a module-wide error handler and an authentication gate (see Preconditions). Unless stated otherwise, the standard success envelope is a JSON object containing a boolean success flag, a `data` payload, and a human-readable `message` string; standard error envelopes contain `success: false`, an `error` message string, a machine error `code`, a `timestamp`, and a request correlation id.

#### List operators (paged) — GET /api/agents
- Invocation: authenticated client request.
- Inputs (query): `page` (integer, optional, default 1, must be a positive integer no greater than 1000), `limit` (integer, optional, default 20, must be a positive integer no greater than 100), `includeInactive` (string flag; only the literal value `true` enables inclusion of deactivated operators; any other/absent value excludes them), `search` (string, optional; matches against display name or email substring), `teamId` (integer, optional; restricts to members of that team), `role` (string, optional; restricts by system role), `status` (string, optional; one of online/busy/away/offline/break/meeting — values outside this set are ignored).
- Preconditions & Authorization: caller must be an administrator or a team leader. Ordinary operators are forbidden.
- Behavior: returns a page of operator profiles ordered with the most recently created first, plus pagination metadata. Inactive operators are excluded by default. The `status` query parameter is accepted/validated but does not filter results in this endpoint.
- Success Output: 200; `data` is an array of operator profile objects (see Data Concepts; password material is always blank), and a separate `pagination` object carrying current page, page size, total matching count, and total page count.
- Error Conditions: invalid `page`/`limit` (non-integer, below 1, page over 1000, or limit over 100) -> 400 validation error; non-privileged role -> 403; missing/invalid auth -> 401.

#### Bulk update operators — PUT /api/agents/batch
- Invocation: authenticated client request.
- Inputs (body): `agentIds` (array of operator identifier strings, required, non-empty, at most 50 entries, each string 10–50 characters), `updates` (object carrying any subset of editable profile fields — display name, email, role, team membership, active flag).
- Preconditions & Authorization: administrator only.
- Behavior: applies the same update set to each listed operator, processing them independently; failures on individual operators are skipped and do not abort the batch.
- Success Output: 200; `data` is the array of successfully updated operator records; message reports the count updated.
- Error Conditions: `agentIds` not an array / empty / over 50 / containing an out-of-range identifier -> 400 validation error; non-administrator -> 403; unauthenticated -> 401.
- Invariants: best-effort — individually failing operators are silently omitted from the result rather than causing overall failure.

#### Bulk transfer operators to a team — PUT /api/agents/batch/transfer
- Invocation: authenticated client request.
- Inputs (body): `agentIds` (same constraints as bulk update: array, non-empty, ≤50, each 10–50 chars), `toTeamId` (target team identifier, required), `fromTeamId` (optional, informational), `reason` (optional free text, not persisted observably).
- Preconditions & Authorization: administrator only.
- Behavior: each listed operator's existing team membership(s) are replaced with a single primary membership in the target team. Operators are processed independently; per-operator failures are collected rather than aborting.
- Success Output: 200; `data` is an object containing an overall success boolean and a list of per-operator error entries (each identifying the operator and the error text). Top-level `success` mirrors whether the error list is empty; message states either full success or the number of errors.
- Error Conditions: validation failures on `agentIds` -> 400; target team does not exist -> surfaced as a server error (500) with a "target team not found" message; non-administrator -> 403; unauthenticated -> 401.
- Invariants: this is the only sanctioned path for changing an operator's team; a transfer makes the new team the operator's primary team and removes prior memberships.

#### Search operators — POST /api/agents/search
- Invocation: authenticated client request.
- Inputs (body): `keyword` (string; display-name or email substring), `teamIds` (array of team identifiers), `roles` (array of role strings), `isActive` (boolean), `lastActiveAfter` / `lastActiveBefore` (timestamp bounds on last-active time), plus optional `limit` (default 50) and `offset` (default 0). Also accepts `skills`, `status`, `joinedAfter`, `joinedBefore` fields by shape, though filtering behavior beyond the listed criteria is not guaranteed.
- Preconditions & Authorization: administrator or team leader.
- Behavior: returns matching operator profiles ordered by most recently active first, limited/offset per inputs.
- Success Output: 200; `data` is an array of operator profile objects (password material blank).
- Error Conditions: non-privileged role -> 403; unauthenticated -> 401.

#### Get aggregate status statistics — GET /api/agents/status/statistics
- Invocation: authenticated client request.
- Inputs: none.
- Preconditions & Authorization: administrator or team leader.
- Behavior: counts how many operators are currently in each presence state across the operator population. Operators with no recorded presence count as offline.
- Success Output: 200; `data` is an object with a numeric count for each of the six presence states (online, busy, away, offline, break, meeting).
- Error Conditions: non-privileged role -> 403; unauthenticated -> 401.
- Notes: reading each operator's status during aggregation can passively trigger auto-expiry (see State & Lifecycle).

#### Get operator skills — GET /api/agents/:agentId/skills
- Invocation: authenticated client request.
- Inputs (path): `agentId` (operator identifier; must be 10–50 characters).
- Preconditions & Authorization: authenticated; access scoping applies — administrators may target any operator; ordinary operators may target only their own identifier; team leaders are permitted to target any operator (no team-membership check is enforced).
- Behavior: returns the operator's full skill inventory.
- Success Output: 200; `data` is an array of skill objects (see Data Concepts). Empty list if none.
- Error Conditions: identifier length out of range -> 400; ordinary operator targeting another operator -> 403; unauthenticated -> 401.

#### Add a skill to an operator — POST /api/agents/:agentId/skills
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars). Inputs (body): `name` (string, required, 2–100 chars), `category` (required; one of communication, technical, product, language, platform, soft_skill), `level` (required; one of beginner, intermediate, advanced, expert), `description` (optional, max 500 chars), `certified` (optional boolean).
- Preconditions & Authorization: same access scoping as "Get operator skills".
- Behavior: appends a new uniquely-identified skill to the operator. Certification timestamp is set when created as certified, otherwise null.
- Success Output: 201; `data` is the created skill object.
- Error Conditions: missing required field / invalid category / invalid level / name length out of range / description too long / non-boolean `certified` -> 400; a skill with the same name already exists for that operator -> surfaced as a server error (500) with a duplicate-skill message; ordinary operator targeting another -> 403; identifier length invalid -> 400; unauthenticated -> 401.
- Invariants: skill names are unique per operator.

#### Update a skill — PUT /api/agents/:agentId/skills/:skillId
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars), `skillId` (skill identifier). Inputs (body): any subset of `level`, `description`, `certified`.
- Preconditions & Authorization: access scoping as above.
- Behavior: merges provided fields into the existing skill. If `certified` is supplied, the certification timestamp is set to now when turning certified on, or cleared when turning it off; if `certified` is absent the prior certification timestamp is preserved.
- Success Output: 200; `data` is the updated skill object.
- Error Conditions: skill not found for that operator -> surfaced as a server error (500) with a not-found message; ordinary operator targeting another -> 403; identifier length invalid -> 400; unauthenticated -> 401.

#### Delete a skill — DELETE /api/agents/:agentId/skills/:skillId
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars), `skillId`.
- Preconditions & Authorization: access scoping as above.
- Behavior: removes the named skill from the operator's inventory.
- Success Output: 200 with confirmation message.
- Error Conditions: skill not present -> 404 with a "not found" error; ordinary operator targeting another -> 403; identifier length invalid -> 400; unauthenticated -> 401.

#### Get skill statistics for an operator — GET /api/agents/:agentId/skills/statistics
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars).
- Preconditions & Authorization: access scoping as above.
- Behavior: summarizes the operator's skills.
- Success Output: 200; `data` contains total skill count, a breakdown count by category, a breakdown count by level, the number of certified skills, and a certification rate as a percentage rounded to two decimals (zero when no skills).
- Error Conditions: identifier length invalid -> 400; ordinary operator targeting another -> 403; unauthenticated -> 401.

#### Get an operator's current presence — GET /api/agents/:agentId/status
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars).
- Preconditions & Authorization: access scoping as above.
- Behavior: returns the operator's current presence record. If none has ever been set, a default offline record is returned. If the stored record carries an expiry timestamp that is now in the past, the operator is automatically switched to offline (with an auto-expired note) before the response is returned.
- Success Output: 200; `data` is a presence object: current state, the timestamp the state took effect, optional availability-until timestamp, optional note.
- Error Conditions: identifier length invalid -> 400; ordinary operator targeting another -> 403; unauthenticated -> 401.
- Side Effects: may transition the operator to offline and append a history entry as a read side effect (auto-expiry).

#### Update an operator's presence — PUT /api/agents/:agentId/status
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars). Inputs (body): `status` (required; one of online, busy, away, offline, break, meeting), `availableUntil` (optional; must be a valid date strictly in the future), `note` (optional, max 200 chars).
- Preconditions & Authorization: access scoping as above (operators may set their own presence; administrators any; team leaders any).
- Behavior: replaces the operator's presence with the supplied state, stamping the effective time as now, recording the availability-until and note (null if omitted), and appending the change to presence history.
- Success Output: 200; `data` is the new presence object.
- Error Conditions: missing status / invalid status value / non-future or unparseable availability-until / note too long -> 400; ordinary operator targeting another -> 403; identifier length invalid -> 400; unauthenticated -> 401.
- Side Effects: persists the new presence and prepends a timestamped entry to the operator's presence history (history is capped at the most recent 100 entries).

#### Get presence history — GET /api/agents/:agentId/status/history
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars). Inputs (query): `limit` (integer, optional, default 20; caller-supplied with no enforced ceiling here).
- Preconditions & Authorization: access scoping as above.
- Behavior: returns the most recent presence-change entries, newest first, truncated to the requested count.
- Success Output: 200; `data` is an array of history entries, each being a presence record plus the timestamp it was recorded. Empty array if no history.
- Error Conditions: identifier length invalid -> 400; ordinary operator targeting another -> 403; unauthenticated -> 401.

#### Get a single operator's full details — GET /api/agents/:agentId
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars).
- Preconditions & Authorization: access scoping as above.
- Behavior: returns the operator's profile combined with the full skill inventory and the current presence record.
- Success Output: 200; `data` is the operator profile plus a `skills` array and a `currentStatus` presence object. Password material is blank.
- Error Conditions: operator not found -> 404 with a "not found" error; identifier length invalid -> 400; ordinary operator targeting another -> 403; unauthenticated -> 401.

#### Update an operator profile — PUT /api/agents/:agentId
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars). Inputs (body, all optional but at least one required): `displayName` (2–50 chars), `email` (valid email format), `role` (one of administrator/operator), `teamId` (positive integer team identifier), `isActive` (boolean), `passwordPolicy`.
- Preconditions & Authorization: access scoping as above. Additional guards: (1) a caller cannot assign a role higher in privilege than their own; (2) a non-administrator caller cannot include a team change on their own record (self team transfer is blocked — team changes must go through the administrator-only bulk transfer).
- Behavior: validates the body, then applies the changes. If the email is changed it must not collide with another operator. If a team identifier is supplied, the operator's prior team membership(s) are replaced with a single new primary membership in that team. Other fields are updated in place; the profile's updated timestamp is refreshed.
- Success Output: 200; `data` is the updated operator record.
- Error Conditions: empty body / invalid email format / display name length out of range / invalid role value / invalid team-id format / non-boolean active flag -> 400; operator not found -> 404; attempting to assign a role above the caller's own -> 403 with an explicit message; a non-administrator attempting a team change on their own record -> 403 with an explicit message; target team does not exist -> internal error (500) with a "team not found" message; new email already used by another operator -> internal error (500) with a duplicate-email message; ordinary operator targeting another -> 403; unauthenticated -> 401.
- Invariants: email is unique across operators; role-elevation and self-team-transfer privilege escalations are prevented.

#### Delete (remove) an operator — DELETE /api/agents/:agentId
- Invocation: authenticated client request.
- Inputs (path): `agentId` (10–50 chars).
- Preconditions & Authorization: administrator only.
- Behavior: removes the operator. Before removal, references to the operator across related records are cleaned up: notifications addressed to them are removed; queued/delayed outbound items they authored are removed; message authorship is detached (nulled); various audit/ownership fields elsewhere (recall logs, uploaded files, tag creators/assigners, conversation transfer actors, activity actors, report creators/downloaders/templates, channel-configurer, customer-feedback handler) are either nulled or reassigned to a placeholder sentinel identity so the operator can be deleted without violating referential constraints. Team memberships and personal reminders are removed automatically.
- Success Output: 200 with confirmation message.
- Error Conditions: operator not found -> 404 with a "not found" error; non-administrator -> 403; identifier length invalid -> 400; unauthenticated -> 401.
- Invariants: removal cascades reference cleanup; historical records authored by the operator survive with authorship anonymized to a placeholder rather than being destroyed.

> Note: A facility to create a new operator account exists in the underlying logic (validating required email + display name, email-format, display-name length 2–50, role restricted to administrator/operator, optional team membership), but no create endpoint is exposed through this module's external router. Operator creation is therefore not part of this area's observable HTTP surface.

### Data Concepts (neutral)

- **Operator profile**: a unique operator identifier (opaque string, treated as 10–50 characters for routing), a unique email address, a human-readable display name, a system role (administrator or operator; a "team" role value is also recognized by authorization for team-leader behavior), an active/deactivated flag, a password-policy indicator, a last-active timestamp, a last-login timestamp, creation/update timestamps, and a soft-deletion timestamp. Password material is never returned to callers (always blank in responses). Each profile may carry a primary team identifier and the corresponding team's display name, derived from membership.
- **Team membership**: links an operator to a team with an in-team role and a primary-membership flag. An operator's "primary team" is the membership marked primary. Membership is created on profile creation (when a team is given), replaced on profile team-change, and replaced on bulk transfer. The first/only membership is treated as primary.
- **Skill**: a uniquely-identified entry per operator, carrying a name (unique within the operator), a category (communication, technical, product, language, platform, soft_skill), a proficiency level (beginner, intermediate, advanced, expert), a certified flag, a certification timestamp (set when certified, cleared otherwise), and an optional description. Skills are stored as the operator's private inventory and are not shared.
- **Presence record**: the operator's current state (one of online, busy, away, offline, break, meeting), the timestamp the state took effect, an optional availability-until timestamp after which the state auto-expires, and an optional note.
- **Presence history entry**: a presence record plus the timestamp it was recorded. History is kept newest-first and retained to the most recent 100 entries per operator.
- **Soft-delete semantics**: operator profiles carry a soft-deletion timestamp concept; listing/search exclude deactivated operators by default (an explicit include flag overrides for listing). Other domains anonymize references to a removed operator rather than deleting their historical contributions.

### State & Lifecycle

- **Profile activation state**: active or deactivated (via the active flag). Deactivated operators are hidden from default listings/searches but can be surfaced explicitly in listing.
- **Role hierarchy**: operator (lower) and administrator (higher); a "team-leader" capability is granted to a distinct team-role value at authorization time. A caller can never set another operator's role above the caller's own level.
- **Team membership lifecycle**: created on operator creation with a team; replaced wholesale on profile team-change or bulk transfer (prior memberships removed, one new primary membership added). Self-initiated team change is blocked for non-administrators.
- **Presence state machine**: any of the six states can transition to any other via an update. A special automatic transition exists: when a presence record's availability-until time has passed, the next read of that operator's presence forces a transition to offline (annotated as auto-expired) and records it in history. There is no terminal presence state.

### Real-time / Event Behavior

The module defines a conceptual catalog of operator domain events (operator created, updated, deleted, presence-changed, skill-added, skill-updated, assigned to work, and performance-updated), each carrying the operator identifier, an event-specific payload, a timestamp, and optionally the acting user. These describe intended real-time notifications for operator lifecycle, presence, skill, and assignment changes. Within this area's observable HTTP handlers, the primary externally-observable real-time-relevant side effects are: presence updates and auto-expiry transitions persisting new presence plus a history entry, and the eligibility helpers (which operators are online/available) that downstream assignment logic consults to decide who may receive a conversation. Availability eligibility treats only the online state as available; busy, away, break, meeting, and offline are treated as not available for assignment.

## Collaboration

### Purpose
This area provides real-time multi-agent collaboration features layered on top of conversations. It lets authenticated staff users see who else is currently viewing a given conversation, announce that they have joined or left a conversation view, broadcast "is typing" indicators, publish their own online/availability status, and read aggregate collaboration statistics. Collaboration state is live-session presence: short-lived, tied to active connections, not retained as long-term history, and delivered to other connected clients as live events.

### Operations

#### Get conversation collaboration state — GET /api/collaboration/conversations/{conversationId}/state
- Invocation: Authenticated client request.
- Inputs:
  - Path parameter `conversationId`: required, must be a positive integer; rejected by parameter validation otherwise.
  - Query parameter `protocol`: optional string; the only effective transport is the real-time WebSocket transport. Any other value either selects an equivalent default or is treated as unsupported (see Error Conditions).
- Preconditions & Authorization: Requires a valid authentication token (the operation reads the caller's identity from the auth context). Any authenticated staff role is accepted.
- Behavior: Returns a snapshot of the live collaboration state for the specified conversation: the current set of viewers, the current set of users showing a typing indicator, a live connection count, the active transport label, and a last-activity timestamp. If the underlying live room cannot supply full metrics, a reduced snapshot is returned containing at least the viewer list (with empty typing list and a connection count equal to the viewer count). If the live room is entirely unreachable, an empty/degraded snapshot is returned rather than an error.
- Success Output: HTTP 200 with a standard success envelope wrapping a state object containing: the conversation identifier; an array of viewer entries (see Data Concepts); an array of typing entries (each with the user identifier, username, display label, conversation identifier, a start timestamp, and an expiry timestamp); a total live-connection count; the transport label; a last-activity timestamp; and optional metadata (e.g. a count of recent message history and an "is active" flag).
- Side Effects: None observable; read-only.
- Error Conditions: Invalid/non-integer conversation identifier -> validation error (HTTP 400). Missing/invalid auth -> rejected by the auth layer (HTTP 401). Explicitly requesting an unsupported transport may yield a "protocol not supported" error (machine code indicating protocol not supported, HTTP 400) or an "adapter not initialized" error (machine code indicating the adapter is not initialized, HTTP 500). Unexpected server failures surface through the standard error handler.
- Invariants & Guarantees: Read-only; reflects current ephemeral state at call time. Degrades gracefully to a partial snapshot rather than failing when room metrics are unavailable.

#### List conversation viewers — GET /api/collaboration/conversations/{conversationId}/viewers
- Invocation: Authenticated client request.
- Inputs: Path parameter `conversationId` (required, positive integer); optional `protocol` query parameter (same semantics as above).
- Preconditions & Authorization: Valid auth token; any authenticated staff role.
- Behavior: Returns the list of users currently viewing the specified conversation. Viewer entries are normalized: each carries a numeric user identifier, a username (falling back to a generated "User <id>" label when absent), a display label (falling back to the username, then the generated label), a role (defaulting to the agent role when absent), a join timestamp, the transport label, a typing flag, and a last-activity timestamp. Entries whose user identifier cannot be interpreted as a finite number are omitted. If the live room is unreachable, an empty list is returned.
- Success Output: HTTP 200 with a success envelope wrapping an object whose single field is the array of viewer entries.
- Side Effects: None; read-only.
- Error Conditions: Invalid conversation identifier -> 400. Missing/invalid auth -> 401. Unsupported explicit transport -> protocol-not-supported (400) or adapter-not-initialized (500).
- Invariants & Guarantees: Read-only; tolerant of an unavailable room (returns empty list, never throws on room failure).

#### Join a conversation view — POST /api/collaboration/conversations/{conversationId}/join
- Invocation: Authenticated client request.
- Inputs: Path parameter `conversationId` (required, positive integer). Request body: optional JSON; a missing or unparseable body is tolerated and treated as empty. Optional body field `protocol` selects the transport. The caller's user identity, username, display label, and role are taken from the authenticated session, not from the body.
- Preconditions & Authorization: Valid auth token; any authenticated staff role.
- Behavior: Registers the calling user as an active viewer/participant of the specified conversation's live room, attaching their identity metadata (username, display label, role).
- Success Output: HTTP 200 with a success envelope and a null data payload, conveying that the user joined.
- Side Effects: The caller becomes observable in subsequent viewer/state reads for that conversation, and a "user joined" type event becomes available to other connected participants of that room (see Real-time / Event Behavior). State is ephemeral session state, not a durable record.
- Error Conditions: Invalid conversation identifier -> 400. Missing/invalid auth -> 401. If the underlying room cannot accept the join, the failure propagates to the standard error handler. A "room full" condition is a defined error concept (machine code indicating the room is full, HTTP 403) when a per-conversation viewer capacity is exceeded.
- Invariants & Guarantees: Joining is the inverse of leaving. A per-conversation maximum-viewer limit is a configured constraint (default capacity is fifty viewers per conversation).

#### Leave a conversation view — POST /api/collaboration/conversations/{conversationId}/leave
- Invocation: Authenticated client request.
- Inputs: Path parameter `conversationId` (required, positive integer). No meaningful body fields; the user identity comes from the authenticated session.
- Preconditions & Authorization: Valid auth token; any authenticated staff role.
- Behavior: Removes the calling user from the active viewer/participant set of the specified conversation's live room.
- Success Output: HTTP 200 with a success envelope and null data payload.
- Side Effects: The caller no longer appears in subsequent viewer/state reads for that conversation, and a "user left" type event becomes available to other connected participants of that room.
- Error Conditions: Invalid conversation identifier -> 400. Missing/invalid auth -> 401. Underlying room failures propagate to the standard error handler.
- Invariants & Guarantees: Idempotent in effect (leaving when not present has no harmful effect). Inverse of join.

#### Send typing indicator — POST /api/collaboration/typing
- Invocation: Authenticated client request.
- Inputs: JSON body with:
  - `conversationId`: required; the conversation the typing indicator applies to (accepted as a number or numeric string and interpreted as an integer).
  - `status`: required; must be exactly "start" or "stop". The user identity comes from the authenticated session.
- Preconditions & Authorization: Valid auth token; any authenticated staff role.
- Behavior: Publishes a typing-start or typing-stop signal for the calling user within the specified conversation's live room, so other connected participants can show or clear the "is typing" hint.
- Success Output: HTTP 200 with a success envelope, null data payload, and a confirmation message reflecting the chosen status.
- Side Effects: A typing-start or typing-stop type event becomes available to other connected participants of that room. Typing indicators are time-limited and auto-expire (default expiry on the order of a few seconds; the indicator is meant to be refreshed or cleared).
- Error Conditions: Missing `conversationId` or missing `status` -> HTTP 400 with a "missing required fields" message. `status` not equal to "start" or "stop" -> HTTP 400 with an "invalid status" message. Missing/invalid auth -> 401. A body that is not valid JSON causes a request-parse failure surfaced through the standard error handler.
- Invariants & Guarantees: Indicators are ephemeral and expire automatically; "stop" cancels an active indicator.

#### Update presence / availability — POST /api/collaboration/presence
- Invocation: Authenticated client request.
- Inputs: JSON body with:
  - `status`: required; must be one of "online", "away", "busy", "offline".
  - `currentConversation`: optional; an identifier of the conversation the user is currently focused on (accepted as number or numeric string, interpreted as an integer when present).
  - `metadata`: optional free-form object of additional attributes.
  - The user identity comes from the authenticated session.
- Preconditions & Authorization: Valid auth token; any authenticated staff role.
- Behavior: Records/updates the calling user's availability status in their per-user live presence session. If a current conversation is supplied, a presence-update signal for that user is additionally published to that conversation's live room.
- Success Output: HTTP 200 with a success envelope and null data payload.
- Side Effects: The user's presence becomes the latest known availability for that user. When a current conversation is provided, a "presence update" type event (carrying the user identifier and new status) becomes available to other connected participants of that conversation.
- Error Conditions: Missing `status` -> HTTP 400 "missing required field". `status` not in the allowed set -> HTTP 400 listing the allowed values. Missing/invalid auth -> 401. Invalid JSON body -> parse failure via standard error handler.
- Invariants & Guarantees: Presence is ephemeral and subject to expiry (default expiry on the order of several minutes of inactivity). Status values are constrained to the four allowed states.

#### Get collaboration statistics — GET /api/collaboration/stats
- Invocation: Authenticated client request.
- Inputs: Optional `protocol` query parameter to scope to a single transport; otherwise statistics are aggregated across all available transports.
- Preconditions & Authorization: Valid auth token; any authenticated staff role.
- Behavior: Returns aggregate counters describing current collaboration activity.
- Success Output: HTTP 200 with a success envelope wrapping a statistics object containing: total active viewers, total active typing indicators, total active rooms, a per-transport connection-count map, and a ranked list of the most active conversations (each entry pairing a conversation identifier with its viewer count, ranked by viewer count and limited to a small top-N set). Note: in the current build these counters report baseline/zero values for the live transport; the shape is stable even when the values are not yet fully populated.
- Side Effects: None; read-only.
- Error Conditions: Missing/invalid auth -> 401. Explicit unsupported transport -> protocol-not-supported (400) or adapter-not-initialized (500).
- Invariants & Guarantees: When no transport is specified, results are merged across transports; the top-active list is capped to a small number of entries.

#### Clean up expired collaboration state — POST /api/collaboration/cleanup
- Invocation: Authenticated client request.
- Inputs: None of significance in the body.
- Preconditions & Authorization: Requires a valid auth token AND the administrator role. Non-administrator callers are rejected.
- Behavior: Triggers a pass that purges expired/stale collaboration state across the available transports and reports how many items were removed. In the current build the live transport relies on its own internal expiry, so the reported removed count is typically zero.
- Success Output: HTTP 200 with a success envelope wrapping an object reporting the count of cleaned items.
- Error Conditions: Caller is authenticated but not an administrator -> HTTP 403 with an "insufficient permissions" message. Missing/invalid auth -> 401.
- Invariants & Guarantees: Administrator-only. Safe to invoke repeatedly.

#### Health check — GET /api/collaboration/health
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Mounted under the authenticated route group; requires a valid auth token.
- Behavior: Reports whether the collaboration subsystem has been initialized along with selected configuration values. Initialization is lazy (it happens on the first business request), so this endpoint may legitimately report a not-initialized state on a cold path, accompanied by an explanatory note advising the caller to retry after activity.
- Success Output: HTTP 200 with a success envelope wrapping: a status label of either "healthy" or "not_initialized"; a configuration subset (the default transport label and whether the WebSocket transport is enabled); the list of available transports; a current timestamp; and, when not yet initialized, an explanatory note.
- Error Conditions: An server failure during the check -> HTTP 500 with a generic "health check failed" message.
- Invariants & Guarantees: Non-mutating; never initializes the subsystem itself.

### Data Concepts (neutral)
- Viewer: a representation of a staff user currently observing a conversation. Carries a numeric user identifier, a username, a human-readable display label, a role classification (one of: administrator, team, agent), a timestamp of when they began viewing, the transport label, a flag indicating whether they are currently typing, and a timestamp of their most recent activity. Viewers with an uninterpretable user identifier are excluded from listings.
- Typing entry: a transient indication that a specific user is composing input in a specific conversation. Carries the user identifier, username, display label, conversation identifier, a start timestamp, and an expiry timestamp after which it is considered stale.
- Presence record: a per-user availability state. Carries the user identifier, an availability status (online / away / busy / offline), an optional current-conversation focus, a last-seen timestamp, and optional free-form metadata. Subject to expiry after inactivity.
- Conversation room state: a composite live snapshot for one conversation, aggregating its viewers, its active typing entries, a live connection count, the transport label, a last-activity timestamp, and optional metadata (such as a recent-message-history count and an active flag).
- Collaboration statistics: aggregate counters — total viewers, total typing indicators, total rooms, a per-transport connection-count map, and a ranked top list of conversations by viewer count.
- All of the above are ephemeral session/runtime state tied to live connections, not durable database rows; there is no soft-delete concept here — entries simply expire or are removed when a user leaves or disconnects.

### State & Lifecycle
- Conversation viewing: a user transitions from "not viewing" to "viewing" by joining, and from "viewing" back to "not viewing" by leaving or disconnecting. Capacity for a single conversation is bounded (default fifty viewers); exceeding it is a defined "room full" failure.
- Typing indicator: transitions from inactive to active on a "start" signal and back to inactive on a "stop" signal or automatically upon expiry (short timeout, default a few seconds).
- Presence: cycles among online, away, busy, and offline as the user updates it; an inactive presence is reclaimed after an expiry window (default several minutes). There is no terminal state beyond expiry/offline.
- Subsystem initialization: the collaboration subsystem is initialized lazily on first business use; until then the health endpoint reports a not-initialized status.

### Real-time / Event Behavior
The following live events are emitted into a conversation's real-time room and are delivered to that room's other connected participants (the originating user may be excluded when an exclusion list is supplied):
- A user-joined event when someone joins a conversation view.
- A user-left event when someone leaves a conversation view.
- A typing-start event when a user signals they are typing (payload identifies the user).
- A typing-stop event when a user signals they stopped typing (payload identifies the user).
- A presence-update event when a user updates their availability while focused on a specific conversation (payload identifies the user and the new status).
- A generic broadcast event mechanism exists that can publish an arbitrary collaboration event type (including message-sent and conversation-update event categories) with a custom data payload and an optional set of user identifiers to exclude from delivery.
These events are the primary way other clients observe collaboration changes; the HTTP read endpoints (state/viewers/stats) provide point-in-time snapshots of the same underlying live state. Event delivery is best-effort over the real-time transport; failures to reach a room are logged and, for read paths, degrade to partial or empty results rather than hard errors.

## Activity Log & Reversible Actions

### Purpose
This area provides a system-wide audit trail. Every meaningful user action across the platform (conversation handling, customer and tag management, team and membership changes, message activity, authentication, file operations, etc.) can be recorded as an immutable activity entry tagged with who did it, what they did, which resource it affected, and contextual metadata. A subset of recorded actions are marked "reversible": for those, an eligible caller can later request an undo that restores the affected resource to its prior state, subject to a time window, conflict detection, and concurrency safeguards. The area also exposes read and statistics endpoints over the audit trail.

All HTTP endpoints below are mounted under a common base path of `/api/activities`. All require a valid authenticated session (bearer/JWT) unless otherwise noted.

### Operations

#### List activity entries — GET /api/activities/
- Invocation: authenticated client request.
- Inputs (query, all optional):
  - page number (integer, default 1)
  - page size (integer, default 50; server caps the effective value at 100)
  - actor identifier filter
  - action-type filter
  - resource-type filter
  - start timestamp (ISO 8601 string)
  - end timestamp (ISO 8601 string)
- Preconditions & Authorization: must be authenticated. Administrators may filter by any actor identifier and thereby view all actors' entries. Non-administrators are silently scoped to their own entries only — any actor filter they pass is overridden with their own identity.
- Behavior: returns a page of audit entries ordered newest-first, plus pagination totals.
- Success Output: a paginated envelope containing the list of entries and pagination metadata (current page, page size/limit, total count). Each entry carries: numeric entry id, actor identifier, actor display name, actor role, action type, resource type, optional resource identifier, optional structured detail object, optional originating IP, optional client descriptor, and creation timestamp. HTTP 200.
- Error Conditions: not authenticated -> 401 with "Unauthorized". Invalid query parameters (e.g. malformed page/page-size out of the 1–1000 validation range, non-ISO date, start after end) -> 422 validation error naming the offending field. Unexpected failure -> 500.
- Invariants & Guarantees: ordering is strictly newest-first by creation time. Non-admin scoping is enforced server-side regardless of supplied filters.

#### Get a single activity entry — GET /api/activities/{id}
- Invocation: authenticated client request.
- Inputs: path id (must be a positive integer; validated by middleware).
- Preconditions & Authorization: must be authenticated. An administrator may view any entry; a non-administrator may view an entry only if they are the actor recorded on it.
- Behavior: looks up and returns the entry.
- Success Output: the full entry object (same fields as the list entry). HTTP 200.
- Error Conditions: not authenticated -> 401. Non-integer/invalid id -> rejected by validation middleware. Entry not found -> 404 "Activity not found". Authenticated but neither admin nor the recorded actor -> 403 "Forbidden". Unexpected failure -> 500.

#### Get per-actor activity statistics — GET /api/activities/user/{userId}/stats
- Invocation: authenticated client request.
- Inputs: path actor identifier; query window length in days (integer, default 30).
- Preconditions & Authorization: authenticated. An administrator may view any actor's stats; a non-administrator may only view their own.
- Behavior: aggregates the actor's entries within the trailing window.
- Success Output: total action count, a breakdown of counts per action type, and a short list of the actor's most recent entries. HTTP 200.
- Error Conditions: not authenticated -> 401. Non-admin requesting another actor -> 403 "Forbidden". Unexpected failure -> 500.

#### Cleanup (purge old entries) — POST /api/activities/cleanup
- Invocation: authenticated client request.
- Inputs: query retention length in days (integer, default 90).
- Preconditions & Authorization: administrator only.
- Behavior: permanently removes audit entries older than the retention cutoff. This is a hard deletion of audit records.
- Success Output: count of removed entries, with a human-readable confirmation message. HTTP 200.
- Error Conditions: not administrator -> 403 "Forbidden". Retention value rejected by validation (must be a positive integer, at least 30 days, and no more than 3650 days) -> 422 validation error. Unexpected failure -> 500.
- Invariants & Guarantees: retention floor of 30 days is enforced — the system refuses to purge to a shorter horizon.

#### Statistics overview — GET /api/activities/overview
- Invocation: authenticated client request. Inputs: query window in days (default 7).
- Authorization: administrator only; otherwise 403.
- Success Output: aggregate counts for the window — total entries, counts per action type, top contributors (display name, role, count), per-day counts, and the resolved window descriptor (days, start, end). HTTP 200. Failure -> 500.

#### Resource-type statistics — GET /api/activities/stats/resources
- Authenticated; administrator only (else 403). Inputs: query window in days (default 30).
- Success Output: per resource-type entries with raw count, percent share, and a localized display label. HTTP 200. Failure -> 500.

#### Role distribution statistics — GET /api/activities/stats/roles
- Authenticated; administrator only (else 403). Inputs: query window in days (default 30).
- Success Output: per actor-role entries with count, percent share, and localized label. HTTP 200. Failure -> 500.

#### Custom-period statistics — GET /api/activities/stats/custom
- Authenticated; administrator only (else 403). Inputs: query start timestamp and end timestamp (both required).
- Behavior: aggregates entries within the explicit window (same shape as the overview).
- Success Output: overview-shaped aggregate for the given range. HTTP 200.
- Error Conditions: missing either start or end -> 422 validation error ("Start date and end date are required"). Non-admin -> 403. Failure -> 500.

#### Activity trends — GET /api/activities/trends
- Authenticated; administrator only (else 403). Inputs: query window in days (default 30).
- Success Output: per-day series, each day carrying a total count and a per-action-type count breakdown. HTTP 200. Failure -> 500.

#### Activity heatmap — GET /api/activities/heatmap
- Authenticated; administrator only (else 403). Inputs: query window in days (default 30).
- Success Output: per (day, hour-of-day) buckets with a count and an intensity classification of low / medium / high (thresholds: high at 50+, medium at 20+, otherwise low). HTTP 200. Failure -> 500.

#### Performance metrics — GET /api/activities/metrics
- Authenticated; administrator only (else 403). Inputs: query window in days (default 7).
- Success Output: average entries per day, the peak hour-of-day, the most active contributor's display name (or null), the most common action type (or null), and a coarse system-load classification of low / medium / high (thresholds: high above 1000/day, medium above 500/day, otherwise low). HTTP 200. Failure -> 500.

#### Restore (undo a reversible action) — POST /api/activities/{id}/restore
- Invocation: authenticated client request targeting a previously recorded reversible activity entry.
- Inputs:
  - path id (positive integer; validated by middleware).
  - request body, optional: a boolean force flag (default false). A missing or unparseable body is treated as force = false.
  - The caller identity is read from the authenticated session. For test contexts an alternate caller may be supplied via a test-only request header carrying a JSON object with id and role.
- Preconditions & Authorization:
  - The target entry must exist and must be flagged reversible.
  - It must not already be restored, and a restore must not currently be in progress for it.
  - Authorization rule: an administrator may always restore; the original actor may restore only if the entry's restore policy does not require admin. Anyone else is forbidden.
  - The entry's restore window must not have elapsed.
  - The entry must carry a resource identifier and reference a known restore strategy and a supported resource type.
- Behavior (observable ordering):
  1. Validate id, load the entry, confirm it is reversible and not yet/already restored.
  2. Authenticate the caller and enforce the authorization rule above.
  3. Confirm the restore window is still open.
  4. Resolve the target resource's current state. If the resource no longer exists and the strategy does not permit a missing current state, the restore is rejected; for membership-style restores a missing current state is permitted.
  5. Run conflict detection: compare the resource's current state against the state recorded at the time of the original action. If any tracked field has drifted since then and force is not set, reject with a conflict report.
  6. Atomically claim the restore (a guarded one-time transition) so concurrent restore attempts cannot both proceed.
  7. Apply the reversal mutation that returns the resource toward its prior recorded state, and in the same atomic batch record a new audit entry describing the restore action.
  8. Link the original entry to the new restore audit entry so it is now marked restored.
  9. Emit a real-time broadcast announcing the restoration.
- Success Output: a success envelope containing the original entry id that was restored and the id of the newly created restore audit entry. HTTP 200.
- Side Effects: the affected resource's persisted state is reverted (e.g. a re-assigned/transferred conversation returns to its prior owning team and related fields; a soft-deleted record is undeleted; a created record is soft-deleted; a tag assignment is removed or re-added; a removed team membership is reinstated). A new immutable audit entry is appended for the restore itself (recorded as not-itself-reversible). The original entry becomes permanently marked as restored, referencing the new entry. A real-time event is broadcast to clients.
- Error Conditions (each maps to a machine-readable code where noted):
  - Non-positive/non-integer id -> 400 "Invalid activity id".
  - Entry not found -> 404 "Activity not found".
  - Entry not flagged reversible -> 422, code NOT_REVERSIBLE.
  - A restore is already underway -> 409, code RESTORE_IN_PROGRESS, with a suggested retry delay.
  - Entry already restored -> 409, code ALREADY_RESTORED, including the id of the entry that performed the restore.
  - Caller not authenticated -> 401 "Unauthenticated".
  - Caller not permitted -> 403 "Forbidden".
  - Restore window elapsed -> 410, code RESTORE_EXPIRED.
  - Entry has no resource identifier -> 422, code MISSING_RESOURCE_ID.
  - Unknown restore strategy reference -> 422, code RESTORE_HANDLER_NOT_FOUND.
  - Target resource no longer exists (and strategy disallows missing state) -> 422, code RESOURCE_NOT_FOUND.
  - Detected drift without force -> 409, code RESTORE_CONFLICT, with a list of changed fields (see conflict report below).
  - Unsupported resource type -> 422, code UNSUPPORTED_RESOURCE_TYPE.
  - The atomic claim is lost to a concurrent restore -> 409 (RESTORE_IN_PROGRESS or ALREADY_RESTORED depending on the winner's state).
  - The reversal batch fails to apply -> 500, code BATCH_FAILED; in this case the in-progress claim is released so a later retry is possible.
- Invariants & Guarantees:
  - Exactly-once restoration: a guarded transition ensures only one restore can succeed for a given entry; concurrent or repeated attempts observe IN_PROGRESS or ALREADY_RESTORED.
  - Restore and its audit logging happen together atomically.
  - Conflict safety: by default a restore is refused if the resource changed after the original action; the caller may override with force.
  - Time-bounded: restoration is only possible within the entry's restore window (defaulting to 24 hours from the original action unless otherwise specified at capture time).
  - Audit immutability: restoring does not erase the original entry; it appends a new entry and links them.
  - The real-time broadcast is best-effort and never causes an otherwise-successful restore to fail.

### Data Concepts (neutral)

- Activity Entry: an audit record of one user action. Carries a numeric identifier, the actor's identifier, the actor's display name, the actor's role (administrator or agent), an action-type code, a resource-type code, an optional affected-resource identifier, an optional structured detail object, optional originating IP and client descriptor, and a creation timestamp. Audit entries are append-only; they are not individually edited or soft-deleted, but bulk-purged by age via the cleanup operation.

- Action-type codes (neutral catalog): a fixed vocabulary covering conversation lifecycle (assign, transfer, close, reopen, unassign, bulk-assign), messaging (send, recall, forward, received), authentication (login, logout), user/member management (create, update, delete, bulk delete, bulk update, restore), system settings and integration creation, team management (create, update, delete, invite, member update, member remove, member add/remove), QR-code generation, customer management (create, update, delete) and customer interaction signals (followed, unfollowed), tag management (create, update, delete, assign, unassign, bulk update), delayed-message scheduling and cancellation, system monitoring/backup/restore, and a dedicated family of restore action codes (conversation/customer/tag/team/team-member/delayed-message/user restore). Recording rejects any action code or resource-type code outside this vocabulary, and any actor role outside {administrator, agent}.

- Resource-type codes (neutral catalog): conversation, message, user, team, customer, system, file, QR code, webhook, integration, tag, team member, delayed message.

- Reversible-action metadata (embedded in an entry's detail object at capture time): a reversibility flag; a reference to which restore strategy applies; a snapshot of the resource's prior state; a snapshot of the resource's state immediately after the original action; a restore policy carrying an expiry timestamp and a "requires administrator" flag; and a restore-state indicator with three observable values — not yet restored, restore in progress, or restored (in which case it references the restore audit entry).

- Conflict report: a list of per-field drift descriptors. Each names the field, the field's value recorded at the time of the original action, the field's current value now, and the value the field would take if the restore proceeds. A field is reported as changed when its current value differs from the value recorded at the time of the original action, including changes within list or nested values.

- Restore strategies (neutral): the system maps each reversible action to one undo strategy. The strategy families observable through behavior are: undelete a soft-deleted record; soft-delete a record (to undo a creation); revert a single named field; revert a set of named fields; remove a previously added tag association; add back a previously removed tag association; and reinstate a removed team membership (this last one tolerates the membership being currently absent). Restore strategies operate only on a fixed allowlist of resource collections and fields; an attempt to restore a field outside the allowlist, or with required prior-state values missing, causes the reversal batch to fail (surfaced as BATCH_FAILED).

### State & Lifecycle

Reversible-entry lifecycle (observable restore states):
- Not yet restored: the only state from which a restore may begin; eligible for undo while inside the restore window and free of unforced conflicts.
- Restore in progress: entered when a restore claims the entry; concurrent restore attempts are rejected with IN_PROGRESS. This is transient — it advances to "restored" on success, or returns to "not yet restored" if the reversal batch fails.
- Restored (references the restore audit entry): terminal. Further restore attempts are rejected with ALREADY_RESTORED.
- Expired: once the restore window passes, the entry can no longer be restored (RESTORE_EXPIRED), regardless of link state.

Authorization to trigger a restore transition: administrators always; the original actor only when the entry's policy does not require administrator privileges.

A representative reversible flow (conversation transfer/reassignment): the original action records prior and post field snapshots for the affected conversation; an authorized undo reverts the conversation's stored fields (such as its owning team) to the prior snapshot, provided the conversation has not drifted in the interim (or force is supplied), and appends a "conversation restore" audit entry.

### Real-time / Event Behavior

- Emitted on a successful restore: a real-time event named `resource.restored`, broadcast through the routed delivery path to connected clients. Payload fields: the affected resource type, the affected resource identifier, the identifier of the caller who performed the restore, and the identifier of the newly created restore audit entry.
- Trigger condition: emitted only after the reversal and its audit entry have been committed. Delivery is best-effort; a broadcast failure is swallowed and does not affect the already-committed restore or its HTTP success response.
- This area does not itself consume real-time events; it is a producer of audit records (consumed by other areas that call the recording service after their own state changes) and of the restore broadcast.


---

# 4. Channels & Integration

## Channel Integrations

### Purpose
This area lets a team connect outside messaging platforms (LINE, Facebook Messenger, with WhatsApp also accepted and the structure designed so additional platforms can be added without storage changes) to the support system. Administrators register a platform connection for their team by supplying the platform's identifiers and secret credentials; the system stores the secrets encrypted, generates a per-connection inbound webhook address with a secret routing token, lets operators verify the connection by calling the live platform API, exposes usage statistics and a health indicator, and allows the connection to be enabled or disabled. Each team may have at most one *active* connection per platform.

### Operations

All management operations are authenticated client (HTTP/JSON) requests under a `/api/channels` path family and require a valid session token. The caller's identity carries a system role (notably `admin`) and a primary team association used for ownership scoping. Responses generally follow a `{ success: boolean, ... }` envelope. Secret credentials are NEVER returned in any response; every record returned to a client has its encrypted-credential blob stripped out before serialization.

#### List connections for a team — GET /api/channels
- Invocation: authenticated client request.
- Inputs (query, all optional): a team identifier (numeric); a platform filter accepting `line`, `facebook`, or `whatsapp`.
- Preconditions & Authorization: caller must be authenticated. A caller with a primary team is scoped to that team. A caller with the `admin` role and no primary team may pass a team-identifier query parameter to scope to a specific team, or omit it to list connections across all teams. A non-admin caller with no primary team is rejected.
- Behavior: returns the set of connection records matching the team (and optional platform) filter, newest first.
- Success Output: `success: true`, a `data` array of sanitized connection records (no credentials), and a `count` of how many were returned. Status 200.
- Error Conditions: not authenticated -> 401 with an authentication-required error. Admin supplied a non-numeric team-identifier -> 400 invalid-parameter error. Non-admin without a resolvable team -> 400 team-not-found error.

#### Create a connection — POST /api/channels
- Invocation: authenticated client request.
- Inputs (JSON body): a required `platform` (one of `line`, `facebook`, `whatsapp`); an optional team identifier (used only when the admin caller has no primary team); an optional free-form configuration-metadata object; and exactly one platform-specific configuration object matching the platform:
  - LINE config: a channel identifier, a channel access token, and a channel secret — all required.
  - Facebook config: a page identifier, an access token, and an app secret — all required.
  - WhatsApp config: a phone number, a business-account identifier, and an access token — all required.
- Preconditions & Authorization: caller must be authenticated AND have the `admin` role; otherwise rejected. The target team is the caller's primary team, or (for an admin without one) the team identifier from the body; if neither is available the request is rejected. A team may not already have an *active* connection for the same platform.
- Behavior (observable order): validates role, resolves the team, validates the platform value and the required platform-specific fields, rejects if an active same-platform connection already exists, otherwise generates a fresh random secret webhook token and a corresponding inbound webhook address, encrypts the supplied secrets, persists a new connection (initially enabled and not-yet-verified) with separated non-sensitive configuration, encrypted credentials, webhook configuration, and zeroed usage statistics, then records an audit entry of the creation.
- Success Output: `success: true`, the sanitized created record in `data`, and the generated inbound `webhookUrl`. Status 201.
- Side Effects: a new persisted connection; an activity/audit log entry capturing the actor, platform, team, and new connection identifier, plus the caller's network address and user-agent.
- Error Conditions: not authenticated -> 401. Not admin -> 403 with an administrators-only message. No resolvable team -> 400. Missing/invalid platform -> 400. Missing required platform-specific config -> 400 with a field-specific message. Existing active same-platform connection -> the create fails with `success: false` and an explanatory error, returned as 400. Persistence returning no record -> 500. Unexpected error -> standardized error response.

#### Get one connection — GET /api/channels/:id
- Invocation: authenticated client request.
- Inputs: path identifier (validated as an integer).
- Preconditions & Authorization: caller must be authenticated. The connection must belong to the caller's primary team; an `admin` caller may access a connection of any team; a caller whose team differs and who is not admin is denied.
- Behavior: looks up the connection by identifier and returns it (credentials stripped).
- Success Output: `success: true` with the sanitized record in `data`. Status 200.
- Error Conditions: not authenticated -> 401. Not found -> 404. Belongs to another team and caller not admin -> 403. Non-integer path id -> validation rejection.

#### Update a connection — PUT /api/channels/:id
- Invocation: authenticated client request.
- Inputs: path identifier; JSON body any of: a partial LINE/Facebook/WhatsApp config object (only fields to change), an enabled/disabled boolean, and a configuration-metadata object.
- Preconditions & Authorization: caller authenticated, must have a primary team, must have `admin` role, and the connection must belong to the caller's primary team; otherwise denied.
- Behavior: only the config block matching the connection's own platform is applied. Non-secret fields (identifiers, phone number, business-account id) are merged over existing values. Secret fields (tokens, secrets) replace prior values only when supplied; omitted secrets keep their current encrypted values. The enabled flag and metadata are updated when present. If any secret value changes, the connection's verified status is cleared and its last-verified marker is removed.
- Success Output: `success: true` with the sanitized updated record in `data`. Status 200 on success.
- Side Effects: persisted changes to the connection; re-encryption of credentials when secrets change; verified status reset when secrets change.
- Error Conditions: missing team context -> 400. Not admin -> 403. Not found -> 404. Belongs to another team -> 403. Service-level failure -> `success: false` returned as 400.

#### Disable a connection (soft delete) — DELETE /api/channels/:id
- Invocation: authenticated client request.
- Inputs: path identifier.
- Preconditions & Authorization: caller authenticated, must have a primary team, must have `admin` role, and the connection must belong to the caller's team.
- Behavior: marks the connection as disabled (not enabled); it is NOT physically removed. A disabled connection no longer counts against the one-active-per-platform rule and is no longer reachable via inbound webhook token lookup.
- Success Output: `success: true` and a confirmation message. Status 200.
- Error Conditions: missing team context -> 400. Not admin -> 403. Not found -> 404. Belongs to another team -> 403. Underlying disable failure -> 500.

#### Verify a connection — POST /api/channels/:id/verify
- Invocation: authenticated client request.
- Inputs: path identifier; optional JSON body field carrying a test message (currently accepted but not required; absent/invalid body is tolerated and treated as empty).
- Preconditions & Authorization: caller authenticated and must have a primary team; the connection must belong to the caller's team (otherwise denied). The connection must be enabled to be verifiable.
- Behavior: decrypts the stored credentials and performs a live call to the platform's own API to confirm the credentials are valid:
  - LINE: validates the access token against LINE's token-verify endpoint.
  - Facebook: fetches the configured page's profile using the access token and page identifier.
  - WhatsApp: fetches the configured phone number's metadata using the access token and business-account identifier.
  On success the connection is marked verified, a last-verified timestamp is set, and its error counter and last-error record are cleared. On failure the connection's error counter is incremented and a structured last-error record is stored.
- Success Output: a verification result object: `success`, `verified`, a human-readable `message`, and a `details` object with platform-relevant fields (e.g. resolved client/channel identifier and webhook address for LINE; page id and page name for Facebook; phone-number id, display phone number, and verified name for WhatsApp; plus the last-verified timestamp). Status 200 when verified, 400 when not verified.
- Side Effects: persisted verified status + timestamp on success; persisted error-count increment + last-error record on failure; an outbound network call to the third-party platform.
- Error Conditions: missing team context -> 400. Connection not found -> 404 (handler) or a not-found verification result. Belongs to another team -> 403. Connection disabled -> `verified: false` with a not-active message. Missing required credential/identifier -> `verified: false` with a specific message. Third-party API non-success -> `verified: false` echoing the platform status. Unsupported platform -> `verified: false` with a not-supported message.

#### Get connection statistics — GET /api/channels/:id/stats
- Invocation: authenticated client request.
- Inputs: path identifier.
- Preconditions & Authorization: caller authenticated and must have a primary team; the connection MUST belong to the caller's primary team (strict equality — admins are not granted cross-team access on this endpoint).
- Behavior: returns aggregate usage figures for the connection.
- Success Output: `success: true` with `data` containing: the connection identifier and platform; total messages sent; total messages received; the timestamp of the last message (or null); whether the connection is enabled; whether it is verified; the current error count; and an uptime object expressing whole days since creation plus a fixed hours-in-last-day figure.
- Error Conditions: missing team context -> 400. Not found -> 404. Belongs to another team -> 403.

#### Get connection health — GET /api/channels/:id/health
- Invocation: authenticated client request.
- Inputs: path identifier.
- Preconditions & Authorization: same strict same-team ownership rule as statistics.
- Behavior: derives a health classification from the connection's accumulated error count: healthy when there are no errors, degraded when there are a few, and down once errors exceed a threshold.
- Success Output: `success: true` with `data` containing: connection identifier and platform; a status of `healthy`, `degraded`, or `down`; the time of this check; the consecutive-error count; the most recent stored error record (or null); and a recommendations list.
- Error Conditions: missing team context -> 400. Not found -> 404. Belongs to another team -> 403.

### Data Concepts (neutral)

- **Channel Connection**: the central record. Carries: a numeric identifier; the owning team; the platform name (an open string — `line`, `facebook`, `whatsapp` are validated at the management API, but the storage model accepts arbitrary platform names for extensibility); an enabled flag; a verified flag and last-verified timestamp; an error counter and a most-recent structured error record; creation and update timestamps; an optional free-form metadata blob; and four separated JSON payloads described below. Credentials are always omitted from any client-facing serialization.
- **Non-sensitive configuration payload**: platform-relevant non-secret identifiers (e.g. LINE channel identifier, Facebook page identifier, WhatsApp phone number and business-account identifier), plus room for arbitrary future keys — this is the extensibility mechanism so new platforms need no storage changes.
- **Encrypted credentials payload**: the platform secrets (access tokens, channel secret, app secret, etc.) stored only in encrypted form using a tamper-evident encryption scheme, and never returned in any client-facing serialization. The system tolerates historical credential storage formats so that previously stored connections remain readable.
- **Webhook configuration payload**: the generated inbound webhook address, a secret routing token, and an optional platform verification token.
- **Usage statistics payload**: running totals of messages sent and received and the timestamp of the most recent message; defaults to zeroed counters when absent or unparseable.
- **Structured error record**: a timestamp, an error category label, a human-readable message, an attempt/retry counter, an optional captured stack, and an optional context object.

Lifecycle: connections are soft-disabled, never hard-deleted, via the enabled flag. Newest-first ordering is used wherever multiple connections are returned.

### State & Lifecycle

- **Enabled vs disabled**: a connection is created enabled. An admin may toggle it via update or disable it via delete. Only enabled connections (a) count toward the one-active-per-platform-per-team uniqueness rule, (b) are eligible for verification, and (c) are resolvable by inbound webhook token lookup. Disabling is reversible by re-enabling through update (subject to the uniqueness rule).
- **Verified vs not-verified**: a connection is created not-verified. A successful verification sets verified and records a last-verified timestamp. Any change to a secret credential during update clears verified status and the last-verified marker, requiring re-verification. A failed verification leaves the connection not-verified.
- **Health progression**: healthy -> degraded -> down as the error counter rises across failed verifications/operations; a successful verification resets the counter and the last-error record back to a clean (healthy) state.
- **Uniqueness invariant**: at most one *enabled* connection per (team, platform). Creation is rejected while an enabled one exists; the owner must disable the existing one first. Disabled duplicates may coexist.

### Real-time / Event Behavior

- **Audit event on creation**: creating a connection emits an audit/activity entry recording the actor, platform, team, and new connection identifier (with caller network address and user-agent). No real-time websocket broadcast is emitted by the management endpoints themselves.
- **Inbound webhook reception (related side of this area)**: each connection's generated inbound address embeds the platform, team, and secret routing token; the system can resolve an enabled connection from a presented (platform, team, token) triple, rejecting the lookup if the token does not match the stored token or the connection is disabled. Inbound platform webhooks are authenticated by platform-specific request-signature verification (not by the session token), enforce a payload-size ceiling (about one megabyte), respond to the platform's subscription-verification challenge where applicable, process each contained event independently so one failing event does not abort the rest, and return a server-error status when one or more events fail so the platform will retry the batch (processing is de-duplicated so retried successful events are not double-counted). Successful inbound and outbound messages increment the per-connection sent/received counters and update the last-message timestamp.

## Inbound Webhook Ingestion & Platform Parsing

### Purpose
This area is the public ingress for messaging-platform events. External chat platforms (a LINE-style messaging platform, and a Facebook/Instagram-style messaging platform) push event batches to fixed webhook endpoints. The system authenticates each delivery cryptographically, validates the payload shape and size, rejects replays/duplicates, normalizes each platform's raw event into a common internal message representation, persists the resulting customer/conversation/message records, and emits real-time updates to connected staff clients. It also exposes helpers for generating and validating the externally-configured webhook URLs and supports the platform handshake used to register a webhook subscription.

### Operations

#### Inbound message webhook for the LINE-style platform — POST /api/webhook
- Invocation: Unauthenticated public HTTP request originated by the external platform. No session/JWT; trust is established by signature verification only. A companion `GET /api/webhook` exists purely as a readiness probe (see below).
- Inputs:
  - Request body: raw UTF-8 text of a JSON object containing a destination identifier (string) and an array of event objects. Each event carries an event kind (e.g. message / follow / unfollow / other), a numeric timestamp, a source descriptor (with optional end-user identifier, group identifier, room identifier), an optional reply credential (short-lived), and, for message events, a nested message object. The message object carries a kind (text / image / video / audio / file / location / sticker / unknown), and kind-specific fields: text body; a platform media identifier; media file name and size; location title/address/latitude/longitude; sticker package and sticker identifiers; media duration.
  - Required header: a signature header carrying the request's signature. Header name is matched case-insensitively.
- Preconditions & Authorization:
  - A configured platform channel secret must be present in the environment.
  - The request body byte length must not exceed 1 MB.
  - The signature header must be present and must match a signature computed over the exact raw request body keyed by the channel secret, following the external platform's published signature scheme. A missing or non-matching signature causes rejection.
- Behavior (observable order):
  1. Body size is checked first; oversize is rejected before any other work.
  2. Signature is verified against the raw bytes.
  3. Body is parsed as JSON; malformed JSON is rejected.
  4. Payload shape is validated (must be an object; must contain an events array).
  5. Each event is processed independently and sequentially. A failure on one event does not abort the others; failures are counted.
  6. For a message event with a message present: the event is deduplicated by its platform message identifier (already-seen identifiers are skipped entirely, performing no customer/conversation side effects). For a non-duplicate, the message is normalized, the sending customer is found-or-created, an open conversation is found-or-created (optionally team-assigned), the message is persisted, an automatic-reply rule evaluation is attempted synchronously (because the reply credential is short-lived), and non-critical follow-up work is deferred to run after the HTTP response (real-time broadcast, media retrieval enqueue, activity logging, notification).
  7. For a follow event: see the follow lifecycle operation below.
  8. For an unfollow event: see the unfollow lifecycle operation below.
  9. Other event kinds are ignored.
  10. After the loop, if any event failed, an alert is recorded and dispatched (deferred) and the whole batch is reported as failed so the platform retries.
- Normalization rules (observable in stored content and downstream events):
  - text -> kind text, content is the text body (empty string if absent).
  - image -> kind image, content is a fixed image placeholder label; media reference points at the platform's content retrieval URL plus a preview URL.
  - video -> kind video, fixed video placeholder label; media reference includes content URL and preview URL.
  - audio -> kind audio, fixed voice placeholder label; media reference includes content URL and a duration (default 0).
  - file -> kind file, content is a file placeholder label followed by the file name (or "Unknown file"); media reference includes content URL, file name, file size.
  - location -> kind location, content is a location label combining title (default "Location") and address (default "Unknown address"); media reference carries title, address, latitude, longitude (defaulting numerics to 0).
  - sticker -> kind sticker, content is a fixed sticker placeholder label; media reference carries package and sticker identifiers.
  - unknown/unsupported -> content is a bracketed label echoing the raw kind; treated as text-equivalent; original kind retained in metadata.
  - Type self-correction: if a message declares a non-file kind but carries a file name, it is reclassified as a file.
- Success Output: HTTP 200 with a standard success envelope (a success flag, a human-readable confirmation message, and a null data field). Returned only when zero events failed.
- Side Effects: may create or update a customer record; may create or update an open conversation (and backfill its team assignment); persists a new inbound message; updates the conversation's most-recent-activity marker; triggers a latest-message cache refresh; emits a real-time new-message event; may enqueue media retrieval; may evaluate and send an automatic reply; logs an activity entry; may trigger a new-conversation notification; on partial failure records an audit alert and may post to an external alert sink.
- Error Conditions:
  - Body over 1 MB -> HTTP 413, message "Payload too large".
  - Missing signature header -> HTTP 401 (unauthorized) with reason "Missing signature header" equivalent.
  - Missing configured channel secret -> HTTP 401 with reason indicating the secret is missing.
  - Signature mismatch or verification error -> HTTP 401 with reason "Invalid signature" (or the underlying error message).
  - Invalid JSON -> HTTP 400-class error with "Invalid JSON payload".
  - Payload not an object / missing events array -> error response listing validation errors.
  - One or more events fail during processing -> HTTP 500 with a message stating how many of how many events failed; the platform is expected to retry the batch.
- Invariants & Guarantees:
  - Per-platform-message-identifier idempotency: a redelivery of the same message identifier produces no new message row and no duplicated side effects. A unique-constraint race on insert is caught and resolved to the existing record's identifier rather than erroring.
  - Concurrent inbound deliveries for the same customer never produce duplicate open conversations: conversation creation is serialized per customer, so at most one open conversation results.
  - Successfully processed events are not re-applied on retry due to the dedup guarantee, even though the whole batch is retried on partial failure.
  - The conversation's most-recent-activity marker advances only when a message row is actually inserted (never on a pure redelivery).

#### LINE-style webhook readiness probe — GET /api/webhook
- Invocation: Unauthenticated HTTP GET (browser/console verification).
- Behavior: Returns a static readiness descriptor.
- Success Output: HTTP 200 JSON with a success flag, a readiness message, a current timestamp, the endpoint path, and the expected method ("POST").
- Side Effects: none.

#### Subscription verification + inbound message webhook for the Facebook/Instagram-style platform — ALL /api/webhooks/facebook
- Invocation: Unauthenticated public HTTP request from the external platform. Handles both the subscription handshake (GET) and event delivery (POST) on the same path.
- Inputs:
  - Subscription handshake (query parameters): a mode value, a verification token, and a challenge string.
  - Event delivery (body): raw JSON text with a top-level object type and an array of entries; each entry has an identifier, a time, and optionally an array of messaging items and/or an array of field-change items. Each messaging item carries sender id, recipient id, timestamp, and optionally a message or a postback. The message object carries an optional platform message identifier, optional text, and an optional attachments array; each attachment has a kind (image / video / audio / file / location / fallback / other) and a payload (URL, title, geo coordinates).
  - Headers: a signature header carrying the request signature (matched case-insensitively); optionally a content-length header.
- Preconditions & Authorization:
  - Subscription handshake succeeds only when the mode equals the platform's subscribe value and the supplied verification token exactly equals the configured verification token.
  - For event delivery: a configured application secret must be present (two environment names are accepted, in priority order). The signature header must be present and must match a signature computed over the exact raw request body keyed by the application secret, following the external platform's published signature scheme (any platform-defined prefix marker is tolerated). A missing or non-matching signature causes rejection.
- Behavior (observable order):
  1. If the request is a valid subscription handshake, the challenge string is echoed back verbatim and processing stops.
  2. If a content-length header indicates more than 1 MB, the request is rejected before reading the body.
  3. The raw body is read; the app secret presence is checked; the signature is verified against raw bytes.
  4. The body is parsed as JSON; malformed JSON is rejected.
  5. Payload shape is validated (must be an object with a valid object type and an entries array; accepted object types are page / instagram / user).
  6. Only when the object type is the platform's page object type are events processed. For each entry's messaging items that contain a message: the item is processed independently. Each is deduplicated by platform message identifier; non-duplicates are normalized, the customer is found-or-created, an open conversation is found-or-created, the message is persisted, and deferred follow-up work runs after the response (broadcast, media retrieval + a follow-up media-processed broadcast, activity logging, notification when the conversation has a team).
  7. Failures are counted per item; one failure does not abort the others.
- Normalization rules:
  - Plain text present -> kind text with the text body.
  - First attachment is used to determine kind: image/video/audio map to their kinds with a media reference holding the attachment URL; file maps to kind file with a file placeholder label plus the attachment title; location maps to kind location with a label and coordinates; a fallback attachment maps to text using the attachment title (or a link placeholder); any other kind maps to a bracketed label echoing the raw kind. All raw attachments are retained in metadata; quick-reply and reply-to context, when present, are retained in metadata.
  - When neither text nor attachments are present, content becomes a fixed unknown-message placeholder.
- Success Output:
  - Handshake: HTTP 200 with the challenge string as the body (empty string if no challenge).
  - Event delivery: HTTP 200 with a standard success envelope when zero items failed.
- Side Effects: same family as the LINE-style operation (customer/conversation/message persistence, activity log, notification, latest-message cache refresh), plus: a real-time new-message broadcast, asynchronous media retrieval followed by a real-time message-updated broadcast signaling media is ready; on partial failure an audit alert plus optional external alert dispatch.
- Error Conditions:
  - Content-length over 1 MB -> HTTP 413 "Payload too large".
  - Missing app secret configuration -> HTTP 401 "Webhook not configured".
  - Missing/invalid signature -> HTTP 401 with reason "Invalid signature" (or underlying message).
  - Invalid JSON -> HTTP 400-class "Invalid JSON payload".
  - Invalid payload shape (bad/absent object type, disallowed object type, missing entries array) -> error response "Invalid webhook payload" / listed errors.
  - One or more items fail -> HTTP 500 stating how many of how many Facebook events failed.
- Invariants & Guarantees: same idempotency, duplicate-conversation prevention under concurrent delivery, and retry-on-partial-failure semantics as the LINE-style operation.

#### Follow / opt-in lifecycle handling (LINE-style)
- Invocation: Triggered by a follow event inside the LINE-style message webhook batch.
- Inputs: source end-user identifier (required; event ignored if absent), optional source group identifier, optional reply credential, and optional tracking parameters that may carry a team-routing token (sourced from a standard follow parameter, a one-time link token, or an embedded mini-app context value).
- Behavior:
  - The end-user's profile (display name, avatar) is fetched from the platform; failure is tolerated and a default display name is used (a previously captured name may be used as fallback).
  - A team assignment is resolved by checking, in priority order, an existing stored customer-to-team assignment, then a tracking-token lookup. Result determines whether the customer is auto-routed to a team.
  - The customer record is created if absent, then its profile is updated with the latest name/avatar and follow metadata (including a last-followed timestamp and, when routed, an assigned-via-tracking marker).
  - If a team was resolved: an existing non-closed conversation is reused (and team-backfilled if it had no team), otherwise a new active, normal-priority conversation is created for that team.
  - A real-time conversation-assignment/transfer event is broadcast carrying enough data for clients to reconcile a previously-pending conversation against this confirmed one.
  - A welcome auto-reply is attempted (before notifications, because the reply credential is short-lived); if no welcome rule matches, a default localized welcome message is sent and also stored so the conversation is not empty.
  - An activity entry is logged and a customer-followed notification is triggered.
- Side Effects: customer create/update; possible conversation create/update; a real-time transfer/assignment broadcast; optional outbound welcome message (stored as a system-authored message and the conversation activity marker advanced); activity log; notification.
- Error Conditions: absent end-user identifier -> silently ignored (no error to caller). Profile sync, tracking lookup, broadcast, welcome send, notification, and activity logging failures are individually tolerated and logged without aborting the follow flow. A thrown error from the overall follow flow propagates to the batch loop and counts as a failed event (driving a batch-level 500 + retry).

#### Unfollow / opt-out lifecycle handling (LINE-style)
- Invocation: Triggered by an unfollow event inside the LINE-style message webhook batch.
- Inputs: source end-user identifier (required; ignored if absent).
- Behavior: locates the existing customer for that platform identifier; if found, advances the customer's last-updated marker and logs an unfollow activity entry. If no customer is found, the event is a no-op.
- Side Effects: customer last-updated marker advanced; activity entry logged (failure tolerated).
- Error Conditions: absent identifier -> ignored. A thrown error propagates to the batch loop and counts as a failed event.

#### Webhook URL helper behavior
- Invocation: configuration and administration surfaces request webhook URL derivation.
- Capabilities: derive the full externally-facing webhook URL for a given platform from the environment base URL; produce a config object including base URL, platform path, full URL, and environment label (production vs development); list all platform webhook URLs; validate that a string is a well-formed http/https URL with a host and a non-root path; detect whether a URL is a webhook endpoint and which platform it targets by inspecting its path segments; build a webhook URL with appended query parameters; compare two webhook URLs for equivalence ignoring query string and trailing slash. Recognized platform path segments include the LINE-style, Facebook-style, Instagram-style, plus reserved WhatsApp-style and Telegram-style paths.
- Guarantees: URL validation rejects non-http(s) schemes, host-less URLs, and root-only paths; platform extraction returns nothing for non-webhook paths; comparison normalizes trailing slashes and ignores query parameters.

### Data Concepts (neutral)
- Normalized inbound message: a platform-agnostic representation carrying display content (a human-readable string, often a localized placeholder for non-text media), a message kind (text / image / video / audio / file / location / sticker), the originating platform, an optional media reference, and a free-form metadata bag (e.g. raw attachments, sticker identifiers, location detail, quick-reply/reply-to context, original-kind echo for unsupported types).
- Media reference: an optional descriptor for downloadable or external media, carrying a media kind, optional platform media identifier, an external/origin content URL and optional preview URL, optional duration, and optional file name/size. Location and sticker messages are explicitly treated as non-downloadable.
- Customer: an end-user on a given platform, keyed by the platform plus the platform-specific user identifier, carrying a display name, avatar, and a metadata bag (follow history, routing markers). Soft-deleted customers are excluded from active lookups.
- Conversation: a thread between a customer and the support org, with a lifecycle status, a priority, an optional assigned team, a most-recent-activity marker, and timestamps. Lookups consider only conversations that are not in the closed state.
- Inbound message record: belongs to a conversation, authored by a customer (or system, for the default welcome), with content, a kind, a sender display name, a delivery state (treated as delivered on ingest), a serialized media/metadata blob, and the originating platform message identifier used for deduplication (effectively unique).
- Customer-to-team routing assignment: a record linking a platform user identifier to a team, with a source label and an assigned-at timestamp; the most recent one wins.
- Failure alert record: an audit entry capturing platform, failed/total event counts, last error, and timestamp; retained for seven days; optionally mirrored to an external alert sink.
- Replay record (security utility): a per-request-identifier marker with a first-seen timestamp and an occurrence counter, retained one hour, used to flag repeated deliveries.
- Rate-limit counter (security utility): per-integration and per-platform request counters within a fixed time window.

### State & Lifecycle
- Conversation status: created in an active state; an open (non-closed) conversation is reused for new inbound messages; closed conversations are never reused (a new active conversation would be created instead). Team assignment can transition from unassigned to assigned (backfilled on a later message or follow event) but the reverse is not performed during ingestion.
- Customer follow state: a follow event marks the customer as followed (with a last-followed timestamp and possible routing); an unfollow event records the opt-out by advancing the customer's last-updated marker. Customers are never hard-deleted by this area; deactivation/removal elsewhere uses soft-delete and removes the record from active ingestion lookups.
- Message delivery state on ingest: inbound messages are recorded as delivered.

### Real-time / Event Behavior
- New-message event: emitted (deferred, after the HTTP response) for every successfully persisted inbound message. Payload includes the conversation identifier and a message object (identifier, content, kind, sender type "customer", sender identifier, platform, timestamp, delivery state, and serialized media metadata when present). Audience: clients subscribed to that conversation/team. Source is marked as originating from the webhook.
- Media-processed/message-updated event: emitted for Facebook/Instagram-style messages after asynchronous media retrieval completes, signaling that the message's media is now available; high priority. Audience: subscribers of the conversation.
- Conversation transfer/assignment event: emitted during follow-driven auto-routing, carrying source/target team, target team name, a conversation summary, and reconciliation markers (including the platform user identifier and a flag that this is a webhook-confirmed conversation) so clients can replace an optimistic/pending placeholder.
- Failure alert: on partial batch failure, an audit alert is recorded (deferred) and, when an external alert URL is configured, a message is posted to it (a chat-style formatted message when a chat-alert URL is configured, otherwise a raw JSON body). This never blocks or fails the webhook response.
- Notifications (downstream triggers, not client websocket events per se): new-conversation notifications on first message / new conversation; customer-followed notifications on follow. All are best-effort and tolerate failure.

## LIFF (LINE Front-end Framework) Integration

### Purpose
This area lets a LINE messaging account onboard customers to specific support teams through scannable codes. Each team can have a generated code image; when a LINE user opens the linked LINE front-end mini-page (via scanning) the page records which team the user came through, optimistically surfaces a placeholder conversation to agents in real time, and (for users who are already friends of the messaging account) reconciles or creates the team-assigned conversation and pushes a welcome message. It also provides administrators bulk code generation and coverage reporting, and team managers per-team code generation/retrieval/statistics.

### Operations

#### Onboarding service health probe — GET /api/liff/health
- Invocation: unauthenticated client request (public).
- Inputs: none.
- Preconditions & Authorization: none; public.
- Behavior: returns a static liveness payload immediately.
- Success Output: 200 with `status` ("healthy"), `module` ("liff"), `version` (a semantic-version string), and `timestamp` (ISO-8601 string).
- Error Conditions: none expected.

#### Get front-end mini-page configuration — GET /api/liff/config
- Invocation: unauthenticated client request (public); called by the mini-page to initialize itself.
- Inputs: none.
- Preconditions & Authorization: none; public. Requires the server to have a configured LINE front-end application identifier.
- Behavior: returns the identifiers and settings the mini-page needs to boot.
- Success Output: 200 with `success: true` and a `data` object containing: `liffId` (the LINE front-end application identifier), `lineBotId` (the messaging account handle, e.g. `@xxxx`; falls back to a built-in default handle when not configured), `lineOaId` (the same handle with a leading `@` stripped), `apiEndpoint` (the backend base URL, empty string when unset), `autoCloseDelay` (a fixed number of milliseconds the page waits before auto-closing, currently 2000), and `version` (semantic-version string).
- Error Conditions: when the front-end application identifier is not configured -> 500 with `success: false` and a human-readable `error` message.

#### Get team display info for the mini-page — GET /api/liff/teams/:teamId
- Invocation: unauthenticated client request (public).
- Inputs: path parameter `teamId` (must parse to an integer).
- Preconditions & Authorization: none; public.
- Behavior: looks up the team and returns its public display fields.
- Success Output: 200 with `success: true` and `data` containing `id` (number), `name` (string), `description` (string or null).
- Error Conditions: non-numeric `teamId` -> 400 with `success: false` and a localized `error`; team not found -> 404 with `success: false` and a localized `error`; unexpected failure -> 500 via the global error formatter.

#### Record team assignment from a code scan — POST /api/liff/assign-team
- Invocation: unauthenticated client request (public); called by the mini-page after the visitor's identity is obtained.
- Inputs: JSON body with `lineUserId` (string, required — the platform user identifier), `teamId` (number, required), `displayName` (string, optional), `timestamp` (string, optional — when supplied, becomes the recorded assignment time; otherwise the server's current time is used).
- Preconditions & Authorization: none; public. The referenced team must exist.
- Behavior: validates the required fields, confirms the team exists, then ensures an assignment record links this platform user to this team. If an assignment for that user+team pair already exists, it is treated as success and the existing record is returned unchanged (idempotent). Otherwise a new assignment record is created with source marked as originating from a code scan, capturing the optional display name and request metadata. After a new record is created, the team's scan counter is incremented if a code exists for that team. Independently and non-blockingly, a real-time "conversation transferred" event is broadcast to the destination team describing a synthetic pending conversation so agents see the incoming customer within sub-second latency, before the user actually adds the account as a friend.
- Success Output: 200 with `success: true` and `data` containing `assignmentId` (string identifier), `teamName` (string), and a localized `message` (distinct messages for "already recorded" vs "newly recorded").
- Side Effects: persists a customer-to-team assignment record (conceptual) tagged as code-scan-sourced with stored display name and captured request metadata; increments the per-team scan counter; emits a real-time conversation-transferred event (see Real-time section). The real-time broadcast is best-effort and its failure does not fail the request.
- Error Conditions: missing `lineUserId` or `teamId` -> 400 with localized `error`; team not found -> 404 with localized `error`; unexpected failure -> 500 via the global error formatter.
- Invariants & Guarantees: idempotent per (platform user, team) pair — repeated calls do not create duplicates and do not re-increment the scan counter. Broadcast failure is swallowed (non-blocking).

#### Welcome and reconcile an existing friend — POST /api/liff/welcome
- Invocation: unauthenticated client request (public); called by the mini-page when the visitor is already a friend of the messaging account.
- Inputs: JSON body with `lineUserId` (string, required), `teamId` (number, required).
- Preconditions & Authorization: none; public. Team must exist. Server must have a configured messaging-account push credential.
- Behavior: validates required fields and team existence. Then it reconciles the user's conversation state, best-effort and non-blocking: it locates the existing customer for that platform user on the LINE channel; if found, it looks for that customer's non-terminal conversation. If a non-terminal conversation exists and is assigned to a different team, the conversation's assigned team is updated and a real-time conversation-transferred event is broadcast (old team -> new team). If it exists and is already assigned to the target team, nothing changes. If no non-terminal conversation exists, a new active, normal-priority conversation assigned to the target team is created and a conversation-transferred event (no source team -> target team) is broadcast. If no customer record is found, reconciliation is skipped with a warning. Regardless of reconciliation outcome, the operation then sends a localized welcome text message to the user through the messaging platform push channel.
- Success Output: 200 with `success: true` and `data.message` (localized confirmation that the welcome message was sent).
- Side Effects: may update a conversation's assigned team or create a new conversation (conceptual); emits real-time conversation-transferred events on reassignment or creation; sends an outbound push text message to the user via the external messaging platform.
- Error Conditions: missing `lineUserId` or `teamId` -> 400 with localized `error`; team not found -> 404 with localized `error`; push credential not configured -> 500 with localized `error`; failed outbound push (non-OK upstream response) -> 500 with localized `error`; unexpected failure -> 500 via the global error formatter. Reconciliation failures are caught and logged but do NOT fail the request (the welcome push still proceeds).
- Invariants & Guarantees: reconciliation is best-effort/non-blocking; conversation reassignment only fires when the existing team differs from the target; terminal/closed conversations are excluded from reconciliation.

#### Admin bulk code generation — POST /api/admin/liff-qr/batch-generate
- Invocation: authenticated client request.
- Inputs: none.
- Preconditions & Authorization: requires a valid auth token AND system-administrator role.
- Behavior: determines all active teams, identifies those that do not yet have a front-end code record, and generates a code for each missing one, accumulating per-team success/failure outcomes. If every team already has a code, it returns early with zero totals.
- Success Output: 200 with `success: true`, `timestamp` (ISO string), and `data` containing `total` (count attempted), `success` (count generated), `failed` (count that errored), and `errors` (array of objects each with `teamId`, `teamName`, and an `error` string). The early-exit case additionally carries a `message` and an empty `errors` array.
- Side Effects: for each generated team, persists/updates a per-team front-end code record and stores a code image artifact (see Data Concepts and the generation operation below).
- Error Conditions: missing/invalid auth -> 401; non-admin -> 403; unexpected failure -> 500 via the global error formatter. Individual per-team generation failures are captured in the `errors` array rather than failing the whole call.

#### Admin code coverage status — GET /api/admin/liff-qr/status
- Invocation: authenticated client request.
- Inputs: none.
- Preconditions & Authorization: requires a valid auth token AND system-administrator role.
- Behavior: reports how many active teams have versus lack a front-end code.
- Success Output: 200 with `success: true`, `timestamp`, and `data` containing `totalTeams` (number), `teamsWithLiffQR` (number), `teamsWithoutLiffQR` (number), `coverage` (a percentage string like "83.33%"), and `teams` (array of `{ id, name, hasLiffQR }`).
- Error Conditions: missing/invalid auth -> 401; non-admin -> 403; unexpected failure -> 500 via the global error formatter.

#### Generate or regenerate a team's front-end code — POST /api/teams/:id/qr-code/liff
- Invocation: authenticated client request.
- Inputs: path parameter `id` (validated as an integer).
- Preconditions & Authorization: requires a valid auth token AND manager-or-administrator authority. The team must exist.
- Behavior: looks up the team, then generates (or updates the existing) front-end code. Generation requires the server to have the front-end application identifier and a file-storage capability configured. The produced link encodes the front-end application identifier together with this team identifier so that scanning routes the user to the correct team. The code image is rendered as a scalable vector graphic and made retrievable through a signed URL. If a code record already exists for the team, its link and image URL are refreshed; otherwise a new record is created with scan count zero and active status.
- Success Output: 200 with `success: true` and `data` containing `id` (code record identifier), `liffUrl` (the front-end link), `qrCodeUrl` (a signed URL to the stored image), `scanCount` (0), and `isActive` (true).
- Side Effects: persists/updates the per-team front-end code record; makes a vector image artifact retrievable with descriptive metadata (team identifier, team name, generation timestamp).
- Error Conditions: missing/invalid auth -> 401; insufficient authority -> 403; non-integer id -> 400; team not found -> 404; generation failure (e.g. missing front-end identifier or storage backend) -> 500 with `success: false` and an `error`; unexpected failure -> 500 via the global error formatter.
- Invariants & Guarantees: at most one front-end code record per team — regenerating reuses the same record identifier and resets/refreshes its link and image rather than creating duplicates.

#### Retrieve a team's front-end code — GET /api/teams/:id/qr-code/liff
- Invocation: authenticated client request.
- Inputs: path parameter `id` (validated integer).
- Preconditions & Authorization: requires a valid auth token AND access to the specified team (team-scoped).
- Behavior: returns the stored front-end code record for the team, with the image URL re-signed for retrieval at response time.
- Success Output: 200 with `success: true` and `data` containing `id`, `liffUrl`, `qrCodeUrl` (freshly signed), `scanCount` (number, defaults to 0), `isActive` (boolean), `createdAt`, `updatedAt` (timestamps).
- Error Conditions: missing/invalid auth -> 401; no team access -> 403; non-integer id -> 400; no front-end code exists for the team -> 404 with `success: false` and an `error`; unexpected failure -> 500 via the global error formatter.

#### Front-end code statistics — GET /api/teams/:id/qr-code/liff/stats
- Invocation: authenticated client request.
- Inputs: path parameter `id` (validated integer).
- Preconditions & Authorization: requires a valid auth token AND access to the specified team.
- Behavior: returns scan and assignment counters for the team's front-end code.
- Success Output: 200 with `success: true` and `data` containing `scanCount` (number, defaults to 0), `assignmentCount` (number of team-assignment records for this team), `createdAt` (creation timestamp), `lastScannedAt` (last-updated timestamp of the code record, used as a proxy for last activity), and `isActive` (boolean).
- Error Conditions: missing/invalid auth -> 401; no team access -> 403; non-integer id -> 400; no front-end code exists for the team -> 404 with `success: false` and an `error`; unexpected failure -> 500 via the global error formatter.

#### Deactivate a (legacy) team code — PUT /api/teams/:id/qr-codes/:qrCodeId/deactivate
- Invocation: authenticated client request.
- Inputs: path parameters `id` (validated integer) and `qrCodeId` (non-empty string).
- Preconditions & Authorization: requires a valid auth token AND the highest team-level role (supervisor) for that team.
- Behavior: marks the identified team code as inactive after verifying it belongs to the team. As a downstream effect, the team's quick-access cached code reference is updated to the next most-recent still-active code, or cleared if none remain.
- Success Output: 200 with `success: true`, a `message` ("QR code deactivated successfully"), and `timestamp`.
- Error Conditions: missing/invalid auth -> 401; insufficient team role -> 403; non-integer id -> 400; blank `qrCodeId` -> 400 with `success: false` and an `error`; code not found / not owned by the team -> surfaced as a server error via the global error formatter; unexpected failure -> 500.
- Invariants & Guarantees: ownership is enforced — a code must belong to the named team to be deactivated.

#### Public legacy code redirect/join page — GET /join and GET (qr-code join handler)/join
- Invocation: unauthenticated browser navigation (public); reached by scanning a legacy direct-link code that carries a team reference in a query parameter.
- Inputs: query parameter `team` (a team reference string).
- Preconditions & Authorization: none; public.
- Behavior: resolves the team reference; renders an HTML invitation page. Team-supplied display fields are HTML-escaped before rendering to prevent injection.
- Success Output: 200 HTML page. When the `team` parameter is missing -> an "invalid link" HTML page. When the reference resolves to no team -> an "expired link" HTML page. On unexpected failure -> a generic error HTML page. All cases return HTML (not JSON).

(Note: additional legacy non-front-end code endpoints — per-team code creation, listing, latest/fast retrieval with object-storage-backed images and a cached quick-read of the team's current code reference, and a test-code generator — coexist in this area. They generate codes that may point either at the front-end mini-page link or, as a fallback, a direct add-friend link, and they expose `scanCount`/usage counters; their image artifacts are returned as data URLs or stored-object URLs. These follow the same auth pattern: integer-id validation, team access for reads, supervisor role for creation/deactivation.)

### Data Concepts (neutral)

- **Per-team front-end code record**: at most one per team. Carries a unique record identifier, the owning team identifier, a front-end mini-page link that encodes both the front-end application identifier and the team identifier, a retrievable image URL, a scan counter (starts at 0), an active flag (starts true), and creation/update timestamps. Image URLs are re-signed on read and may be refreshed on regeneration.
- **Code image artifact**: a scalable-vector-graphic image of the front-end link, retrievable through a signed, time-limited URL with descriptive metadata (team identifier, team display name, generation timestamp). High error-correction rendering.
- **Customer-to-team assignment record**: links a platform user identifier to a team, tagged with a source marker indicating it came from a code scan, an optional captured display name, an assignment timestamp, and captured request metadata. Uniqueness is enforced per (platform user, team) pair.
- **Legacy team code record**: an alternate code model carrying a record identifier, owning team, a tracking token, a target link (front-end link or direct add-friend link), an image reference, a usage/scan counter, optional usage cap, optional expiry (defaulting to roughly 30 days out), an active flag, and timestamps. Scan events for these may be recorded separately.
- **Team quick-access code reference**: a denormalized convenience pointer on the team to its current code image, kept loosely in sync (best-effort, non-blocking) with the authoritative code records, and used for fast reads.
- **Conversation (as touched here)**: identified by a unique identifier, tied to a customer, has an assigned team, a status, and a priority; non-terminal conversations are eligible for reconciliation, terminal/closed ones are excluded. Newly created reconciliation conversations are active and normal-priority.

### State & Lifecycle

- **Front-end code**: created active (scan count 0) -> may be regenerated in place (same record identifier, link and image refreshed). No explicit deactivation path is exposed for the front-end code record itself; legacy codes have an explicit active -> inactive transition (supervisor-only), which is terminal for that code and triggers re-pointing or clearing of the team quick-access reference.
- **Customer-to-team assignment**: created once per (user, team) and thereafter treated as immutable/idempotent; re-recording is a no-op returning the existing record.
- **Conversation reconciliation (welcome flow)**: a non-terminal conversation assigned to a different team transitions its assigned team to the target; absence of any non-terminal conversation triggers creation of a new active conversation assigned to the target team; an already-correctly-assigned conversation is left unchanged. Closed/terminal conversations are never reassigned.

### Real-time / Event Behavior

- On a newly recorded code-scan assignment (assign-team), the system emits a real-time conversation-transfer event (named `conversation_transferred`) describing a synthetic pending conversation. The payload identifies a placeholder conversation id, names the destination team (with no source team), and carries conversation details including a placeholder customer name, platform "line", status "pending", a placeholder last-message preview and timestamp, zero unread count, the assigned team id/name, and a metadata block flagging the conversation as pending and including the platform user identifier, the assignment identifier, and the scan time. The actor is reported as a system identity labeled as a code scan, with a reason indicating front-end code pre-assignment. Audience: members of the destination team. Emission is best-effort; failure does not fail the originating request.
- On the welcome (existing-friend) flow, when an existing non-terminal conversation is reassigned to a different team, a `conversation_transferred` event is emitted carrying the real conversation id, the prior team as source and the target team as destination, real customer details (id, display name, platform "line", current status), the new assigned team id, a system actor labeled as a code scan, and a reason indicating existing-friend reassignment. Audience includes both the prior and new team's members.
- On the welcome flow, when a brand-new conversation is created for an existing friend, a `conversation_transferred` event is emitted with the new conversation id, no source team, the target team as destination, real customer details, status "active", a system actor labeled as a code scan, and a reason indicating a new conversation for an existing friend. Audience: the target team's members.
- The welcome flow additionally produces an outbound (non-WebSocket) side effect: a localized text welcome message pushed to the user through the external messaging platform.

## File & Attachment Management

### Purpose
This area lets authenticated staff users upload, list, inspect, download, and delete files (images, video, audio, documents, archives), and lets unauthenticated end-customers and client UIs retrieve stored attachments through signed proxy links. Files are retrievable through stable file references with metadata records; delivery distinguishes between authenticated streaming/buffered downloads and short-lived signed public URLs. There is also a direct upload flow (request a pre-authorized upload target, upload directly, then confirm), and a fallback proxy that fetches and self-heals media originally received from the LINE messaging platform.

All routes below live under a common base path of `/api/files`. There is additionally one standalone public object proxy at `/api/r2-public/:folder/:filename`.

---

### Operations

#### Module health probe — GET /api/files/health
- Invocation: any client request; no authentication required.
- Inputs: none.
- Behavior: reports whether the file area's storage and metadata dependencies are reachable/configured.
- Success Output (200): a success envelope containing a health status label, a module identifier string, a current timestamp, and two booleans indicating file-store availability and database availability.
- Error Conditions: unexpected failures return a generic error envelope.

#### Capability/info description — GET /api/files/info
- Invocation: authenticated client request.
- Preconditions & Authorization: requires a valid auth token.
- Behavior: returns a static, descriptive list of supported features and advertised limits (max file size described as "10MB", a list of broad allowed type families, and the set of supported platform tags: line, facebook, system).
- Success Output (200): a success envelope with a message, a feature-name list, and a limits object.
- Error Conditions: missing/invalid auth -> unauthorized.

#### Upload a file — POST /api/files
- Invocation: authenticated client request with a multipart form body.
- Inputs (multipart form fields):
  - the binary file part (required; rejected if absent or not a file).
  - a platform tag (optional; one of line, facebook, system, admin; defaults to system).
  - an optional conversation reference.
  - an optional message reference.
- Preconditions & Authorization: requires a valid auth token carrying a user identity; otherwise unauthorized.
- Behavior (observable order): the file is validated (size, name, type, content sanity); a unique file identifier and retrievable file reference are produced; a metadata record is persisted; if the file is an image a thumbnail reference may be produced. The acting user is recorded as the uploader. Downloads default to attachment disposition.
- Success Output (201): a success envelope with the new file identifier, the stored filename, byte size, content type, a primary URL, a public URL, an optional thumbnail URL, the derived file-type category, and a creation timestamp.
- Error Conditions:
  - no file part -> 400 "File is required".
  - validation failure -> 400 with `success:false`, a top-level error and/or a list of field-level errors (e.g., file too large, invalid type, invalid filename, corrupted file).
  - file-store/persistence failure -> 400 with an upload-failed message.
- Side Effects: one new persisted file-metadata record; one new object in the store; uploader, optional conversation and message references recorded.
- Invariants: each upload yields a fresh unique identifier (no dedupe).

#### List files — GET /api/files
- Invocation: authenticated client request.
- Inputs (query): page number (default 1), page size (default 20), optional platform tag, optional conversation reference.
- Preconditions & Authorization: requires auth. Scoping: non-administrator callers are restricted to files they uploaded; administrators see all files.
- Behavior: returns a page of file records ordered newest-first.
- Success Output (200): a success envelope containing the items array plus paging fields (total count, current page, page size, has-next, has-previous). Each item carries identifier, filename, content type, size, derived type, a URL, a creation/update timestamp, and basic metadata.
- Error Conditions: missing auth -> unauthorized; server failure -> empty result set rather than an error in some paths.

#### Per-file download / inspect — GET /api/files/:fileId
- Invocation: authenticated client request.
- Inputs: path file identifier (required). Query: a response-mode selector (stream, buffer, or url) and a url-only boolean flag.
- Preconditions & Authorization: requires auth.
- Behavior:
  - If url-only/url mode: returns a success envelope with a URL and optional metadata. The URL is either a freshly generated signed download URL (default ~1 hour validity) or the stored URL.
  - Otherwise: streams the raw bytes back with content type, content length, an attachment content-disposition using the stored filename, and a one-hour cache directive.
- Success Output: 200 (either the JSON URL envelope or the raw byte body).
- Error Conditions: missing identifier -> 400; record not found / download failure -> 404 with `success:false`; no data available -> 400.

#### Delete a file — DELETE /api/files/:fileId
- Invocation: authenticated client request.
- Inputs: path file identifier (required).
- Preconditions & Authorization: requires a valid auth token with a user identity.
- Behavior: removes both the stored object and the metadata record. Object-store deletion failure is tolerated (non-existent objects are treated as already deleted) and the metadata record is still removed.
- Success Output (200): a success envelope confirming deletion and echoing the identifier.
- Error Conditions: missing identifier -> 400; record not found -> 404; other failures -> 400.
- Invariants: deletion is hard (the metadata record is fully removed, not soft-deleted, in this flow); deleting an already-missing object is idempotent-success.

#### Aggregate statistics — GET /api/files/stats/summary
- Invocation: authenticated client request. (Registered ahead of the per-file route so it is not captured by it.)
- Inputs: none (period is fixed to a 30-day window here).
- Preconditions & Authorization: requires auth. Scoping: non-administrators see only their own uploads; administrators see all.
- Behavior: computes counts and sizes over the data set.
- Success Output (200): total file count, total bytes, average bytes per file, breakdown by file-type category, breakdown by platform tag, a storage-usage object (used bytes; available/percentage may be reported as unknown sentinels), and a recent-activity object for the period.
- Error Conditions: failures degrade to a zeroed statistics object.

---

#### Request a direct-upload target — POST /api/files/presigned-url
- Invocation: authenticated client request with a JSON body.
- Inputs (JSON): a filename (required string, max 255 chars), a content type (required; must be in an allowed set covering common image/video/audio/document/text/archive types), a byte size (required positive number, max 10 MB), an optional conversation reference, an optional message reference.
- Preconditions & Authorization: requires auth. The direct-upload subsystem must be configured (credentials present); otherwise the service is reported unavailable.
- Behavior: validates inputs; mints a time-limited authorization for direct upload (validity ~15 minutes); returns an upload target and public URL; persists a metadata record in a "pending" upload state with the acting user recorded as uploader. The filename is sanitized (path/dangerous characters stripped, whitespace collapsed, length-capped) before storage.
- Success Output (200): a success envelope with the upload target URL, the new file identifier, the eventual public URL, an expiry timestamp, and an upload-instructions object describing the HTTP method, required content-type header, and the follow-up confirm call.
- Error Conditions:
  - validation errors -> field-level validation error response (filename required/too long; content type required/unsupported; size required/invalid/over limit).
  - subsystem not configured -> 503.
  - other failures -> 500.
- Side Effects: a "pending" file-metadata record is created.

#### Direct-upload subsystem status — GET /api/files/presigned-url/status
- Invocation: authenticated client request.
- Preconditions & Authorization: requires auth.
- Behavior: reports whether the direct-upload subsystem is configured.
- Success Output (200): a success envelope with a configured boolean, the max byte size and the same value in MB, the list of allowed content types, the upload-URL validity in seconds, and a human-readable status message.

#### Confirm a direct upload — POST /api/files/:fileId/confirm
- Invocation: authenticated client request with a JSON body, issued after the client has completed the direct upload.
- Inputs: path file identifier (required); JSON body with a byte size (required positive number) and an optional checksum.
- Preconditions & Authorization: requires auth.
- Behavior: looks up the pending metadata record; if already confirmed, returns the existing record unchanged. Otherwise, when the subsystem is configured, it verifies the object actually exists in the store (a reported size mismatch is logged but not fatal), then transitions the record to a "completed" state recording the confirmed size. If verification finds no object, the record is transitioned to a "failed" state and an error is returned.
- Success Output (200): a success envelope with the file record fields (identifier, filename, URL, public URL, size, content type) plus a confirmed flag.
- Error Conditions: missing identifier or invalid/missing size -> validation error; record not found -> 404; verification failure -> error (object missing in store); other -> 500.
- Side Effects: the pending record's upload state advances to completed or failed; size is updated.
- Invariants: confirming an already-completed record is idempotent.

#### Direct-upload record status — GET /api/files/:fileId/status
- Invocation: authenticated client request.
- Inputs: path file identifier (required).
- Preconditions & Authorization: requires auth.
- Behavior: returns the current state of a (typically pending) upload record.
- Success Output (200): identifier, filename, content type, size, upload state, URL, creation timestamp, update timestamp.
- Error Conditions: missing identifier -> validation error; no such record -> 404; other -> 500.

---

#### Public proxy by storage path — GET /api/files/public/*
- Invocation: any client (no auth token); designed for end-customers and client UIs.
- Inputs: the remainder of the path identifies the stored object location. Query: a signature value and an expiry value (both required for access).
- Preconditions & Authorization: no auth token, BUT the URL must carry a valid signature. The signature is verified against the storage location and expiry; expired or mismatched/absent signatures are rejected.
- Behavior: validates the signature, fetches the object, and streams it back with the object's stored content type and content disposition, a 24-hour cache directive, and permissive cross-origin headers.
- Success Output (200): the raw object bytes with content headers.
- Error Conditions: empty path -> 400; invalid/expired/missing signature -> 404 (deliberately "not found", not "unauthorized", to avoid revealing whether a given location exists); file store unconfigured -> 500; object absent -> 404.
- Invariants: access is gated solely by a valid time-limited signature bound to the exact storage location.
- A cross-origin preflight (OPTIONS) on this path returns 204 with permissive CORS headers.

#### Public download by attachment identifier — GET /api/files/download/:attachmentId
- Invocation: any client (no auth token). This is the canonical "force download" link for attachments.
- Inputs: path attachment identifier (required). Query: signature value and expiry value (both required).
- Preconditions & Authorization: no auth token; the URL signature must be valid. The signature is bound to the resolved storage location of the attachment (not to the identifier), limiting the blast radius of any leaked link to one file.
- Behavior: resolves the attachment's stored location from its metadata record, verifies the signature against that location, fetches the object, and streams it back forced as an attachment. The download filename is taken from the stored filename and, if it lacks an extension, an extension is appended based on the content type (so older records without an extension still download as openable files). Includes a 24-hour cache directive and permissive cross-origin headers.
- Success Output (200): the raw object bytes with an attachment content-disposition.
- Error Conditions: missing identifier -> 400; no matching metadata record -> 404; metadata record has no storage location -> 404; invalid/expired/missing signature -> 404; file store unconfigured -> 500; object absent in store -> 404.
- A cross-origin preflight (OPTIONS) returns 204 with permissive CORS headers.

#### LINE media fallback proxy — GET /api/files/line-proxy/:lineMessageId
- Invocation: any client (no auth token); used by client UIs to display inbound LINE media.
- Inputs: path LINE message identifier (required; must be all digits).
- Preconditions & Authorization: no auth; no signature required for this route.
- Behavior (observable order): (1) fast path — if a stored copy of this LINE message's media already exists, it is streamed from the file store; (2) fallback — otherwise the media is fetched live from the LINE content API using the configured channel token; on success the bytes are returned to the caller, and in the background the system self-heals by downloading and persisting the media to the file store and creating an attachment metadata record (if one does not already exist) so future requests use the fast path.
- Success Output (200): the raw media bytes with content type, content length, a 24-hour cache directive, and permissive cross-origin headers.
- Error Conditions: non-numeric identifier -> 400; LINE token not configured / file store not configured -> 500; upstream LINE content unavailable -> 404 if upstream reported not-found, otherwise a bad-gateway status; other -> generic error.
- Invariants: the background self-heal is best-effort and non-fatal; duplicate attachment records are avoided.
- A cross-origin preflight (OPTIONS) returns 204 with permissive CORS headers.

#### Public object proxy with CORS — GET /api/r2-public/:folder/:filename
- Invocation: any client (no auth token); used for things like backend-generated QR-code asset links.
- Inputs: path folder segment and filename segment (combined into a storage location). Query: signature value and expiry value (both required).
- Preconditions & Authorization: no auth; URL signature must be valid (verified against the combined storage location).
- Behavior: verifies signature, fetches the object, returns it with the stored content type, an entity tag, a long (1-year) cache directive, and origin-checked CORS headers (only allow-listed origins receive cross-origin headers).
- Success Output (200): raw bytes with content headers.
- Error Conditions: invalid/expired/missing signature -> 404; object absent -> 404; failure -> 500.
- A preflight (OPTIONS) returns 204 with origin-checked CORS headers.

---

The following capabilities exist in the module's richer route definitions (multi-file, chunked, platform-specific, conversation/message-scoped, search, batch). They follow the same auth + validation + rate-limiting contract described below.

#### Upload multiple files (batch) — POST (multi-file upload route)
- Inputs: multiple binary file parts under a repeated field; shared optional platform tag, conversation reference, message reference. Hard cap of 10 files per request.
- Behavior: each file is uploaded independently and in parallel; partial success is allowed.
- Success Output: a success envelope with a list of successful results, a list of failed entries (filename + error), and a summary (total, successful count, failed count).
- Error Conditions: no files -> validation error; over the per-request file cap -> validation error.

#### Platform-targeted single upload — POST (line / facebook / admin upload routes)
- Same as single upload, but validation rules and rate limits are pre-bound to the named platform. The admin variant allows a larger max size (advertised 50 MB) and higher rate limits and accepts multiple files.

#### Conversation-scoped and message-scoped helpers
- Retrieve files attached to a given conversation reference, with paging and optional type filter.
- Retrieve files attached to a given message reference (returns up to a fixed cap).
- Upload a file directly into a conversation or message context.

#### Search files — GET (search route)
- Inputs: a required query string plus paging and optional platform/type/date filters.
- Behavior: returns files whose stored or original filename contains the query (case-insensitive). Missing query -> validation error.

#### Batch operation — POST (batch route)
- Inputs (JSON): an operation name (required) and a non-empty array of file identifiers (required). The supported operation is delete; unknown operations are reported as failures per-item.
- Authorization: requires a valid user identity, otherwise forbidden.
- Success Output: per-item successful/failed lists plus a summary including total processing time.

#### Get download URL — GET (download-url route)
- Inputs: path identifier; optional validity-in-seconds (default 3600).
- Behavior: returns a freshly generated signed download URL plus the computed expiry timestamp.

#### Chunked upload lifecycle — init / upload-chunk / complete / cancel routes
- A four-step large-file flow: initialize (validates name/size/type and returns an upload-session identifier, a chunk size of 1 MB, the total chunk count, and a 24-hour session expiry), upload an individual chunk, complete, and cancel. These return success acknowledgements; within the current behavioral boundary the uploaded chunks are not durably persisted, and the completion step returns a synthesized record rather than a stored object.

---

### Cross-cutting validation, limits, and authorization (applies to authenticated upload routes)

- Authentication: every authenticated route requires a valid token; upload routes additionally require the token to carry a user identity, returning 401 otherwise.
- Content type: upload routes require a multipart form body; non-multipart requests are rejected with 400.
- Size limits (bytes): global max 10 MB; image max 5 MB; video max 20 MB; audio max 10 MB; document max 10 MB; minimum 1 byte. The admin context advertises a 50 MB max. A file whose declared size does not match its actual byte length is reported as corrupted.
- Allowed types: a curated allow-list of image, video, audio, document, and archive content types. Each platform context permits a different subset (LINE: image/video/audio; Facebook: adds documents; System: adds archives; Admin: all). Disallowed types are rejected.
- Filename rules: required; max 255 characters; must not contain path/control/reserved characters; reserved device names are rejected.
- Prohibited extensions: a security block-list of executable/script extensions is always rejected.
- Content-signature checks (on the strict content middleware and on validation): the leading bytes of the file are checked against the declared content type for common formats (image, document, archive, video, audio, text); unknown signatures fail closed; an empty file is rejected.
- Rate limiting (per acting user, in-process counters): caps on concurrent uploads, uploads per minute, uploads per hour, total bytes per hour, and files per request; exceeding any cap returns 429 with a descriptive message. Defaults vary per route; the admin route has the highest caps. Concurrency counters are released after each request completes.
- Upload timeout: a per-request timeout (about 30 seconds) returns 408 if exceeded.
- File-identifier format validation: per-file routes reject identifiers that are neither a UUID-shaped value nor a simple alphanumeric/dash/underscore token, with 400.

### Data Concepts (neutral)
- File/Attachment record: carries a unique file identifier, the (sanitized) filename, the original filename, content type, byte size, derived extension, a derived type category (image / video / audio / document / archive / other), a primary access URL, an optional public URL, an internal storage-location key, the owning platform tag, optional conversation reference, optional message reference, the uploader's user identity, a processing/upload status, and creation/update timestamps.
- Upload status (for the direct-upload flow): pending -> completed, or pending -> failed.
- Derived storage location: organized conceptually by a prefix, platform, type, a date hierarchy, optional conversation/user qualifiers, and a uniquely generated leaf name; the original filename is normally not preserved in the leaf.
- File-type categorization is inferred from content type and/or extension.
- Statistics aggregate: counts and sizes grouped by type and platform, plus recent-activity counters over a chosen window.

### State & Lifecycle
- Standard upload: an upload immediately produces a completed metadata record and a stored object.
- Direct-upload: request-target (creates pending record) -> client uploads directly -> confirm (verifies object presence; pending -> completed, recording confirmed size) or, on verification failure, pending -> failed. Confirming an already-completed record is a no-op success.
- Expired pending records: pending records older than a configurable age (default ~30 minutes) are eligible for cleanup/removal by a maintenance routine.
- Deletion: removes both stored object and metadata record (hard delete in this flow); deleting an absent object is treated as success.

### Real-time / Event Behavior
- This area emits no WebSocket/real-time events itself.
- It performs one downstream/background side effect: the LINE media fallback proxy, after serving live-fetched media, asynchronously stores the media to the file store and creates an attachment metadata record so subsequent reads hit the stored copy. This self-heal is best-effort and does not affect the response to the caller.
- Access to stored content for unauthenticated audiences is mediated entirely by time-limited URL signatures bound to the exact storage location; signatures expire (default validity on the order of a day for proxy links) and invalid/expired signatures return "not found" to avoid disclosing object existence.


---

# 5. Real-time Infrastructure

## WebSocket Gateway & Protocol

### Purpose
This area provides the real-time messaging backbone for the support platform. Authenticated staff clients open a persistent bidirectional connection, are routed to either a per-conversation real-time room or a per-user personal channel, and thereafter exchange live events (new messages, typing indicators, presence, conversation assignment/transfer, team and tag changes, notifications). It also covers the supporting non-realtime HTTP surface: the connection handshake gate, a disconnect/cleanup call, feature-flag/migration configuration, health and readiness probes, an operational metrics/dashboard set, and an error/quality analytics service. The gateway enforces authentication and team-based authorization, fans out events to the correct audience, and provides reconnection synchronization so a client can recover messages missed while disconnected.

### Operations

#### Open a real-time connection — GET /api/websocket/connect
- Invocation: Authenticated client opening a WebSocket. The request must be a protocol-upgrade request (the upgrade header must indicate websocket); otherwise it is rejected.
- Inputs (all as query parameters):
  - `token` (string, required): a signed bearer credential identifying the caller. Must be a three-segment dotted token.
  - `conversationId` (string, optional): when present, the connection targets a specific conversation room; when absent, it targets the caller's personal channel.
  - `deviceId` (string, optional): a client device label, used for multi-device tracking.
- Preconditions & Authorization:
  - The credential must be present, well-formed (three dotted segments), valid, and not expired. A small expiry safety margin is enforced (see Error Conditions).
  - The credential must carry a usable account identifier (non-empty, non-zero) and a role of either administrator or agent. Any other role is rejected.
  - The real-time feature must be enabled by current configuration; otherwise the connection is refused as unavailable.
  - If a `conversationId` is supplied and the caller is an agent (not an administrator), the caller must be authorized for that conversation. Authorization holds when the conversation is assigned to one of the caller's teams or is currently unassigned (in the shared pool). Administrators are authorized for every conversation.
  - Per-account and global connection ceilings are enforced before accepting (see Invariants).
- Behavior: On success, the connection is accepted into either the requested conversation stream or the caller's personal stream. The target stream completes the upgrade and immediately sends a connection-established welcome event (see Real-time / Event Behavior). The connection is also scheduled to be force-closed automatically at the moment the credential would expire.
- Success Output: A protocol-switch response (status 101) establishing the live socket. No JSON body.
- Side Effects: A live connection becomes active; the account is added to the conversation's participant set or marked online for personal delivery. On first connection of a previously-offline account, that account becomes reachable for global/team fan-out. Presence/participant join events may be emitted to other participants.
- Error Conditions:
  - Missing credential -> HTTP 401, machine code 4401, error label indicating no token; a companion header advertises a websocket close code 4401.
  - Malformed credential (not three segments) -> HTTP 401, code 4402.
  - Invalid or unverifiable credential -> HTTP 401, code 4403.
  - Already-expired credential -> HTTP 401, code 4404 (response echoes expiry and current time).
  - Credential expiring within the safety margin (under ~30 seconds) -> HTTP 401, code 4405 (response includes remaining seconds). Credentials expiring within ~5 minutes are allowed but flagged.
  - Missing/zero account identifier -> HTTP 401, code 4406.
  - Disallowed role -> HTTP 401, code 4407 (response lists the allowed roles).
  - Agent lacks access to the requested conversation -> HTTP 403, code 4403, error label indicating conversation access denied.
  - Real-time feature disabled -> service-unavailable status.
  - Not an upgrade request -> bad-request status.
  - Per-account or global connection ceiling reached -> too-many-requests status.
  - Unexpected gate failure -> HTTP 500, code 4500. Each error response carries machine-readable headers indicating an error code and a suggested websocket close code, plus a suggested recovery action (provide token / refresh token / contact admin / retry with new token).
- Invariants & Guarantees: The credential is checked once at handshake; a separately scheduled forced close enforces expiry on the live socket thereafter. Authorization results for an agent's accessible conversations are cached briefly (about 5 minutes) and invalidated when assignments change.

#### Request a connection challenge (alternate handshake) — issued by the conversation room before upgrade
- Invocation: Authenticated HTTP call to the conversation room's challenge endpoint (full-feature rooms only), carrying a bearer credential in the authorization header.
- Inputs: Bearer credential in the authorization header.
- Preconditions & Authorization: The credential must be present, properly prefixed, valid, and carry an account identifier.
- Behavior: Issues a short-lived single-use challenge bound to the caller and credential. The client later completes the upgrade by presenting the challenge identifier plus a signature instead of the raw credential.
- Success Output: A challenge identifier, its absolute expiry time, and the challenge lifetime.
- Side Effects: The challenge is recorded (with its bound account, role, and credential) for later verification; expired challenges are purged.
- Error Conditions: Missing/badly-formed authorization -> unauthorized. Invalid/expired credential -> unauthorized. Server failure -> server error.
- Invariants & Guarantees: A challenge is valid for about 30 seconds, is single-use (consumed on successful verification), and is verified by comparing a keyed signature of the challenge identifier and the bound credential. There is also a token-only handshake path: presenting a valid credential directly is accepted and is the path the standard client uses.

#### Close / clean up a connection — POST /api/websocket/disconnect
- Invocation: Authenticated client request.
- Inputs (JSON body): `connectionId` (string), `reason` (string, optional).
- Preconditions & Authorization: Same authenticated handshake gate as the connect operation (valid credential, allowed role).
- Behavior: Removes the named connection from the caller's personal delivery stream and updates account reachability. If concurrent cleanup attempts target the same connection, only one proceeds at a time; overload fails fast instead of waiting indefinitely.
- Success Output: Confirmation containing the connection identifier and a disconnect timestamp.
- Side Effects: Connection state is removed; the account stops being reachable for personal/global delivery when no other connections remain; an offline/presence transition may follow.
- Error Conditions: Handshake-gate failures as above; server failures yield a server error.
- Invariants & Guarantees: Cleanup is best-effort and mutually exclusive per target; a cleanup timeout does not leave later cleanup attempts blocked.

#### Read gateway feature/migration status — GET /api/websocket/migration-status
- Invocation: Public (no authentication).
- Behavior: Returns whether the real-time feature is enabled, the rollout percentage, whether the underlying realtime infrastructure is available, and the set of feature flags.
- Success Output: Status object with enabled flag, rollout percentage, infrastructure-availability flag, feature flags, and a timestamp.

#### Update feature/migration configuration — POST /api/websocket/migration-config
- Invocation: Authenticated client request.
- Inputs (JSON body): A partial configuration: enable flag, strategy label, rollout percentage, and feature flags.
- Preconditions & Authorization: Administrator role required.
- Behavior: Merges the supplied fields over current configuration and persists them.
- Success Output: The effective configuration after the update, plus who updated it and a timestamp.
- Error Conditions: Non-administrator -> forbidden. Rollout percentage outside 0–100 -> validation failure.
- Side Effects: Persists the new gateway configuration, affecting subsequent handshake and broadcast decisions.

#### Basic connection health — GET /api/websocket/health
- Invocation: Public.
- Behavior: Reports aggregate connection counts, average latency, and an error rate, and derives an overall status (healthy / degraded above ~10% error / unhealthy above ~25% error).
- Success Output: Health object with status, enabled flag, total and active connection counts, connection-by-type breakdown, average latency, error rate, timestamp. Status code is 200 when healthy, a multi-status code when degraded, and service-unavailable when unhealthy.

#### Comprehensive component health — GET /api/websocket/health
- Invocation: Authenticated client request (this richer health endpoint is protected). 
- Behavior: Checks the realtime infrastructure, the realtime feature, the key-value store, and the database; computes an overall status from component statuses.
- Success Output: Per-component health (status, message, last-check time), configuration summary (enabled, rollout), and basic metrics. Status code is 200 when healthy or degraded, service-unavailable when unhealthy.

#### Readiness probe — GET /api/websocket/readiness
- Invocation: Authenticated client request.
- Behavior: Reports ready only when, given the feature is enabled, the realtime infrastructure is available and the key-value store is reachable.
- Success Output: A ready flag; when not ready, a reason; service-unavailable status on failure.

#### Liveness probe — GET /api/websocket/liveness
- Invocation: Authenticated client request.
- Behavior: Returns an alive flag and timestamp without dependency checks.

#### Aggregate gateway metrics — GET /api/websocket/metrics
- Invocation: Authenticated client request.
- Behavior: Returns feature/rollout info, realtime-infrastructure availability flags, live connection metrics, mutual-exclusion metrics, and headline performance figures (latency percentiles, throughput, reliability, error rate). Some performance and instance-count figures are reported as fixed representative values within the current behavioral boundary rather than live measurements.
- Success Output: A wrapper with a status field and a data object containing the above sections.

#### Detailed component health — GET /api/websocket/health-detail
- Invocation: Authenticated client request.
- Behavior: Probes each realtime component and each infrastructure dependency individually, returning per-component availability, response time, and an overall health score and status.

#### Architecture comparison — GET /api/websocket/comparison
- Invocation: Authenticated client request.
- Behavior: Returns a static comparison of the current real-time architecture versus a deprecated polling architecture (latency, throughput, reliability, error rate, cost, features) for reporting purposes.

#### Operational dashboard: real-time metrics — GET /api/websocket/dashboard/metrics
- Invocation: Authenticated client request.
- Preconditions & Authorization: Administrator role required.
- Behavior: Returns near-real-time connection counts (by conversation, by account, by protocol), throughput, infrastructure health, latency percentiles, and resource usage, cached briefly.
- Error Conditions: Non-administrator -> forbidden.

#### Operational dashboard: active connections — GET /api/websocket/dashboard/connections
- Authorization: Administrator role required.
- Behavior: Returns the list of currently tracked connections and a count.

#### Operational dashboard: connection history — GET /api/websocket/dashboard/history
- Inputs: `period` query parameter (defaults to a 24-hour window).
- Authorization: Administrator role required.
- Behavior: Returns historical connection data points for the requested window.

#### Operational dashboard: performance trends — GET /api/websocket/dashboard/trends
- Inputs: `period` query parameter (default 24-hour window).
- Authorization: Administrator role required.
- Behavior: Returns trend data points and a summary (peak, averages, incident count).

#### Operational dashboard: infrastructure status — GET /api/websocket/dashboard/durable-objects
- Authorization: Administrator role required.
- Behavior: Returns availability of each realtime infrastructure dependency.

#### Operational dashboard: active alerts — GET /api/websocket/dashboard/alerts
- Authorization: Administrator role required.
- Behavior: Returns currently active operational alerts and a count.

#### Analytics: dashboard data — GET /api/websocket/analytics/dashboard
- Invocation: Authenticated client request.
- Authorization: Administrator role required.
- Behavior: Returns aggregated analytics dashboard data.
- Error Conditions: Non-administrator -> forbidden.

#### Analytics: trend analysis — GET /api/websocket/analytics/trends
- Inputs: `timeRange` query parameter in hours (defaults to 24; must be between 1 and 168).
- Authorization: Administrator role required.
- Behavior: Returns trend analysis over the requested window.
- Error Conditions: Out-of-range time window -> bad request; non-administrator -> forbidden.

#### Analytics: record error — POST /api/websocket/analytics/errors
- Invocation: Trusted system request (no authentication required on this endpoint).
- Inputs (JSON body): an error record requiring a timestamp, an error code, and an error type.
- Behavior: Persists the error record for trend analysis.
- Success Output: Confirmation with a derived error identifier.
- Error Conditions: Missing required fields -> bad request.

#### Analytics: record connection quality — POST /api/websocket/analytics/quality
- Invocation: Trusted system request (no authentication required).
- Inputs (JSON body): a quality record requiring a timestamp, an account identifier, and a connection identifier.
- Behavior: Persists the quality sample.
- Error Conditions: Missing required fields -> bad request.

#### Analytics: trigger alert — POST /api/websocket/analytics/alerts/trigger
- Authorization: Administrator role required.
- Inputs (JSON body): an alert level, title, and description. Level must be one of: informational, warning, critical, emergency.
- Behavior: Raises an operational alert tagged with the triggering account.
- Error Conditions: Missing fields or invalid level -> bad request; non-administrator -> forbidden.

#### Analytics: system self-check — GET /api/websocket/analytics/health
- Authorization: Administrator role required.
- Behavior: Probes analytics sub-systems (storage, trend generation, alerting) and returns an overall status and score.

#### Analytics: get alert configuration — GET /api/websocket/analytics/config/alerts
- Authorization: Administrator role required.
- Behavior: Returns the current alert thresholds, or defaults when unset, with a flag indicating whether defaults are in effect.

#### Analytics: update alert configuration — PUT /api/websocket/analytics/config/alerts
- Authorization: Administrator role required.
- Inputs (JSON body): error-rate threshold, latency threshold, connection-failure threshold, satisfaction threshold, and a time-window value (all required).
- Behavior: Validates ranges (error rate within 0–1; latency within 0–30000) and persists the configuration.
- Error Conditions: Missing fields or out-of-range values -> bad request; non-administrator -> forbidden.

#### Analytics: export trends — GET /api/websocket/analytics/export/trends
- Authorization: Administrator role required.
- Inputs: `format` (defaults to a structured object; a tabular export is offered as an alternative) and `timeRange` in hours (default 24).
- Behavior: Returns trend data either as a structured object or as a downloadable tabular file.

#### Connectivity self-test — GET /api/websocket/test-connection
- Invocation: Public (no authentication).
- Inputs: `userId` (required) and `conversationId` (optional).
- Behavior: Returns the status snapshots of the relevant realtime components for the given account and conversation, plus current configuration. Intended for diagnostics.
- Error Conditions: Missing account identifier -> bad request.

### Inbound client message protocol (over the live socket)
Once connected, the client sends JSON frames, each carrying a type discriminator and an optional payload, optional message identifier, optional target conversation identifier, and optional timestamp. Frames that cannot be parsed produce an error frame back to the sender. Supported inbound types:
- Keepalive: a ping frame is answered with a pong frame echoing a timestamp.
- Subscribe / unsubscribe (personal channel): subscribe to or unsubscribe from a conversation's notifications. Subscribe is permission-checked (the account must be allowed to view the conversation); on success the channel records the subscription and acknowledges; if the per-account subscription ceiling is reached the client receives an error. Unsubscribe always succeeds and is acknowledged. (In conversation rooms these frames are accepted but are effectively no-ops because membership is implicit in being connected.)
- Chat message: in a conversation room, this is permission-checked, assigned a monotonically increasing in-room order number, optionally appended to the room's short message history, and broadcast as a message-sent event to all participants of that conversation (regardless of which instance serves their connection). A frame flagged as a typing indicator is instead relayed only to other participants without storage. On the personal channel, a chat frame requires a target conversation the account is subscribed to and is simply acknowledged back to the sender.
- Event: typing-start / typing-stop frames are relayed to other participants/connections; other event types are acknowledged.
- Sync request (conversation room, reconnection recovery): the client sends a since-timestamp; the room replies with a sync-response frame containing the messages received after that time, a missed count, the sync time, and the server's latest-message timestamp.
- Any other type yields an error frame naming the unknown type.

Inbound limits (personal channel): each connection is rate-limited (about 10 frames per second); exceeding it produces an error frame and the frame is dropped. Frames larger than a fixed maximum (about 10 KB) are rejected with an error frame.

### Data Concepts (neutral)
- Connection record: identifies a single live socket — a connection identifier, the owning account identifier, role (administrator or agent), optionally the bound conversation, last-activity time, active flag, and optional metadata (device label, user agent, client IP, connect time).
- Conversation room: the live stream for one conversation. Tracks its live connections, its participant account set, a running in-room message-order counter, last-activity time, an optional bounded recent-message history (used for reconnection sync, roughly the last several dozen messages), and an optional set of outstanding challenges. Rooms operate in a full-feature mode (history, advanced auth, permission checks) or a simplified mode (minimal, query-parameter auth only).
- Personal channel: the live stream for one account across conversations. Tracks the account's live connections (multi-device), its set of subscribed conversations, presence/online state, last-seen time, per-account preferences, and usage counters.
- Connection challenge: a short-lived, single-use authentication token bound to an account, role, and credential, with an expiry around 30 seconds.
- Broadcast event: a unit of fan-out carrying a unique event identifier, an event type, an origin label (e.g. originating from a client socket, an internal API action, a queue, or a webhook), a timestamp, optional account and conversation references, a payload, a priority (low / normal / high / urgent), and delivery options (the target audiences, a persistence flag, and a time-to-live). Targets can be: a conversation audience, a specific-account audience, a team audience, or a global audience optionally filtered by role.
- Subscription record: a binding of an account to a conversation it wishes to receive notifications about (personal channel), subject to a per-account ceiling (about 50).
All persisted realtime state (connections, participants, subscriptions, recent history, challenges) is keyed by the relevant identifier and is removed on disconnect/expiry rather than retained indefinitely; recent-message history is bounded and rolls over.

### State & Lifecycle
- Connection lifecycle: pending -> connected (after handshake validation and socket acceptance) -> closed. A connection is force-closed automatically when the originating credential reaches its expiry (the client receives close code 4401, signaling it should refresh and reconnect). Connections idle past an inactivity timeout (around 5 minutes) are reaped. Closing a socket removes it; when an account's last connection closes, the account transitions to offline and is made unreachable for global broadcast delivery.
- Presence lifecycle: an account becomes online on its first connection and offline when its last connection ends; presence transitions can be broadcast to the account's team(s) and to administrators.
- Reconnection sync: a reconnecting client compares the server's reported latest-message time (delivered in the welcome event) with its own last-seen time and, if behind, requests a sync; the room returns only the messages after the supplied time.
- Challenge lifecycle: issued -> (verified and consumed) or (expired and purged). Single-use and short-lived.
- Connection-state machine published to clients (informational labels): disconnected, connecting, connected, reconnecting, error.

### Real-time / Event Behavior
The defined event taxonomy (lowercase identifiers) includes: new message; user connected; user disconnected; typing started; typing stopped; message read; message recalled; connection-state change; heartbeat; and error. The broadcast layer additionally emits richer domain events described below.

Welcome events on connect:
- Conversation room: a connection-established event containing the conversation identifier, the connection identifier, the current participant list, the room mode, and the server's latest-message timestamp (for sync).
- Personal channel: a user-connected event containing the account identifier, the connection identifier, the account's current subscriptions, preferences, and usage stats.

Participant/presence events:
- When an account joins a room, a user-joined event with the account, connection, role, and participant count is broadcast to room participants. When an account's last connection to a room leaves, a user-left event with the updated participant count is broadcast.
- Presence events (online / offline / away / available / busy) are broadcast to the account's team (when known) and to administrators.

Message events:
- A new-message broadcast is delivered to two audiences: the conversation's real-time detail view, and the conversation-list view of the relevant team members plus administrators. When a team scope is supplied, fan-out is restricted to that team and administrators; when no team scope is supplied, a global fan-out is used (and flagged as a less-secure fallback). The payload carries the message identity, content, message type, sender type (customer or agent), sender identity and name, platform, timestamp, delivery status, optional serialized media metadata, and optional file-attachment descriptors.
- Message-state events (sent, delivered, read, recall-succeeded, recall-failed, updated) are broadcast to the conversation audience.
- Typing indicators are broadcast to the conversation audience (and relayed among a room's other participants directly).
- Delayed-message events (countdown, sent, recalled, failed) are broadcast to both the conversation audience and the originating agent's personal channel; the countdown variant is transient while the terminal variants are marked persistent with a longer lifetime.

Conversation lifecycle and assignment events:
- Assignment, unassignment, transfer, status change, tags update, customer-profile update, and participant join/leave are broadcast to the conversation audience; assignment- and transfer-related events also reach administrators and team audiences.
- A conversation transfer performs a three-part notification: the previous owning team is notified of removal, the receiving team is notified of assignment (carrying the conversation card data including customer identity, platform identity, avatar, status, last message, unread count, and the new team), and the conversation room is notified of the team change. These three fan-outs run concurrently and each reports success independently. Removal notices are transient; assignment notices are persistent.

System, team, tag, and notification events:
- Notification events are delivered to a specific account's personal channel; urgent/high notifications carry elevated priority.
- Team-member add/remove and team-update events are delivered to administrators and to the affected team.
- Customer-tag add/remove/set events are delivered to administrators and agents.

Audience routing and delivery semantics:
- Conversation-targeted events reach the relevant conversation audience; account-targeted events reach that account; team- and global-targeted events reach the matching teams/administrators (optionally including administrators alongside a team). Global fan-out can be filtered by role.
- Non-urgent events may be coalesced into short time-window batches (window on the order of a few hundred milliseconds, with a maximum batch size and a hard queue cap) to reduce triggers; urgent-priority events bypass batching and are sent immediately.
- The broadcast path is protected by a circuit breaker: when downstream error volume crosses a threshold the breaker opens and events are diverted to the fallback batch queue rather than attempted live; it recovers automatically after a cooldown.
- A broadcast is reported successful if at least one targeted audience accepts it. Cross-instance delivery for very large rooms is fire-and-forget and never blocks local delivery. The real-time feature being disabled, or the realtime-messaging flag being off, causes broadcasts to be suppressed.

## Conversation Room & Message Broadcast Delivery

### Purpose
This area defines the observable real-time behavior for conversation-scoped rooms and routed event delivery. A conversation room accepts live WebSocket connections for one conversation, delivers messages to participants in a single ordered stream, reports presence, supports typing indicators, and lets a reconnecting client recover messages that are still within the recent-history window. Routed event delivery accepts server-originated events and delivers them to the requested audience: specific conversations, specific users, members of teams, all administrators, or everyone, with priority, batching, retry, overflow, and failure-reporting behavior described below.

---

### Operations

#### Establish a live connection to a conversation room — `GET /?conversationId=<id>` with WebSocket upgrade
- Invocation: A client opens a WebSocket against the conversation room instance (the upgrade is detected by the standard upgrade request header). The room operates in one of two modes set at creation time: a full-featured mode and a reduced/simplified mode for high-traffic rooms.
- Inputs (query parameters):
  - Conversation identifier — string, used to bind the socket to a conversation; if omitted the room keeps its existing/“unknown” identifier.
  - Authentication, full mode (one of two methods required):
    - A bearer credential token — string; OR
    - A challenge identifier plus a signature — both strings (legacy challenge-response method).
  - Authentication, simplified mode: a user identifier, a token, and a role — all three required strings; the role string is taken at face value (no cryptographic verification of the token in this mode).
  - Optional token expiry — a numeric epoch-seconds value. When present and positive, the server schedules an automatic close of the socket at that moment.
- Preconditions & Authorization:
  - Full mode with a token: the token must be a valid, verifiable credential; the authenticated identity and role are taken from it.
  - Full mode with challenge-response: the referenced challenge must exist, be unexpired, and the supplied signature must match the expected signature for that challenge; the challenge is single-use and is consumed on success.
  - Simplified mode: only requires that the three parameters are present.
  - The room must not already be at its maximum connection capacity (default cap is 100 connections per room).
- Behavior (observable order):
  1. Authenticate per the mode; on failure return an HTTP error before any socket is created.
  2. Reject if at capacity.
  3. Create the server side of a WebSocket pair, assign the connection a unique connection identifier, mark it as an active connection, and add the user to the participant set.
  4. Persist the connection record and the updated participant list.
  5. Emit a “user joined” event to all current connections (see Real-time Behavior). This is suppressed only by virtue of being broadcast to everyone including the joiner.
  6. Immediately send the new socket a “connection established” welcome event containing the conversation identifier, the assigned connection identifier, the current participant list, the room mode, and the server’s last-known message timestamp (used by the client to decide whether to request a sync). In full mode the last-message timestamp reflects stored history; in simplified mode it is null.
  7. If a valid future token-expiry was provided, schedule a forced close at that time with a specific close code and reason indicating token expiry.
- Success Output: an HTTP 101 switching-protocols response that returns the client side of the WebSocket pair; all further interaction is via WebSocket frames.
- Side Effects: a persisted per-connection record and a persisted participant list; a broadcast “user joined” event; a delivered “connection established” welcome event.
- Error Conditions:
  - Missing both auth methods in full mode -> HTTP 400 with a message that either a token or challenge+signature is required.
  - Invalid token (full mode) -> HTTP 401 (“invalid token”).
  - Invalid/expired/mismatched challenge response (full mode) -> HTTP 401 (“invalid challenge response”).
  - Missing user identifier, token, or role (simplified mode) -> HTTP 400.
  - Room at capacity -> HTTP 429 (“connection limit reached”).
  - Any unexpected failure during upgrade -> HTTP 500.
- Invariants & Guarantees: each conversation has one canonical ordering stream. A forced expiry close fires regardless of socket activity. Connection capacity is enforced strictly before acceptance.

#### Generate an authentication challenge — `POST /challenge` (full mode only)
- Invocation: Authenticated HTTP request made before opening a challenge-response WebSocket.
- Inputs: a bearer credential supplied in the standard authorization header.
- Preconditions & Authorization: the authorization header must be present and well-formed; the credential must verify and resolve to a valid identity.
- Behavior: produces a new opaque challenge identifier with a short time-to-live (30 seconds), records it (associated with the resolved identity, role, and the originating credential) until consumed or expired, and opportunistically purges any already-expired challenges.
- Success Output: HTTP 200 JSON containing the challenge identifier, its absolute expiry time, and the time-to-live in milliseconds.
- Error Conditions: missing/malformed authorization -> HTTP 401; unverifiable/invalid credential -> HTTP 401; unexpected failure -> HTTP 500. In simplified mode this route does not exist -> HTTP 404 with an explanatory message.
- Invariants & Guarantees: a challenge is single-use (consumed on successful verification), expires after its time-to-live, and verification of an expired challenge deletes it.

#### Connection acknowledgement / status — `POST /connect`
- Invocation: trusted HTTP request.
- Behavior/Output: HTTP 200 JSON with the conversation identifier, current active connection count, and room mode. No state change.

#### Force-disconnect a connection — `POST /disconnect`
- Invocation: trusted HTTP request.
- Inputs: a connection identifier (string, required in body).
- Behavior: removes that connection from the room as if it had closed (see leave behavior). Always returns HTTP 200 JSON success even if the connection identifier is unknown (no-op).
- Side Effects: connection record deleted; participant list updated; a “user left” event broadcast only if that user has no remaining connections.

#### Inject a broadcast event into a room — `POST /broadcast`
- Invocation: trusted HTTP request used to deliver a supplied event.
- Inputs: a fully-formed real-time event object in the body.
- Behavior: delivers the event to every active connection in the room as an “event” frame.
- Success Output: HTTP 200 JSON success.

#### List participants — `POST /participants`
- Output: HTTP 200 JSON with the participant list, the active connection count, and the last activity timestamp.

#### Room metrics — `POST /metrics`
- Output: HTTP 200 JSON with conversation identifier, mode, active connection count, participant count, a monotonically-increasing message sequence counter, last activity timestamp, and an active flag. In full mode it additionally reports the recent-message history length and an uptime estimate.

**Conversation-room scaling (observable guarantee).** A single conversation's real-time room may be served transparently by more than one server-side instance to bear load; this is invisible to clients. The system guarantees that a message delivered to the room reaches every participant of that conversation regardless of which instance serves their connection, that no connection receives the same message twice, and that the per-conversation message order is preserved. Cross-instance propagation is best-effort in the sense that a failure to reach one instance never blocks delivery to participants on another.

#### WebSocket inbound message handling (frames sent by a connected client)
- Invocation: a client sends a JSON frame over an established socket. Every inbound frame refreshes the connection’s and the room’s last-activity timestamp.
- Recognized frame kinds and observable effects:
  - Heartbeat ping -> the room replies to that socket with a heartbeat pong carrying a timestamp.
  - Subscribe / unsubscribe (full mode only) -> accepted and acknowledged; no client-visible state changes beyond logging in the current behavior; ignored entirely in simplified mode.
  - Chat message -> see “Send a chat message” below.
  - Generic event (full mode only) -> only typing-start/typing-stop event kinds have an effect; they are relayed to all other connections (not echoed to the sender) as an event frame. Other event kinds are ignored.
  - Sync request (full mode only) -> see “Reconnection sync” below.
  - Any unrecognized frame kind -> the room sends that socket an error frame stating the unknown message kind.
  - Any frame that fails to parse as JSON -> the room sends that socket an error frame “invalid message format”.

#### Send a chat message (inbound chat frame)
- Inputs (frame body): optional content string, an optional message-type label (e.g. text/image/file or typing markers), an optional sender display name, optional free-form metadata, and an optional client-supplied message identifier.
- Preconditions & Authorization (full mode only): the sender must have an active connection in this room bound to this conversation and a recognized role (administrator or agent); otherwise the room sends an error frame “permission denied to send messages” and drops the message. Simplified mode performs no permission check.
- Behavior (observable order):
  1. Assign the message the next value of the room’s monotonically-increasing sequence counter (this ordering number is attached to the outgoing event metadata).
  2. If the message-type marks it as a typing indicator, relay it to all other connections only and stop (no history, no persistence).
  3. Otherwise build a “message sent” event (carrying message identifier, content, message-type defaulting to text, sender name, and metadata including the order number), tagged high priority.
  4. In full mode, append a normalized copy to the room’s recent-message history (history is capped, default 50 entries, oldest dropped when full) and schedule a short-delay preservation of recent history.
  5. Fan the event out to every connection in the room.
  6. The message is also delivered to participants served by any other instance of the same conversation (best-effort, non-blocking).
  7. In full mode, hand the event to an external persistence integration point.
- Success Output: no direct reply to the sender beyond the broadcast event the sender also receives; ordering number is observable in the delivered event.
- Invariants & Guarantees: ordering numbers are strictly increasing per room and define the canonical order of messages in that room. Typing indicators are never stored. History is bounded; a crash may lose the most recent few seconds of recoverable room history, but delivery and external persistence are independent of that recent-history buffer.

#### Reconnection sync (inbound sync request frame, full mode only)
- Inputs: an optional “since” timestamp (ISO 8601) and optionally the conversation identifier.
- Behavior: computes the set of stored history messages newer than the supplied “since” time (empty set if no “since” given, no history, or not in full mode) and sends that socket a “sync response” event containing the missed messages, the missed count, a synced-at timestamp, and the server’s current last-message timestamp.
- Invariants: only messages still resident in the bounded history can be recovered; messages older than the retention window are unrecoverable via this path.

#### Connection leave / cleanup (socket close, error, forced disconnect, or inactivity)
- Invocation: a socket closes, errors, is force-disconnected, or is reaped by the inactivity sweep (full mode runs a periodic sweep roughly every five minutes, removing connections idle beyond the inactivity timeout, default five minutes).
- Behavior: removes the connection; if the user has no other connections in the room, removes them from the participant set and broadcasts a “user left” event to remaining connections. Connection record is deleted from storage and the participant list re-persisted.
- Invariants: a “user left” event fires once per user departure, not once per socket, so multi-tab users do not appear to leave until their last socket closes.

---

#### Queue an event for routed delivery — `POST /broadcast`
- Invocation: trusted HTTP request for routed delivery.
- Inputs: an event object, a list of targets (each target names an audience type — conversation, user, team, or global — and the specific identifiers), and optional delivery options.
- Preconditions: the event must be well-formed (must carry an identifier, a type, a timestamp, and a data payload).
- Behavior: enqueues the event into either the high-priority queue (if priority is high or urgent) or the normal queue (otherwise); enforces queue-overflow protection by evicting older low/normal-priority items when the normal queue exceeds its cap (default cap 10000); persists queue state. Actual delivery happens asynchronously when the relevant queue is processed (see background processing).
- Success Output: HTTP 200 JSON with success, the event identifier, a queued-at timestamp, and the current normal-queue depth.
- Error Conditions: malformed event -> HTTP 400 (“invalid event format”); unexpected failure -> HTTP 500.
- Invariants: overflow eviction prefers dropping lower-priority events; an eviction counter is tracked. `POST /queue-event` is an alias with identical behavior.

#### Deliver immediately to specific conversations — `POST /broadcast-to-conversations`
- Inputs: an event and an array of conversation identifiers.
- Behavior: delivers the event synchronously (in parallel batches) to each named conversation room and reports per-target outcomes.
- Success Output: HTTP 200 JSON with success, event identifier, target count, count successful, count failed, and processing time in milliseconds.
- Error Conditions: missing event or non-array targets / malformed body -> HTTP 400; unexpected failure -> HTTP 500.

#### Deliver to specific users — `POST /broadcast-to-users`
- Inputs: an event and an array of user identifiers.
- Behavior/Output: same batched delivery and same response shape as the conversation variant, scoped to user connection endpoints. Same error contract.

#### Deliver to teams — `POST /broadcast-to-teams`
- Inputs: an event and an array of team identifiers (numeric).
- Behavior: for each team, resolves the team’s active members (members whose accounts are active and not soft-deleted, counting both a primary-team association and additional multi-team memberships) and delivers the event to each member’s connection. Reports aggregate successes/failures.
- Output/Errors: same response shape and error contract as the other targeted variants.

#### Deliver to teams plus all administrators — `POST /broadcast-to-teams-and-admins`
- Inputs: an event, an array of team identifiers, and an optional flag controlling administrator inclusion (defaults to including admins).
- Behavior: delivers to the named teams (as above) and, unless suppressed, additionally delivers to every active, non-deleted administrator account. This enforces team data-isolation while still letting administrators observe all conversations.
- Success Output: HTTP 200 JSON with success, event identifier, team count, the admin-inclusion flag, aggregate successes/failures, and processing time.
- Error Conditions: missing event or non-array team list -> HTTP 400; unexpected failure -> HTTP 500. Failure to resolve admins degrades gracefully (team delivery still reported).

#### Deliver globally to everyone — `POST /broadcast-global`
- Inputs: an event and an optional explicit target (defaults to “everyone”).
- Behavior: delivers the event to every currently reachable conversation room and user connection.
- Success Output: HTTP 200 JSON with success, event identifier, and aggregate successes/failures.
- Error Conditions: missing event / malformed body -> HTTP 400; unexpected failure -> HTTP 500.

#### Deliver a batch of mixed-target events — `POST /batch-broadcast`
- Inputs: an array of events and a parallel array of targets (one target per event; if a per-event target is absent the first target is reused).
- Behavior: groups events by resolved destination (per conversation, per user, per team) and delivers each group, reporting how many events were processed and aggregate successes/failures.
- Success Output: HTTP 200 JSON with success, processed count, successful count, failed count.
- Error Conditions: missing/non-array events or targets -> HTTP 400; unexpected failure -> HTTP 500.

#### Mark a live endpoint reachable — `POST /register-connection`
- Inputs: an endpoint kind (“conversation” or “user”) and its identifier.
- Behavior: marks that conversation room or user connection reachable for future broadcasts; increments the active-connection counter; preserves the set of reachable endpoint identifiers across restart when possible.
- Success Output: HTTP 200 JSON with success and the new active-connection count.
- Error Conditions: missing required dependency for the requested endpoint kind -> HTTP 503 with an explanatory message; unexpected failure -> HTTP 500.

#### Mark a live endpoint unreachable — `POST /unregister-connection`
- Inputs: an endpoint kind and its identifier.
- Behavior: marks the endpoint unreachable; decrements the active-connection counter (never below zero); preserves the updated reachable-endpoint set when possible.
- Success Output: HTTP 200 JSON with success and the new active-connection count.
- Error Conditions: unexpected failure -> HTTP 500.

#### Update subscription filters — `POST /update-filters`
- Inputs: a target key and a list of subscription filter descriptors.
- Behavior: replaces the stored filter set associated with that target key.
- Success Output: HTTP 200 JSON success.

#### Flush a delivery queue on demand — `POST /flush-queue`
- Inputs: an optional priority selector.
- Behavior: forces immediate processing of either the high-priority queue (if priority is “high”) or the normal queue (otherwise).
- Success Output: HTTP 200 JSON with success and the combined remaining queued-event count.

#### Emit a system notification — `POST /system-broadcast`
- Inputs: a message (string) and an optional priority (defaults to normal).
- Behavior: constructs a system-notification event addressed to everyone and enqueues it for delivery.
- Success Output: HTTP 200 JSON with success and the generated event identifier.

#### Routed-delivery metrics — `POST /metrics`
- Output: HTTP 200 JSON reporting total events, successful and failed delivery counts (under both legacy and current field names), average latency, events-per-second, last-processed time, queue depth, active connection count, number of reachable conversation rooms, number of reachable user connections, current normal- and high-priority queue depths, number of active exclusive sections, an uptime estimate, and a memory-usage snapshot.

#### Routed-delivery health/status — `POST /status` and `POST /health`
- Output: HTTP 200 JSON with a health flag (degraded when the normal queue exceeds 80% of capacity), queue depth, processing rate, last-processed time, active connection count, a status label (“healthy”/“degraded”), uptime estimate, computed error rate, average latency, memory snapshot, and timestamp.

#### Reachability debug snapshot — `POST /debug-connections`
- Output: HTTP 200 JSON listing the identifiers of reachable users and reachable conversations, the active connection count, and a timestamp.

#### Unknown routes (real-time delivery endpoints)
- Any unrecognized path -> HTTP 404 (“Not Found”). Any uncaught error in routing -> HTTP 500.

---

### Data Concepts (neutral)
- **Conversation room state**: per conversation, a set of live connections (each with a unique connection identifier, the owning user identifier, role, bound conversation, last-activity time, and an active flag), a participant set (distinct user identifiers currently present), a monotonically-increasing message sequence counter, last-activity timestamp, and an active flag. Full mode additionally keeps a bounded recent-message history and a set of outstanding authentication challenges.
- **Authentication challenge**: an opaque single-use identifier with an absolute expiry, associated with a resolved identity, role, and the originating credential. Expires after a short fixed lifetime.
- **Room capacity**: each conversation room has a bounded maximum number of simultaneous connections; to bear higher load a single conversation may be served by multiple instances (currently up to five) that transparently share its participants.
- **Recent-message history entry (full mode)**: a normalized record of a delivered message — message identifier, conversation identifier, content, message-type, sender type and identifier, optional sender display name, optional metadata, creation time, and a read flag. The collection is size-bounded; oldest entries are evicted first.
- **Broadcast reachability state**: records which conversation and user targets are currently reachable for event delivery; after restart, reachable targets are rediscovered or restored as needed.
- **Queued event**: an event augmented with its target audience descriptors, delivery options, a queued-at time, and a retry counter (and a retry-after time once retried).
- **Distribution statistics**: cumulative totals of events, successful and failed deliveries, last-processed time, events-per-second, average latency, queue depth, evicted-event count, and active connection count. Persisted periodically.
- **Queue-processing exclusion**: a guarantee that draining of a given event queue is mutually exclusive, so at most one processor handles it at any moment.

### State & Lifecycle
- **Connection lifecycle**: pending upgrade -> authenticated & accepted (active) -> closed/errored/force-disconnected/reaped (removed). A user is “present” while they hold at least one active connection and “absent” once their last connection is removed. Connections may also be terminated automatically at credential expiry.
- **Challenge lifecycle**: issued -> (verified once -> consumed) or (expired -> deleted on next access/sweep). Single-use and terminal once consumed or expired.
- **Room availability lifecycle**: unavailable -> available (repeating an initialization request leaves it available). Once available, the room participates in transparent multi-instance delivery.
- **Queued event lifecycle**: queued -> processed (delivered) or -> retried (re-queued, retry count incremented) up to a small retry ceiling, after which it is abandoned; low-priority events are not retried. Events may be evicted from the normal queue under overflow before ever being processed.
- **Routed-delivery health states**: healthy <-> degraded, derived from normal-queue saturation; not a persisted status, recomputed on request.

### Real-time / Event Behavior
Events delivered to clients over a room socket are wrapped as “event” frames (or “pong”/“error” frames for those cases) and include a timestamp.
- **connection_established**: sent only to a newly-connected socket; carries conversation identifier, the new connection identifier, current participant list, room mode, and the server’s last-message timestamp.
- **user_joined**: broadcast to all room connections when a connection is added; carries the user identifier, the connection identifier, role, and the new participant count.
- **user_left**: broadcast to remaining room connections only when a user’s last connection is removed; carries the user identifier, the connection identifier, and the new participant count.
- **message_sent**: broadcast to all of the conversation's room connections (including any served by other instances) when a non-typing chat message is sent; carries message identifier, content, message-type, sender name, and metadata that includes the strictly-increasing ordering number. Typing-indicator messages are instead relayed only to other connections and are never stored.
- **typing_start / typing_stop**: relayed to all other connections (never echoed to the sender); not persisted.
- **sync_response**: sent only to the requesting socket in reply to a sync request; carries the missed-message list, missed count, a synced-at time, and the server’s last-message timestamp.
- **pong**: sent only to the requesting socket in reply to a heartbeat ping.
- **error**: sent only to the offending socket for invalid frames, unknown frame kinds, or denied actions.
- **Cross-instance delivery**: when a conversation is served by multiple instances, a message reaches participants on every instance; a connection never receives a duplicate, deliveries for a mismatched conversation are rejected, and the sender does not wait on cross-instance propagation.
- **Routed fan-out**: accepted events are delivered by audience — to specific conversation rooms (which then fan out internally), to specific users, to all members of named teams, to all administrators (for the teams-and-admins variant), or to every reachable room and user (global/system notifications). High/urgent events are processed on a fast loop; normal/low events on a slower loop; both are drained with mutual exclusion so that only one processor drains a given queue at a time. Background loops also recompute metrics on fixed intervals.

## User Real-time Sessions

### Purpose
This area defines the observable behavior of one authenticated staff user's live real-time sessions across devices and browser tabs. It covers opening and closing sessions; following conversations; online/offline presence and "last seen" timestamps; notification preferences; per-user activity counters; inbound security checks; and fan-out of pushed events to all of the user's live sessions. All operations below are implicitly scoped to one user identity.

### Operations

#### Open a real-time session — WebSocket upgrade request (Upgrade: websocket)
- Invocation: A client initiates a WebSocket upgrade for the user's personal real-time stream, supplying parameters in the request query string.
- Inputs (query string):
  - authentication token — string, required.
  - role — string, required; expected values are "admin" or "agent".
  - user identifier — string, required; the user this session belongs to.
  - device identifier — string, optional; defaults to a placeholder ("unknown") if absent.
  - token expiry — numeric epoch seconds, optional; when a positive finite value is present it schedules automatic session termination at that time.
- Preconditions & Authorization: The token must be a valid authentication token and its embedded user identity must exactly equal the supplied user identifier. The user must not already be at the maximum simultaneous-session limit.
- Behavior (observable order): validate presence of required parameters; verify the token and that it belongs to the named user; reject if the per-user session cap is already reached; otherwise establish the real-time channel, assign the session a unique session identifier, make the session reachable, persist session metadata, recompute and persist user state, and (only when this is the user's first live session) make the user reachable for global/team broadcasts. Immediately after acceptance the server pushes a welcome event to the new session (see Real-time section). If a valid token-expiry value was supplied, the session is automatically closed at that moment with a dedicated close code signaling "refresh token and reconnect."
- Success Output: a protocol "switching protocols" response (status 101) that completes the real-time channel handshake; the client end of the channel is returned to the caller.
- Side Effects: a new live session is tracked; the user becomes online; total-session-count counter increments; session metadata (device, user-agent, client IP, connect time) is retained for the session's lifetime; user presence/state is persisted; on first session the user is reachable through global delivery so global/team broadcasts reach this user.
- Error Conditions:
  - Missing token or missing role -> status 400, body "Missing required parameters".
  - Missing user identifier -> status 400, body "Missing userId parameter".
  - Token invalid or token's user identity does not match the supplied user -> status 401, body "Unauthorized".
  - Per-user simultaneous-session cap already reached -> status 429, body "Connection limit reached".
  - Server failure during channel creation/upgrade -> status 500, body "WebSocket upgrade failed".
- Invariants & Guarantees: At most a fixed number of simultaneous live sessions per user (the cap is 5). Each session receives a unique identifier. The reachability transition happens only when moving from zero to one live session (no duplicate reachability events). Session is force-closed at token expiry when expiry is provided.

#### Subscribe the user to a conversation — POST /connect (and alias POST /subscribe)
- Invocation: Authenticated request with a JSON body.
- Inputs (JSON body): conversation identifier — string, required.
- Preconditions & Authorization: The user must hold view permission for the target conversation (see Conversation Access Rules in Data Concepts).
- Behavior: verify permission; add the conversation to the user's followed set; increment the "conversations joined" counter; persist the followed set; then push a "conversation subscribed" event to every live session of the user.
- Success Output (status 200): an object with a success flag (true), the conversation identifier, and the user's current count of followed conversations.
- Side Effects: followed-conversation set persisted; counter incremented; real-time event fanned out to all the user's sessions.
- Error Conditions:
  - Permission denied -> status 403, body object with an error message "Permission denied".
  - Any processing failure -> status 500, body object with an error message "Failed to connect to conversation".
- Invariants & Guarantees: A user may follow at most a fixed number of conversations (cap is 50); this endpoint does not surface a distinct cap error but the underlying add is silently capped.

#### Unsubscribe the user from a conversation — POST /disconnect (and alias POST /unsubscribe)
- Invocation: Authenticated request with a JSON body.
- Inputs (JSON body): conversation identifier — string, required.
- Preconditions & Authorization: No permission check is performed for removal.
- Behavior: remove the conversation from the followed set; persist the set; push a "conversation unsubscribed" event to all the user's live sessions.
- Success Output (status 200): an object with a success flag (true), the conversation identifier, and the user's current count of followed conversations.
- Side Effects: followed-conversation set persisted; real-time event fanned out to all sessions.
- Error Conditions: any processing failure -> status 500, body object with error message "Failed to disconnect from conversation".
- Invariants & Guarantees: Removing a conversation that is not followed is a no-op that still reports success.

#### Update presence (heartbeat) — POST /presence
- Invocation: Authenticated client request with a JSON body.
- Inputs (JSON body): status — string, optional and currently not acted upon.
- Behavior: mark the user online, refresh the "last seen" timestamp, and persist user state.
- Success Output (status 200): an object with a success flag (true), the current online flag, and the current "last seen" timestamp.
- Side Effects: presence/state persisted.

#### Read notification preferences — GET /preferences
- Invocation: Authenticated client request.
- Behavior: return the user's current notification preferences.
- Success Output (status 200): the preferences object (see Data Concepts).

#### Replace/merge notification preferences — PUT /preferences
- Invocation: Authenticated client request with a JSON body.
- Inputs (JSON body): an object of preference fields to apply, merged over current preferences (shallow merge).
- Behavior: merge supplied fields into current preferences, persist user state, return the merged result.
- Success Output (status 200): the updated preferences object.
- Error Conditions: any other HTTP method on this path -> status 405, body "Method not allowed".

#### Get connection status snapshot — GET /status
- Invocation: Authenticated request.
- Behavior: return a point-in-time status snapshot.
- Success Output (status 200): an object containing the user identifier, online flag, "last seen" timestamp, current live-session count, an activity-statistics object, and the current count of followed conversations.

#### Get metrics snapshot — /metrics
- Invocation: Authenticated request.
- Behavior: return a metrics snapshot.
- Success Output (status 200): an object containing the user identifier, online flag, "last seen" timestamp, current live-session count, the activity-statistics object, a derived uptime value, and the count of followed conversations.

#### Push a message to all of a user's sessions — POST /broadcast
- Invocation: Trusted delivery request with a JSON body.
- Inputs (JSON body): a real-time message object to deliver.
- Behavior: deliver the supplied message to every live session of the user.
- Success Output (status 200): an object with a success flag (true).
- Side Effects: message fanned out to all the user's open sessions; sessions whose channel is not in the open state are skipped silently.

#### Deliver batched events to a user — POST /batch-events
- Invocation: Trusted delivery request used to deliver global or cross-conversation events (e.g., events relevant to a user viewing a conversation list).
- Inputs (JSON body): events — an array of event objects; each carries at least a type, an optional associated conversation identifier, an optional timestamp, and an event payload.
- Behavior: validate that an array of events was supplied; for each event, wrap it as a real-time event message (preserving its conversation association and timestamp, defaulting timestamp to now) and deliver it to every live session of the user; count delivered events.
- Success Output (status 200): an object with a success flag (true), the number of events delivered, the user identifier, and the current count of active sessions.
- Error Conditions:
  - Missing or non-array events -> status 400, body object with error "Invalid events format".
  - Any processing failure -> status 500, body object with error "Failed to process batch events".

#### Unknown HTTP path — any other route
- Behavior: returns status 404, body "Not Found".
- Any unhandled request-dispatch error returns status 500, body "Internal Server Error".

### Real-time message protocol (inbound over an open session)
Once a session is open, the client may send framed messages. Every inbound frame is first subjected to security checks, then dispatched by its declared type.

- Security gate (applies to all inbound frames):
  - Rate limit: each session may send at most a fixed number of frames per fixed time window (10 frames per 1-second window). Exceeding it does not process the frame; instead an error message "Rate limit exceeded. Please slow down." is returned to that session.
  - Size limit: frames whose serialized form exceeds a fixed maximum (10240 bytes) are rejected with an error message stating the maximum allowed size; the frame is not processed.
  - On any accepted frame, the user's activity timestamp and the session's last-activity timestamp are refreshed.
  - A frame that cannot be parsed yields an error message "Invalid message format" to that session.
- Frame types and handling:
  - "ping" -> the session receives a "pong" frame carrying a server timestamp.
  - "subscribe" -> if the payload designates a conversation target, the user's view permission for that conversation is verified; on success the conversation is added to the followed set, the set is persisted, and a "subscription added" event (with current followed count) is returned to the requesting session; if permission is denied, an error "Permission denied to subscribe to this conversation" is returned; if the followed-conversation cap is already reached, an error "Maximum subscriptions reached" is returned.
  - "unsubscribe" -> if the payload designates a conversation target, the conversation is removed from the followed set, the set is persisted, and a "subscription removed" event (with current followed count) is returned to the requesting session. No permission check.
  - "message" (chat) -> requires an associated conversation identifier (else error "Conversation ID required for chat messages") and requires the user to currently follow that conversation (else error "Not subscribed to this conversation"). On success the session receives a "message acknowledged" event echoing the message id, conversation, and user; the user's sent-message counter increments. (This area only acknowledges; actual message persistence/processing happens elsewhere.)
  - "event" -> for event subtypes "typing start" and "typing stop", the event is re-broadcast to all of the user's own live sessions; all other event subtypes are merely acknowledged (logged) with no client-visible effect.
  - Any other frame type -> error "Unknown message type: <type>" returned to the session.

### Data Concepts (neutral)

- Per-user live-session state: the live-session and follow state for one user identity. Identity defaults to a placeholder until a session supplies the real user identifier.
- Live session: a single real-time client channel. Carries a unique session identifier, the owning user, the role, a last-activity timestamp, an active flag, and metadata (device identifier, client user-agent, client IP, connect time). Persisted session records are keyed per session and removed when the session closes.
- Followed-conversation set: the set of conversation identifiers the user is currently tracking; persisted as a whole and restored on unit startup; capped at 50 entries.
- Notification preferences: an object with a notification-settings group containing independent boolean toggles for: new-message alerts, message-recall alerts, conversation-assignment alerts, and system notifications. All default to enabled. Updates are shallow-merged.
- Activity statistics: per-user counters for total sessions opened over time, messages sent, messages received, conversations joined, and a last-activity timestamp.
- Persisted user-state snapshot: a consolidated record holding user identity, online flag, last-seen timestamp, current session count, the followed-conversation list, preferences, and statistics; restored on unit startup and re-persisted on relevant changes.
- Conversation Access Rules (used for permission checks): permission is evaluated against persisted conversation and user records. Administrators are granted access to any conversation. For a conversation with no assigned owning team, access is granted only for a read-style action and denied otherwise. For a conversation assigned to an owning team, access is granted only if the user is a member of that owning team. If the conversation or the user record cannot be found, or any error occurs during evaluation, access is denied (fail-closed). The "view" permission used by subscribe operations is distinct from "read": an unassigned conversation grants "read" but not "view".

### State & Lifecycle

- Presence state machine: a user is "offline" when zero live sessions exist and "online" when at least one exists.
  - Opening the first session: offline -> online; triggers a transition to globally reachable.
  - Opening additional sessions (up to the cap): remains online; reachability is unchanged.
  - Closing a session while others remain: remains online.
  - Closing the last session: online -> offline; refreshes the "last seen" timestamp; triggers reachability removal from the global delivery reachability.
  - A presence heartbeat explicitly marks the user online and refreshes "last seen".
- Session cleanup: a periodic background sweep (every 5 minutes) closes any session with no activity for longer than a fixed idle threshold (10 minutes); each such closure follows the normal session-removal path (presence recompute, reachability removal if it was the last session, storage cleanup).
- Token-expiry termination: when token expiry was provided at open time, the session is force-closed at that instant with a dedicated close code instructing the client to obtain a fresh token and reconnect.
- Session removal effects (any closure cause): the session is untracked; its rate-limit accounting is discarded; presence is recomputed; if it was the last session the user is made unreachable for delivery; the persisted session record is deleted; user state is re-persisted.

### Real-time / Event Behavior

Events emitted by this area to its own user's live sessions:
- Welcome event ("user connected"): pushed to a newly opened session immediately after acceptance; payload includes the user identifier, the new session identifier, the user's current followed-conversation list, the user's preferences, and the user's current statistics.
- "conversation subscribed" / "conversation unsubscribed": fanned out to all of the user's sessions when the user follows/unfollows a conversation via the HTTP subscribe/unsubscribe operations; payload includes the conversation identifier and the current followed count.
- "subscription added" / "subscription removed": returned to the requesting session in response to inbound subscribe/unsubscribe frames; payload echoes the subscription descriptor and the current followed count.
- "message acknowledged": returned to the requesting session after a valid inbound chat frame; payload echoes message id, conversation, and user.
- "pong": returned to the requesting session in response to an inbound ping; carries a server timestamp.
- "typing start" / "typing stop": re-broadcast to all of the user's live sessions when received as inbound event frames.
- Error frames: returned to a single session whenever validation, rate-limiting, size-limiting, permission, or dispatch checks fail; each carries a human-readable error string and a timestamp.
- Generic pushed events: any message delivered via the broadcast or batch-events HTTP operations is fanned out to all of the user's live sessions as-is; non-open sessions are skipped.

Reachability updates this area produces:
- On the user's first live session it updates global/team broadcast reachability (identifying itself as a user with the user identifier) so global/team broadcasts can reach this user; on losing the last session the user becomes unreachable for global/team broadcasts. Reachability updates are best-effort and non-blocking.

Audience rules: events generated in response to a single inbound frame go only to the originating session; subscription-change notifications via HTTP and all broadcast/batch deliveries go to every live session of the same user; nothing in this area pushes directly to other users.

## Customer-Side Real-time Channels

### Purpose
This area defines per-conversation real-time and message behavior for the customer-support side of the system. For a conversation, authenticated viewers can open a live channel, list messages with pagination, create outbound agent messages, upload file assets, and receive message/update/presence events. After an outbound message is accepted, the system delivers it to connected viewers, attempts any required outbound platform push to the end customer, and emits a global notification used to refresh conversation-list views.

### Operations

#### Open a real-time conversation channel — GET /ws (WebSocket upgrade)
- Invocation: A client opens a WebSocket for the conversation's real-time channel. The request must carry the WebSocket upgrade header; otherwise it is treated as a plain HTTP request.
- Inputs (query string):
  - conversation identifier — string, identifies which conversation channel to join.
  - A "pre-validated" flag — when present and set to the literal "true", caller-supplied identity fields are trusted and skips the fallback session lookup. Optional.
  - When the pre-validated flag is set: a validated user identifier (required for the fast path to be taken), a validated role (defaults to the agent role if absent), and a validated display label (defaults to a generic label if absent).
  - A session token — string; used only on the fallback path when the request is not pre-validated.
- Preconditions & Authorization:
  - Fast path: if the pre-validated flag is "true" AND a validated user identifier is supplied, identity is accepted as-is without a fallback session lookup.
  - Fallback path: the supplied session token is looked up in the session store. The lookup fails (and the socket is rejected) if the token is empty, not found, expired, or stored in an unreadable form. On success the identity (user identifier, display label, role defaulting to the agent role) is taken from the stored session.
- Behavior (observable, in order): a socket pair is created; the server side is accepted; the connection is registered under a freshly generated unique connection identifier (so the same user may hold multiple simultaneous connections, e.g. multiple tabs); a presence "connected" event for that user is then broadcast to all other connections on the channel; thereafter the connection receives all channel broadcasts until it closes or errors.
- Success Output: an HTTP 101 switching-protocols response that hands the client side of the socket back to the caller.
- Side Effects: the connection becomes part of the live audience for this conversation until it closes; a presence "connected" event is emitted to other connections (see Real-time / Event Behavior). No persistent storage is written.
- Error Conditions:
  - Upgrade header absent -> HTTP 400 with a plain message indicating a WebSocket was expected.
  - Fallback path with empty session token -> the server socket is closed with a policy-violation close code and a reason indicating a session identifier is required, and an HTTP 400 plain-text response is returned.
  - Fallback path with invalid/expired/missing/unreadable session -> the server socket is closed with a policy-violation close code carrying the failure reason, and an HTTP 401 JSON response is returned with a success flag of false and an error message.
- Invariants & Guarantees: each accepted connection gets a unique connection identifier; multiple connections per user are supported; inbound application messages from clients are currently accepted but produce no observable effect (reserved for future use such as typing indicators / read receipts).

#### Broadcast a newly created message to the channel — POST /notify-message
- Invocation: Triggered after a message is created, to fan out that message to connected viewers.
- Inputs (JSON body): the conversation identifier; a message object carrying optional fields — a message identifier, content, a message-type label, a sender-type label, a sender identifier, and a platform label.
- Behavior: the system pushes a "new message" real-time event to every currently-open connection on the channel. Connections whose socket is not open, or whose send fails, are removed from the live audience.
- Success Output: HTTP 200 JSON with a success flag of true plus a diagnostic block reporting the current total connection count, the list of distinct connected user identifiers, and the conversation identifier this instance is bound to.
- Error Conditions: any processing error -> HTTP 500 JSON with a success flag of false and a stringified error.
- Side Effects: emits the "new message" event (see Real-time / Event Behavior); prunes dead/closed connections.

#### Broadcast a message update to the channel — POST /notify-message-updated
- Invocation: Trigger, issued after deferred media processing for a message completes (e.g. once file attachments become available).
- Inputs (JSON body): the conversation identifier; the message identifier; and a data object that may include a list of file attachments.
- Behavior: pushes a "message updated" real-time event to every open connection on the channel; prunes closed/failed connections as it goes.
- Success Output: HTTP 200 JSON with a success flag of true.
- Error Conditions: any processing error -> HTTP 500 JSON with a success flag of false and a stringified error.
- Side Effects: emits the "message updated" event; prunes dead connections.

#### List conversation messages — GET /messages
- Invocation: Authenticated client request for a conversation's message list.
- Inputs:
  - Conversation identifier — supplied via a dedicated request header. Required.
  - A page-size limit — query parameter, integer, default 50.
  - An optional "before" cursor — query parameter holding a message identifier; results are constrained to messages older than that anchor message.
- Preconditions & Authorization: the conversation-identifier header must be present. (This endpoint does not itself re-validate a session token; auth is enforced before this endpoint is reached.)
- Behavior: returns up to the requested number of messages for the conversation, newest first. When a "before" cursor is provided and resolves to an existing anchor message with a valid creation time, only messages created earlier than that anchor are returned; if the cursor message cannot be found or has no usable creation time, the request degrades gracefully to returning the latest messages. For the returned messages, associated file attachments are looked up and attached per message. Each attachment is augmented with a separately-minted, time-limited signed "download" link that forces a save-as download (distinct from the inline view URL); if an attachment lacks the stored object reference, or the link cannot be minted, the download link is simply omitted and clients fall back to the inline URL.
- Success Output: HTTP 200 JSON with: a success flag of true; a list of messages; and a "has more" flag that is true when the returned count equals the requested page size (signalling further pages may exist). Each message in the list exposes its stored fields plus a unified sender-identifier field (resolved from whichever of the agent-sender or customer-sender identity applies) and a list of its file attachments (each optionally including the signed download link).
- Error Conditions:
  - Missing conversation-identifier header -> HTTP 400 JSON, success flag false, error message.
  - Any retrieval failure -> HTTP 500 JSON, success flag false, generic failure message.
- Invariants & Guarantees: ordering is strictly newest-first; pagination is cursor-based on the anchor message's position; the "has more" flag is purely derived from whether a full page was returned.

#### Create an outbound (agent) message — POST /messages
- Invocation: Authenticated agent client request to send a message into a conversation.
- Inputs:
  - Conversation identifier — request header. Required.
  - A session/credential token — request header. Required. If it is in a three-part signed-token form, the caller's user identifier and display label are read from its decoded middle segment; otherwise the raw header value is treated as the user identifier.
  - JSON body fields: text content (string); an optional list of asset URLs; an optional list of attachment identifiers to link; an optional message-type label; an optional platform label; an optional correlation identifier used by clients for de-duplication.
- Preconditions & Authorization: both the conversation-identifier header and the credential header must be present. At least one of non-blank content OR a non-empty attachment-identifier list must be supplied.
- Behavior (observable, in order):
  1. A new message record is persisted for the conversation, attributed to the agent sender, marked as sent and in a "delivered" delivery state, with the resolved sender display label captured as a snapshot, and with the supplied assets, attachment identifiers, platform label, and correlation identifier retained as message metadata. The effective message-type becomes a file-type when attachment identifiers are present, otherwise the supplied type or a text default.
  2. Any referenced attachment records are linked to the new message.
  3. The conversation's "last activity" and "last message" timestamps are advanced so the conversation re-sorts to the top of listings.
  4. If the conversation's customer belongs to the LINE platform and has a platform user identity, the message is delivered outbound to that customer over LINE: text content (when present) plus one outbound element per linked attachment — images sent as image elements, other files sent as a file element carrying the file name, type, and size. Outbound elements are sent in batches capped at five per send call. Attachments lacking a usable URL are skipped. LINE delivery failure is logged but does not fail the request.
  5. The new-message real-time event is fanned out to connected viewers of this conversation (via the conversation channel).
  6. A global "new message" notification is emitted to the routed delivery path so conversation-list views update their last-message preview in real time.
- Success Output: HTTP 200 JSON with a success flag of true and the created message object, which includes its stored fields, a unified sender-identifier field, the list of linked attachments, and the echoed correlation identifier (for client-side de-duplication of the matching real-time event).
- Side Effects: persists a new message; links attachments; advances conversation activity timestamps; pushes outbound LINE message(s) to the customer; emits a channel-scoped new-message event and a global new-message event. Real-time fan-out and global broadcast failures are non-fatal (logged only); the message remains persisted and visible on next fetch.
- Error Conditions:
  - Missing conversation-identifier header -> HTTP 400 JSON, success flag false.
  - Missing credential header -> HTTP 401 JSON, success flag false.
  - Neither content nor attachments provided -> HTTP 400 JSON, success flag false.
  - Any persistence/processing failure -> HTTP 500 JSON, success flag false, generic failure message.
- Invariants & Guarantees: each message gets a server-generated unique identifier and creation timestamp; real-time delivery uses the accepted message payload immediately after creation, so clients do not depend on a follow-up read before seeing the event; the correlation identifier is round-tripped end-to-end to let clients suppress duplicate optimistic copies; outbound-platform send and broadcasts are best-effort and never roll back the stored message.

#### Upload a file asset — POST /upload
- Invocation: Authenticated client request to attach a file to a conversation.
- Inputs:
  - Conversation identifier — request header. Required.
  - A session token — request header. Required and validated against the session store.
  - Multipart form data carrying a single file part. Required.
- Preconditions & Authorization: both the conversation-identifier header and session-token header must be present; the session token must resolve to a valid, non-expired session in the session store.
- Behavior: the file is stored in file storage under a key namespaced by the conversation identifier and a random identifier, preserving the original file extension and recording its content type with a one-week cache directive. A public URL for the stored object is then generated.
- Success Output: HTTP 200 JSON with a success flag of true, the public URL of the stored file, the original file name, the file size, and the content type.
- Error Conditions:
  - Missing conversation-identifier header -> HTTP 400 JSON, success flag false.
  - Missing session-token header -> HTTP 401 JSON, success flag false.
  - Invalid/expired/missing session -> HTTP 401 JSON, success flag false, with the validation error.
  - No file part in the form -> HTTP 400 JSON, success flag false.
  - Any storage failure -> HTTP 500 JSON, success flag false, generic failure message.
- Invariants & Guarantees: stored object keys are unique per upload; this operation only stores the asset and returns its URL — it does not itself create a message or link the asset (linking happens later when a message is created referencing the attachment).

#### Non-matching request handling
- For the real-time channel, any path other than the WebSocket-upgrade path and the notification paths returns HTTP 404 plain text. A non-upgrade request to the WebSocket path returns HTTP 400.
- For the message API, cross-origin requests are permitted (permissive CORS is applied to all routes).

### Data Concepts (neutral)

- **Live connection record:** for each open socket, the system tracks a unique connection identifier, the connected user's identifier, their display label, their role, and the time the connection was established. The record exists only while that live connection is active and is not a durable user record. A single user may own several connection records simultaneously.
- **Session/credential:** an opaque token resolving (via the session store) to a user identifier, a display label, an optional role (administrator, agent, or customer; defaults to agent), and an expiry time. Alternatively a three-part signed token whose middle segment yields the user identifier and display label without a store lookup.
- **Message:** belongs to a conversation; carries content text; a sender-type marker; separate agent-sender and customer-sender identity slots (only one populated; the wire exposes a unified sender-identifier); a message-type label (text or file); a delivery-state marker; a "sent" flag; a captured sender-label snapshot; a creation timestamp; recall-related markers; and a metadata blob holding asset URLs, linked attachment identifiers, the originating platform label, and a client correlation identifier.
- **File attachment:** belongs to a message once linked; carries a file name, content type, size, an inline-view URL, and a stored-object reference. On read it is enriched with an optional separately-minted, expiring signed download link that forces a save-as download.
- **Conversation (referenced, not owned here):** carries a reference to its customer and "last activity" / "last message" timestamps that this area advances when a message is created.
- **Customer (referenced):** carries a platform label and a platform-specific user identity used to deliver outbound messages on the customer's channel.
- Soft-delete is not directly handled in this area; this area reads/writes message and attachment data and advances conversation timestamps.

### State & Lifecycle

- **Connection lifecycle:** a connection moves from absent -> active (on socket accept) -> removed (on socket close, on socket error, or when a broadcast send to it fails or finds it not-open). On acceptance a presence "connected" event is emitted to peers. On removal, a presence "disconnected" event for the user is emitted only if that user has no other remaining connections on the channel (so closing one of several tabs does not mark the user offline).
- **Message lifecycle (outbound agent message):** created already in a "sent" state with a "delivered" delivery state; afterward it may receive a "message updated" broadcast once deferred media processing attaches its file attachments. There is no in-band edit/recall flow in this area (recall markers exist on the record but are initialized inactive here).

### Real-time / Event Behavior

All real-time events are JSON frames pushed over the conversation's WebSocket channel to connected viewers. Distinct event types:

- **Presence connected** — emitted when a new connection is accepted. Audience: all other open connections on the channel (the triggering connection is excluded). Payload: an event type marker indicating a user connected, the user identifier, and a timestamp.
- **Presence disconnected** — emitted when a user's last remaining connection closes or errors. Audience: all remaining open connections (the triggering connection excluded). Payload: an event type marker indicating a user disconnected, the user identifier, and a timestamp.
- **New message** — emitted to every open connection on the channel when a message is broadcast. Payload: a lowercase "new message" type marker; the conversation identifier; a top-level data object echoing the conversation identifier, content, message-type, sender-type, sender identifier, platform label (defaulting to LINE), and a timestamp; a copy of the original message object (for backward compatibility); and a timestamp. Clients may de-duplicate against an optimistic local copy using the correlation identifier round-tripped via the create-message response.
- **Message updated** — emitted to every open connection when deferred media processing completes. Payload: a "message updated" type marker; the conversation identifier; a data object with the conversation identifier, the message identifier, and the updated fields (notably the list of file attachments); and a timestamp.
- **Global new message (consumed by a separate routed delivery path, not this channel)** — on message creation, a global event is also emitted to the system-wide broadcaster targeting all listeners, used so conversation-list views refresh their last-message preview. Payload: a generated event identifier, a "new message" type, a source marker, a timestamp, the conversation identifier, a data object (conversation identifier, message identifier, content, message-type, sender-type, sender identifier, platform, timestamp), and a normal-priority marker.

Inbound client WebSocket messages are received but currently produce no observable effect (reserved for future client-side signals such as typing indicators and read receipts).

## Realtime Module & Latest-Message Cache

### Purpose
This area provides the live-messaging backbone for the support platform. It accepts requests to publish real-time events (new messages, typing indicators, status changes, assignments, notifications, system broadcasts) and routes them to connected clients through real-time delivery channels. It also maintains a fast-access cache of the most recent message per conversation, including a refresh behavior that batches cache refreshes to reduce write load and pushes refreshed-message notifications to listeners. Finally, it exposes management and monitoring endpoints for inspecting configuration, statistics, alerts, and health of the real-time subsystem.

### Operations

All HTTP endpoints below are mounted under the base path `/api/realtime` and require a valid bearer authentication token (standard authenticated session). On top of that token check, individual operations enforce additional role checks as noted. Unless stated otherwise, success responses use the platform's standard success envelope (a success flag, a human-readable message, and a `data` object) with HTTP 200, and failures return the standard error envelope with the indicated status code.

#### Publish typing-status (lightweight) — POST /api/realtime/typing
- Invocation: authenticated client request.
- Inputs: JSON body with a conversation identifier (`conversationId`, number or string). Required.
- Preconditions & Authorization: any authenticated caller.
- Behavior: validates that a conversation identifier is present; acknowledges the request. (In the current build this is an acknowledgement-only operation; actual typing propagation is delivered over the persistent real-time channel rather than synchronously by this call.)
- Success Output: 200 with `{ success: true }` and a confirmation message.
- Error Conditions: missing conversation identifier -> 400 with "Conversation ID is required"; unexpected internal error -> standard error response.

#### Broadcast a custom event to a conversation — POST /api/realtime/broadcast
- Invocation: authenticated client request.
- Inputs: JSON body with a conversation identifier (`conversationId`, number or string) and an `event` payload (arbitrary object). Both required.
- Preconditions & Authorization: any authenticated caller.
- Behavior: validates both fields are present; acknowledges the request.
- Success Output: 200 with `{ success: true }`.
- Error Conditions: missing conversation identifier or event -> 400 with "Conversation ID and event are required".

#### Get conversation real-time status — GET /api/realtime/conversation/:id/status
- Invocation: authenticated client request with a conversation identifier in the path.
- Authorization: any authenticated caller.
- Behavior: returns a static informational response directing clients to use the persistent real-time channel for live status.
- Success Output: 200 with a message and a server timestamp.

#### Update presence / online status — POST /api/realtime/online-status
- Invocation: authenticated client request.
- Inputs: JSON body with an `isOnline` boolean flag (optional).
- Authorization: any authenticated caller.
- Behavior: acknowledges the presence update; echoes the supplied flag back.
- Success Output: 200 with `{ success: true, isOnline }`.

#### Get real-time configuration — GET /api/realtime/config
- Invocation: authenticated client request.
- Authorization: requires the administrator role; any other role -> 401-style unauthorized response with "Admin access required".
- Behavior: returns the current real-time runtime configuration.
- Success Output: 200 with a configuration object containing: a delivery-version selector value (one of an automatic mode, a legacy version, or a current version), an event-driven-processing enabled flag, a queue-processing enabled flag, a heartbeat interval (milliseconds), a connection timeout (milliseconds), a maximum-retries count, and an event-storage time-to-live (seconds).

#### Update real-time configuration — PUT /api/realtime/config
- Invocation: authenticated client request.
- Inputs: JSON body containing any subset of the configuration fields listed above.
- Authorization: administrator role only; otherwise unauthorized ("Admin access required").
- Behavior: merges the provided fields into the current runtime configuration. The change affects subsequent operations in the current running service only and is not durable across restart or other running services.
- Success Output: 200 with `{ success: true }`.
- Invariants: configuration updates are runtime-scoped; concurrent running services may continue using their own current configuration until separately changed.

#### Get real-time statistics — GET /api/realtime/stats
- Invocation: authenticated client request.
- Authorization: administrator or elevated/team role; other roles -> unauthorized ("Insufficient permissions").
- Behavior: returns the current configuration snapshot plus a server timestamp.
- Success Output: 200 with `{ currentConfig, timestamp }`.

#### Real-time health check — GET /api/realtime/health
- Invocation: authenticated client request.
- Authorization: any authenticated caller.
- Behavior: returns a health summary derived from current configuration.
- Success Output: 200 with a status string ("healthy"), the configured delivery-version value, the event-driven flag, the queue-processing flag, and a timestamp.
- Error Conditions: server error -> 500 with "Health check failed".

#### Monitoring overview — GET /api/realtime/monitoring/dashboard
- Invocation: authenticated client request.
- Authorization: administrator or elevated/team role; otherwise unauthorized ("Insufficient permissions").
- Behavior: aggregates a dashboard view.
- Success Output: 200 with: a service block (status, uptime, version), a performance summary, a connections block (transport type indicated as the persistent real-time channel, with aggregate counters reported as zero because counts are reported by conversation-specific streams), an events block (total events, success rate, average processing time, per-type breakdown), a latest-metrics snapshot (or null), a capabilities block (queue/key-value/database availability flags, real-time-channel support flag), and a timestamp.

#### Monitoring metrics history — GET /api/realtime/monitoring/metrics
- Invocation: authenticated client request.
- Inputs: optional `limit` query parameter (number; default 50) bounding how many historical points are returned.
- Authorization: administrator or elevated/team role.
- Behavior: returns the most recent metrics point plus a bounded history list.
- Success Output: 200 with `{ latest, history, totalPoints }`. Each metrics point includes connection metrics, event-processing metrics, queue metrics, resource-usage metrics, a timestamp, and a collection-period value.

#### Monitoring alerts — GET /api/realtime/monitoring/alerts
- Invocation: authenticated client request.
- Inputs: optional `active` query flag (when "true", returns only unresolved alerts) and optional `limit` (default 100).
- Authorization: administrator or elevated/team role.
- Behavior: returns the matching alert list plus a summary.
- Success Output: 200 with `{ alerts, summary }`, where summary contains total count, a per-severity-level count map, and a count of alerts raised within the last 24 hours. Each alert carries an identifier, a severity level (informational/warning/error/critical), a metric name, a threshold, a current value, a message, a timestamp, and a resolved flag.

#### Resolve a monitoring alert — POST /api/realtime/monitoring/alerts
- Invocation: authenticated client request.
- Inputs: JSON body with an alert identifier (`alertId`). Required.
- Authorization: administrator or elevated/team role.
- Behavior: marks the identified alert as resolved.
- Success Output: 200 with `{ alertId, resolved: true }`.
- Error Conditions: missing identifier -> 400 with "Alert ID is required"; alert not found or already resolved -> 404 with "Alert not found or already resolved".

#### Monitoring health detail — GET /api/realtime/monitoring/health
- Invocation: authenticated client request.
- Authorization: administrator or elevated/team role.
- Behavior: returns a detailed health report including service health and a checks block for the database, the key-value store, the coordination/queue layer, and the legacy streaming subsystem.
- Success Output: 200. Each dependency check reports a status of healthy/degraded/down and, where applicable, a response-time measurement. Database is reported degraded if its probe exceeds a latency threshold (about one second); the key-value store is reported degraded above a smaller latency threshold (about half a second). The legacy streaming check always reports down (that transport has been removed). The coordination/queue layer reports down if the underlying real-time coordination components are unavailable.

#### Monitoring version info — GET /api/realtime/monitoring/config
- Invocation: authenticated client request (also accepts POST at the same path).
- Authorization: administrator or elevated/team role.
- Behavior: returns the currently selected delivery version, the list of available versions, capability flags, and per-version upgrade recommendations.
- Success Output: 200 with version-information object.

### Programmatic Event-Publishing Operations

The following are not exposed as standalone authenticated HTTP routes in this area but are the canonical event-publishing entry points used by other parts of the system. Their authorization rules and validation are part of this area's contract. Each validates its payload, assigns a priority, routes to delivery, and records statistics.

#### Publish a new-message event
- Inputs: an object with a numeric message identifier, a numeric conversation identifier, message text content, a message-type value (one of: text, image, file, sticker, location), and a sender-type value (one of: customer, agent, system). All required and type-checked.
- Authorization: any authenticated caller.
- Behavior: routed at high priority to the target conversation's real-time room. Records a successful or failed event metric.
- Success Output: an event identifier and a processing-time measurement.
- Error Conditions: payload failing validation -> 400 "Invalid message event data"; unauthenticated -> unauthorized.

#### Publish a typing event
- Inputs: numeric conversation identifier, numeric user identifier, user display name, and a typing-active boolean. All required.
- Authorization: any authenticated caller.
- Behavior: maps to a typing-started or typing-stopped event based on the boolean, excludes the typing user from delivery targets, routes at low priority.
- Error Conditions: validation failure -> 400 "Invalid typing event data".

#### Publish a status-change event
- Inputs: numeric conversation identifier, prior status string, new status string, numeric actor identifier. Required.
- Authorization: any authenticated caller.
- Behavior: routed at normal priority to the conversation.
- Error Conditions: validation failure -> 400 "Invalid status event data".

#### Publish an assignment-change event
- Inputs: numeric conversation identifier, a new-assignee object (type user or team, numeric id, name), an optional prior-assignee object (same shape), and a numeric actor identifier. Required fields type-checked.
- Authorization: administrator or elevated/team role only; otherwise unauthorized ("Insufficient permissions").
- Behavior: routed at high priority. If a prior assignee exists, both the prior and new assignee (as user targets or team targets according to assignee type) are added to the delivery targets so both are notified.
- Error Conditions: validation failure -> 400 "Invalid assignment event data".

#### Publish a notification event
- Inputs: numeric notification identifier, a type string, a title, content text, and a non-empty array of numeric target-user identifiers. Required.
- Authorization: administrator or elevated/team role only.
- Behavior: routed at normal priority to the listed users.
- Success Output: event identifier, count of target users, processing time.
- Error Conditions: validation failure -> 400 "Invalid notification event data".

#### Publish a system-announcement / broadcast event
- Inputs: a type value (one of: maintenance, update, alert, info), a message string, a severity value (one of: low, medium, high, critical), and an optional array of affected-user identifiers. Required fields type-checked.
- Authorization: administrator role only.
- Behavior: broadcast to all connected clients (or scoped to affected users if supplied). Priority is derived from severity: critical maps to the highest urgency, high maps to high, otherwise normal.
- Error Conditions: validation failure -> 400 "Invalid system event data".

#### Read event statistics (programmatic)
- Authorization: administrator or elevated/team role.
- Output: aggregated event counts by type, by priority, by source, average processing time, success rate, error rate, and related metrics.

#### Reset event statistics (programmatic)
- Authorization: administrator role only.
- Behavior: clears the runtime event-statistics counters. Output: `{ success: true }`.

### Latest-Message Cache Operations

The system maintains, per conversation, a short-lived snapshot of the most recent message (conversation identifier, message identifier, message text content, creation timestamp, sender-type, optional agent-sender identifier, optional customer-sender identifier, message-type, and a cached-at timestamp). Cache entries expire automatically after 24 hours.

#### Read latest message for a conversation
- Invocation: event publication requested by another subsystem.
- Inputs: a conversation identifier.
- Behavior (observable): returns the cached snapshot if present; if absent, derives the most recent message from persistent storage, stores it, and returns it; returns absence indicator if the conversation has no messages. On cache errors, falls back to deriving directly from persistent storage.
- Guarantees: read-through population; the returned snapshot always reflects the single most recent message by creation time.

#### Read latest messages for multiple conversations
- Inputs: a list of conversation identifiers.
- Behavior: returns a mapping from conversation identifier to its latest-message snapshot. Identifiers found in cache are returned directly; identifiers missing from cache are resolved from persistent storage and then cached.
- Guarantees: conversations with no messages are simply omitted from the result.

#### Store / refresh latest message
- Invocation: triggered when a conversation gains a new most-recent message.
- Behavior: the latest-message snapshot is refreshed with the 24-hour expiry. Refreshes may be coalesced; if refresh cannot complete, the failure is logged and the authoritative read path remains available.
- Guarantees: write failures are tolerated (logged, non-fatal); the cache is best-effort.

#### Invalidate latest message
- Inputs: a conversation identifier.
- Behavior: removes the cached snapshot for that conversation. Failures are non-fatal.

#### Warm up cache
- Inputs: an optional limit (default 50; a larger limit may apply to batch warmup).
- Behavior: refreshes cache entries for the most recently active conversations up to the limit.
- Output: the count of conversations warmed.

### Cache Refresh & Latest-Message View — guarantees

The system maintains a cached "latest message" view per conversation to serve list and preview reads quickly. The specification states only the observable guarantees, not how refreshes are implemented.

1. **Eventual freshness.** After a conversation's latest message changes, its cached latest-message view eventually reflects the change without external intervention; refreshes may be coalesced, so the update is not necessarily instantaneous.
2. **Authoritative fallback.** A read that misses the cache (or finds no cached view) falls back to the authoritative data, so a missing or not-yet-refreshed cache never yields a wrong or empty answer when the underlying data exists.
3. **Non-blocking failure.** A failure in cache refresh never blocks or corrupts correct response generation; the authoritative path still serves the request.
4. **Invalidation.** A conversation's cached view can be invalidated, after which the next read reflects authoritative data and may repopulate the cache.
5. **Bounded staleness.** A cached latest-message view does not persist indefinitely; it expires (on the order of a day) so stale views do not accumulate.

### Data Concepts (neutral)

- **Latest-message snapshot**: per conversation, the single most recent message. Carries: conversation identifier, message identifier, message text content, creation timestamp, sender category (customer / agent / system), optional agent-sender identifier, optional customer-sender identifier, message-type category, and a cached-at timestamp. Expires automatically 24 hours after being cached.
- **Real-time event**: an envelope carrying a generated event identifier, an event-type category, an emission timestamp, an event source category (system / user / api / webhook / queue / manual), and a type-specific data payload. Event types include: new message, typing started/stopped, agent joined/left, assignment changed, status changed, notification, conversation updated, connection lifecycle signals, heartbeat, system announcement, and user online/offline.
- **Event delivery targets**: a specification of who receives an event — a specific conversation, an explicit set of users, an explicit set of teams, a role set, a global broadcast flag, and an exclusion list of users to skip.
- **Processing/performance statistics**: runtime aggregates of event counts (by type, priority, source), processing-time averages, success/error rates, plus cache-refresh counts (processed, succeeded, failed). These are not persisted long-term and reset on restart or explicit reset.
- **Performance alert**: an identifier, a severity level, the offending metric, its threshold and current value, a message, a timestamp, and a resolution flag.

### State & Lifecycle

- **Cache-refresh lifecycle**: a refresh request is accepted -> duplicate requests for the same conversation may be coalesced -> the latest-message snapshot is recomputed -> success makes the refreshed snapshot visible; failure may be retried up to a fixed maximum, after which the refresh is abandoned and authoritative reads continue to work.
- **Per-refresh processing**: invalidates the existing cached snapshot, re-derives the latest message, repopulates the cache, and, if a message exists, emits a refreshed-message notification. A refresh with no resulting message is treated as completed (no notification).
- **Real-time service status**: the management/monitoring layer reports a service lifecycle status (initializing, running, degraded, stopped, error). Status is reported degraded when one dependency is down and error when two or more are down.

### Real-time / Event Behavior

- **Event routing by type**: each event type has a default priority and delivery strategy. Conversation-scoped events are delivered to that conversation's real-time room; global or user-targeted events are delivered through the routed delivery path. Some low-importance event types (e.g. typing-stopped, agent-left, conversation-updated) are batched and flushed together; most others are delivered immediately; a delayed strategy defers delivery by a fixed interval. Batched events are also flushed on a periodic timer and when the batch reaches its size threshold; on a batch-delivery failure the affected events are returned to the front of the pending batch for retry.
- **Refreshed-latest-message notification**: when a conversation's latest-message snapshot is refreshed and a message exists, the system emits a real-time event with type `latest_message_updated`. Payload shape: `{ type: "latest_message_updated", conversationId, data: { content, createdAt, senderType }, timestamp }`. Audience: subscribers of that specific conversation. Broadcast failures during this notification are non-fatal and do not fail the refresh.
- **Delivery dependencies**: if required real-time delivery capability is unavailable, the corresponding publish attempt fails with a server error.
- **Removed transport**: a legacy server-push streaming transport has been fully removed; related endpoints and health checks now report it as unavailable/deprecated, and all live delivery occurs over persistent real-time channels.

### Invariants & Guarantees

- The latest-message cache is best-effort and read-through: a cache miss or cache error never prevents returning correct data derived from persistent storage.
- Cache entries are time-bounded (24-hour expiry).
- Cache refresh requests are deduplicated per conversation and retried up to a fixed maximum before being abandoned.
- Configuration and statistics held in the management layer are per running instance and not synchronized across instances; statistics reset on restart.
- Connection-level safeguards (where the validation chain is applied): missing required request headers -> 400; an absent or too-short client identifier header -> 400; a non-positive or non-numeric conversation identifier -> 400; a conversation that does not exist -> 400; exceeding a per-window request-rate limit -> 429 with a retry-after hint; exceeding a per-user concurrent-connection cap -> 429.
- Authorization tiers: presence/typing/broadcast/health are open to any authenticated caller; assignment and notification publishing require an elevated/team or administrator role; system broadcasts, configuration changes, statistics reset, and maintenance require the administrator role; monitoring read endpoints require an elevated/team or administrator role.


---

# 6. Operations, Analytics & System

## Analytics

### Purpose
This area supplies the platform's reporting and insight capabilities. It lets authenticated staff query aggregated statistics about customer conversations, messages, users/agents, and system performance over selectable time windows; run ad-hoc parameterized queries; export results to files; record and query numeric time-series metrics; compare a current period against a prior period; build and serve configurable dashboards composed of widgets; trigger real-time widget refreshes; and view a dedicated security-events dashboard. All read operations are role-scoped: administrators see global data, while team/agent users are implicitly limited to their own team (and agents to their own activity).

### Operations

All routes below are served under a common API prefix. The four route families are: the analytics core (`/api/analytics/...`), period comparison (`/api/analytics/comparison/...`), dashboards (`/api/analytics/dashboard/...`), real-time dashboard control (`/api/analytics/realtime/...`), and the security dashboard (`/api/security/dashboard/...`).

#### Conversation analytics — GET /api/analytics/conversations
- Invocation: authenticated client request.
- Inputs (query string):
  - time window selector — optional string, default `7d`. Recognized values include `1h`, `6h`, `12h`, `24h`, `3d`, `7d`, `14d`, `30d`, `90d`, `1y`, `custom`. Window-to-start-time mapping is computed relative to "now"; unrecognized values fall back to a 7-day window.
  - explicit start timestamp / explicit end timestamp — optional ISO date strings; when both are provided they override the window selector.
  - requested metrics — optional comma-separated list, default `total_conversations,active_conversations`. Recognized metric names: total conversations, active conversations, closed conversations, average duration, average messages per conversation, first-response time, resolution time, customer satisfaction, distribution by channel, by team, by priority.
  - channel filter — optional, one of `line`, `facebook`, `web`.
  - team filter — optional integer.
  - status filter — optional string.
  - group-by list — optional comma-separated.
  - ordering — optional comma-separated of `field:direction` pairs; direction defaults to descending.
  - result limit — optional integer.
- Preconditions & Authorization: requires a valid bearer token. The acting user must hold the analytics view permission. Non-admins are implicitly scoped to their own team; if no team filter is supplied, the caller's team is injected.
- Behavior: resolves the concrete date range, computes a summary block, time-series trends, category distributions, and (when a window or prior-period flag is present) a current-vs-previous comparison set. Results may be served from a short-lived cache.
- Success Output (HTTP 200): an object with a `data` member and a `metadata` member. `data` contains: a summary (total/active/closed counts, average duration in minutes, average messages per conversation, average first-response time, average resolution time, customer-satisfaction score, and the resolved period start/end), an array of trend points (timestamp, numeric value, optional label), an array of distribution entries (category, value, percentage, optional label/color), and optionally an array of comparison entries. `metadata` carries total record count, processed-at timestamp, query duration in ms, a cache-hit flag, and an aggregation granularity label (`raw`/`hourly`/`daily`/`weekly`/`monthly`) chosen from the window.
- Error Conditions: validation failures (e.g., neither window nor start provided, or start after end) return HTTP 400 with a validation error message. Other processing failures return HTTP 500.

#### Message analytics — GET /api/analytics/messages
- Invocation: authenticated client request.
- Inputs (query string): same time-window, explicit start/end semantics as above. Requested metrics default to `total_messages,messages_per_hour`; recognized names: total messages, messages per hour, by type, by channel, response times, sentiment, attachment usage. Filters: conversation identifier (optional string), channel (`line`/`facebook`/`web`). Optional group-by list and result limit.
- Preconditions & Authorization: valid token + analytics view permission; non-admin team scoping applies.
- Behavior: resolves date range and returns a summary, message-volume trend series, and distributions by type, by channel, and by sentiment. May be cache-served.
- Success Output (HTTP 200): `data` + `metadata`. `data`: summary (total messages, messages-per-hour rate, average response time in minutes, and three keyed count maps for types/channels/sentiment), a volume trend array, and three distribution arrays. `metadata` as above.
- Error Conditions: validation errors → HTTP 400; other failures → HTTP 500.

#### User analytics — GET /api/analytics/users
- Invocation: authenticated client request.
- Inputs (query string): time window default `7d`; explicit start/end; requested metrics default `active_users,user_activity` (recognized: active users, user activity, user performance, user workload, user satisfaction, login patterns); a user-type selector (`agent`/`customer`/`admin`); team filter (integer) and user filter (string); group-by list; result limit.
- Preconditions & Authorization: valid token + analytics view permission. Agents are additionally scoped to their own user identity.
- Behavior: returns a summary, an activity trend series, a per-user performance list, and a per-user workload list.
- Success Output (HTTP 200): `data` + `metadata`. `data`: summary (total users, active users, average session duration in minutes, average activity per day, and a top-performers list), an activity trend array, a performance array (per user: identifier, display name, role, score, and a metrics sub-object: conversations handled, average response time, customer satisfaction, resolution rate), and a workload array (per user: identifier, display name, active conversations, daily message count, utilization rate %, working hours).
- Error Conditions: processing failures surface through the global error handler (HTTP 500 with a structured error); validation issues are reported as errors as well.

#### Performance analytics — GET /api/analytics/performance
- Invocation: authenticated client request.
- Inputs (query string): time window default `24h`; explicit start/end; requested metrics default `response_times,throughput,error_rates` (recognized: response times, throughput, error rates, system load, database performance, API performance, integration health); channel filter (`line`/`facebook`/`web`); group-by; limit.
- Preconditions & Authorization: valid token + analytics view permission.
- Behavior: returns a performance summary, a trend series, an identified-bottlenecks list, and a generated-recommendations list.
- Success Output (HTTP 200): `data` + `metadata`. `data`: summary (average response time in ms, throughput as requests/sec, error rate %, uptime %, system load %), a trend array, a bottlenecks array (each: category among api/database/integration/network, component name, metric name, current value, threshold, severity, description, recommendation list), and a recommendations array (each: identifier, category, priority, title, description, estimated impact, implementation effort, and an action-item list with per-item identifier, description, optional assignee/due-date, and status).
- Error Conditions: failures surface via the global error handler (HTTP 500).

#### Custom analytics query — POST /api/analytics/custom
- Invocation: authenticated client request with JSON body.
- Inputs (body): a query specification string (`query`), a parameters map (default empty), an optional aggregation configuration (group-by fields, aggregation functions among count/sum/avg/min/max/distinct, optional having-conditions), optional filters, group-by list, result limit, and a time window default `7d` with optional explicit start/end.
- Preconditions & Authorization: valid token; this path requires the analytics "query" permission level (stricter than plain view).
- Behavior: validates the request and executes the described query, returning whatever rows/result the query produces.
- Success Output (HTTP 200): `data` (the query result; an array or single object) plus `metadata` (record count = array length or 1, processed-at, query duration, cache-hit false).
- Error Conditions: invalid/unauthorized requests surface as structured errors via the global error handler.

#### Export analytics — POST /api/analytics/export
- Invocation: authenticated client request with JSON body.
- Inputs (body): output format (default `json`; supported `json`/`csv`/`xlsx`/`pdf`), include-charts flag (default false), optional template identifier, optional file name, optional filters, group-by list, limit, requested metrics list (default empty), and time window default `7d` with optional explicit start/end.
- Preconditions & Authorization: valid token; this path requires the analytics "export" permission level.
- Behavior: selects the underlying dataset based on the requested metrics — if any conversation-style metric is present it exports conversation analytics; otherwise if any message-style metric is present it exports message analytics; if no metrics are supplied it defaults to a conversation total-count export. If metrics are supplied but match neither family, a validation error is raised. A downloadable export artifact is then produced.
- Success Output (HTTP 200): a result object describing the export: a download URL, a file name (defaults to an auto-generated name combining a fixed prefix, a timestamp, and the format extension), a file size, the format, a generated-at timestamp, an expiry timestamp set 24 hours after generation, and an initial download count of zero.
- Error Conditions: invalid metric selection raises a validation error; other failures surface as structured processing errors.
- Invariants: exported artifacts are advertised as expiring 24 hours after generation.

#### Analytics service health — GET /api/analytics/health
- Invocation: authenticated client request (the health path is exempt from the analytics permission check, so any authenticated caller may use it).
- Behavior: probes the data store with a trivial query and probes the key-value cache with a write/delete round-trip.
- Success Output (HTTP 200): a status object reporting overall `healthy` and per-service status for the database and the cache (`healthy`/`unhealthy`).
- Error Conditions: unexpected failures surface via the global error handler.

#### Record metrics — POST /api/analytics/metrics
- Invocation: authenticated client request with JSON body.
- Inputs (body): either a `metrics` array (batch) or a single `metric` object. Each metric object requires: a non-empty string identifier, a non-empty string name, a finite numeric value, a numeric timestamp, and a tags object (key/value string map); an optional unit and optional metadata may be included.
- Preconditions & Authorization: valid token; this path (metrics write) requires the "query" permission level.
- Behavior: validates every metric, buffers them for storage, and persists them; recent metrics may also be mirrored into the short-lived cache keyed by name and tags.
- Success Output (HTTP 200): a success acknowledgement message; no data payload.
- Error Conditions: a body lacking both `metrics` and `metric` returns HTTP 400 ("missing metrics data"). Invalid metric fields (missing/wrong-typed id, name, value, timestamp, or tags) cause the request to fail with a structured error.
- Invariants: writes are batched; a metric with a non-finite value is rejected.

#### Query a named metric — GET /api/analytics/metrics/:name
- Invocation: authenticated client request.
- Inputs: path segment is the metric name. Query string: start time (epoch ms, default 0), end time (epoch ms, default "now"), an aggregation selector (one of sum/avg/min/max/count/percentile-50/percentile-95/percentile-99; ignored if unrecognized), a period/bucket selector (one of 1m/5m/15m/1h/6h/1d/1w/1M; ignored if unrecognized), an optional tags object (JSON-encoded key/value map used to filter), an optional group-by list, an optional ordering (`timestamp` ascending or `value` descending), and an optional result limit.
- Preconditions & Authorization: valid token + analytics view permission.
- Behavior: validates that name and a valid time range are present (start strictly before end). If an aggregation is requested, returns values bucketed by the chosen period; otherwise returns raw matching points (with raw points labeled as a sum aggregation at 1-minute granularity by default). Results may be cache-served for a short interval.
- Success Output (HTTP 200): an object with a `metrics` array (each entry: metric name, applied aggregation, numeric value, bucket timestamp, period label, tags map, and optionally a sample count) and a `metadata` block (record count, query duration, cache-hit flag, and the aggregation-period label, defaulting to 1m).
- Error Conditions: missing name, missing/invalid time range, or start not before end cause the request to fail with a structured error (surfaced via the global error handler).
- Invariants & Guarantees: percentile aggregations interpolate between ordered samples; query results are cached briefly keyed on the full normalized query.

#### Single-metric period comparison — GET /api/analytics/comparison/metric
- Invocation: authenticated client request (this family is protected by the standard bearer-token middleware).
- Inputs (query string): metric name (required), current-period start and end (required ISO strings), optional previous-period start and end, optional team filter (integer), optional user filter (integer).
- Behavior: if a previous period is not supplied, it is computed automatically as an equal-length window ending one second before the current window begins. Both period values are computed and compared.
- Success Output (HTTP 200): a `comparison` object and a `metadata` block. The comparison contains current value, previous value, absolute change, percentage change, a trend label (`up`/`down`/`stable`), and the two periods (each with start/end and a human-readable label). Trend is `stable` when the magnitude of the percentage change is under 5%. When the previous value is zero, the percentage change is reported as 100 if the current value is positive, else 0. Metrics with no available source data report value 0 / stable.
- Error Conditions: missing metric, current-start, or current-end returns HTTP 400 with a "missing required parameters" message. Other failures surface via the global error handler.
- Recognized comparison metrics: total/active/closed conversations, average resolution time, customer-satisfaction score (reported as unavailable / null-treated-as-0), total/customer/agent messages, average response time, messages per conversation, active users, total activities, average session duration, user-engagement rate. Unknown metric names yield 0.

#### Multi-metric period comparison — GET /api/analytics/comparison/metrics
- Invocation: authenticated client request.
- Inputs (query string): comma-separated metric list (required; trimmed), current-period start/end (required), optional previous-period start/end, optional team and user filters.
- Behavior: compares each metric for the same period pair, then derives an overall verdict: `positive` when improved metrics outnumber declined ones and reach at least half the set, `negative` in the symmetric case, otherwise `neutral`.
- Success Output (HTTP 200): a `comparison` object containing a per-metric map of comparison entries plus a summary (total metrics, improved count, declined count, stable count, overall trend), and a metadata block (metric count, the periods).
- Error Conditions: missing metrics list / current-start / current-end → HTTP 400. Empty metric set → HTTP 400.

#### Preset comparison: conversations — GET /api/analytics/comparison/preset/conversation
- Invocation: authenticated client request.
- Inputs: current-period start/end (required), optional team filter.
- Behavior: runs the multi-metric comparison over a fixed conversation metric set (total/active/closed conversations, average resolution time, customer-satisfaction score) for the current period vs an auto-computed previous period.
- Success Output (HTTP 200): the multi-metric comparison object plus a metadata tag identifying the preset.
- Error Conditions: missing current-start/current-end → HTTP 400.

#### Preset comparison: messages — GET /api/analytics/comparison/preset/message
- Same shape as the conversation preset, over a fixed message metric set (total/customer/agent messages, average response time, messages per conversation). Missing required period bounds → HTTP 400.

#### Preset comparison: user activity — GET /api/analytics/comparison/preset/user-activity
- Same shape as the conversation preset, over a fixed user-activity metric set (active users, total activities, average session duration, user-engagement rate). Missing required period bounds → HTTP 400.

#### Comparison cache statistics — GET /api/analytics/comparison/cache/stats
- Invocation: authenticated client request.
- Behavior: loads and returns cache statistics for the comparison subsystem.
- Success Output (HTTP 200): the cache statistics object.

#### Dashboard module health — GET /api/analytics/dashboard/health
- Invocation: authenticated request (health is exempt from the analytics permission check).
- Success Output (HTTP 200): a status object (overall `healthy`, a timestamp, and per-subservice status flags).

#### List available widget types — GET /api/analytics/dashboard/widget-types
- Invocation: authenticated request with analytics view permission.
- Success Output (HTTP 200): a list of widget-type definitions describing the kinds of widgets that can be placed on a dashboard.

#### List dashboard templates — GET /api/analytics/dashboard/templates
- Invocation: authenticated request.
- Inputs: optional category filter (query string).
- Success Output (HTTP 200): a list of dashboard templates (optionally filtered by category), ordered by name.

#### List widget templates — GET /api/analytics/dashboard/widget-templates
- Invocation: authenticated request.
- Inputs: optional category filter and optional widget-type filter (query string).
- Success Output (HTTP 200): a list of matching widget templates.

#### Optimize dashboard layout — POST /api/analytics/dashboard/layout/optimize
- Invocation: authenticated request with JSON body.
- Inputs (body): optional dashboard identifier, optional container width (1–24).
- Preconditions & Authorization: caller role must be administrator or team-level; otherwise HTTP 403.
- Behavior: loads the dashboard configuration, recomputes widget placement to fit the container, persists the updated configuration, and returns it.
- Success Output (HTTP 200): the updated dashboard configuration with a refreshed update timestamp, plus a success message.
- Error Conditions: insufficient role → HTTP 403; other failures via the global error handler.

#### Get dashboard configuration — GET /api/analytics/dashboard/config/:dashboardId?
- Invocation: authenticated request.
- Inputs: optional dashboard identifier path segment (defaults to the caller's default dashboard).
- Behavior: returns the stored configuration for that dashboard scoped to the caller.
- Success Output (HTTP 200): a dashboard configuration object (identifier, name, optional description, layout descriptor, ordered widget list, refresh interval, auto-refresh flag, permission descriptor with owner/viewers/editors, created/updated timestamps, creator, optional tags and theme).

#### Save dashboard configuration — POST /api/analytics/dashboard/config/:dashboardId? and PUT /api/analytics/dashboard/config/:dashboardId?
- Invocation: authenticated request with JSON body (both verbs behave equivalently).
- Inputs (body, schema-validated): name (required, non-empty); optional description; layout object (type one of grid/flex/absolute/responsive, optional columns 1–24, optional rows numeric or `auto`, optional non-negative gap); widgets array; optional permissions (owner, viewers list, editors list); optional theme (`light`/`dark`); optional auto-refresh flag; optional refresh interval (minimum 5000 ms).
- Behavior: stores the configuration under the resolved identifier (path id, else body id, else `default`). Missing permissions default to the caller as owner with empty viewer/editor lists; refresh interval defaults to 30000 ms; auto-refresh defaults to true; created/updated timestamps are set/refreshed.
- Success Output (HTTP 200): the persisted configuration plus a success message.
- Error Conditions: schema-invalid body is rejected by the validator before handler logic runs.

#### Get dashboard data — GET /api/analytics/dashboard/data/:dashboardId?
- Invocation: authenticated request.
- Inputs: optional dashboard identifier; optional time-range parameter as a JSON-encoded object (query string).
- Behavior: loads the dashboard configuration and resolves data for every widget, scoped to the caller.
- Success Output (HTTP 200): a map keyed by widget identifier, each value being a widget-data object (widget id, type, a data payload that may be a time-series array / distribution array / comparison array / object, a loading flag, last-update timestamp, metadata, and optional value/labels/datasets/columns/rows/options/trend fields depending on widget type).
- Error Conditions: an unparsable time-range parameter returns HTTP 400 ("invalid timeRange parameter").

#### Get single widget data — GET /api/analytics/dashboard/widget/:widgetId/data
- Invocation: authenticated request.
- Inputs: widget identifier path segment; optional dashboard identifier and JSON-encoded time-range (query string).
- Behavior: locates the widget within the resolved dashboard and returns its current data.
- Success Output (HTTP 200): a single widget-data object.
- Error Conditions: unparsable time-range → HTTP 400; widget not present in the dashboard → HTTP 404.

#### Clone a widget — POST /api/analytics/dashboard/widget/:widgetId/clone
- Invocation: authenticated request with optional JSON body.
- Inputs (body, optional): a new widget identifier; a dashboard identifier.
- Preconditions & Authorization: caller role must be administrator or team-level; otherwise HTTP 403.
- Behavior: finds the source widget, produces a duplicate (optionally with the supplied new identifier), and returns it.
- Success Output (HTTP 200): the cloned widget plus a success message.
- Error Conditions: insufficient role → HTTP 403; source widget not found → HTTP 404.

#### Create dashboard from template — POST /api/analytics/dashboard/templates/:templateId/create
- Invocation: authenticated request with optional JSON body.
- Inputs: template identifier path segment; optional overrides (name, theme `light`/`dark`, auto-refresh flag, refresh interval ≥ 5000 ms).
- Behavior: instantiates a new dashboard configuration from the named template applying any overrides, scoped to the caller.
- Success Output (HTTP 200): the new dashboard configuration plus a success message.

#### Create widget from template — POST /api/analytics/dashboard/widget-templates/:templateId/create
- Invocation: authenticated request with optional JSON body.
- Inputs: template identifier; optional overrides (title, position {x,y,width,height}, data source {type among analytics/metrics/database/api/static/realtime, query string, config, parameters}, single metric, metric list).
- Preconditions & Authorization: caller role must be administrator or team-level; otherwise HTTP 403.
- Behavior: instantiates a widget from the named template applying overrides.
- Success Output (HTTP 200): the new widget plus a success message.

#### Create widget — POST /api/analytics/dashboard/widget
- Invocation: authenticated request with JSON body.
- Inputs (schema-validated): widget type (one of metric/chart/table/gauge/progress/status), non-empty title, data source (type as above, query string, optional config/parameters/cache), position {x≥0, y≥0, width≥1, height≥1}, optional metric/metrics/filters/real-time flag/refresh interval (≥5000 ms).
- Preconditions & Authorization: caller role must be administrator or team-level; otherwise HTTP 403.
- Success Output (HTTP 200): the created widget plus a success message.

#### Update widget — PUT /api/analytics/dashboard/widget/:widgetId
- Invocation: authenticated request with JSON body (partial widget config permitted).
- Preconditions & Authorization: caller role must be administrator or team-level; otherwise HTTP 403.
- Success Output (HTTP 200): the updated widget plus a success message.

#### Broadcast dashboard update — POST /api/analytics/realtime/broadcast
- Invocation: authenticated request with JSON body (this path requires the bearer-token middleware).
- Inputs (schema-validated): dashboard identifier (required), optional widget identifier, broadcast type (`widget_update` or `config_change`), and a data payload.
- Preconditions & Authorization: caller role must be administrator or team-level; otherwise HTTP 403.
- Behavior: for `widget_update`, dispatches a widget-update event for the given widget (the widget identifier is mandatory for this type); for `config_change`, dispatches a configuration-change event for the dashboard. Real-time fan-out to connected clients is handled by the separate real-time transport layer.
- Success Output (HTTP 200): a success flag and a confirmation message naming the broadcast type.
- Error Conditions: `widget_update` without a widget identifier → error code indicating a missing widget id (HTTP 400). An unsupported type → error code indicating an unsupported broadcast type (HTTP 400). Insufficient role → HTTP 403.

#### Trigger single widget refresh — POST /api/analytics/realtime/trigger-update/:dashboardId/:widgetId
- Invocation: authenticated request.
- Behavior: loads the dashboard, finds the widget, recomputes its data, and dispatches a widget-update broadcast.
- Success Output (HTTP 200): a success flag, the recomputed widget data, and a message.
- Error Conditions: widget not found → a failure response with HTTP 404.

#### Trigger whole-dashboard refresh — POST /api/analytics/realtime/trigger-update/:dashboardId
- Invocation: authenticated request.
- Behavior: recomputes data for the entire dashboard and dispatches a widget-update broadcast for every widget.
- Success Output (HTTP 200): a success flag, the full dashboard data map, a message, and the list of updated widget identifiers.

#### Get real-time connection status — GET /api/analytics/realtime/status
- Invocation: authenticated request.
- Preconditions & Authorization: administrator only; otherwise a failure response with HTTP 403.
- Success Output (HTTP 200): a success flag, a status object (total connection count, a map of connections per dashboard, a map of connections per user), and a timestamp. (In the current transport architecture these counts reflect the externally managed connection layer.)

#### Real-time module health — GET /api/analytics/realtime/health
- Invocation: authenticated request.
- Success Output (HTTP 200): an overall `healthy` status, a timestamp, a service label, a total-connection count, and a metrics block (total connections, distinct-dashboard count, distinct-user count).

#### Clean up expired connections — POST /api/analytics/realtime/cleanup
- Invocation: authenticated request.
- Preconditions & Authorization: administrator only; otherwise a failure response with HTTP 403.
- Behavior: triggers cleanup of stale real-time connections.
- Success Output (HTTP 200): a success flag and a message.

#### Security dashboard health — GET /api/security/dashboard/health
- Invocation: public (no authentication required).
- Success Output (HTTP 200): a success flag, a status object (status, module label, version), and a timestamp.

#### Security dashboard metrics — GET /api/security/dashboard/metrics
- Invocation: authenticated request.
- Inputs (query string): time range, one of `1h`/`24h`/`7d`/`30d`, default `24h`.
- Preconditions & Authorization: administrator role only; otherwise HTTP 403.
- Behavior: gathers recent webhook-security events and recent cross-origin (CORS) events within the chosen window and computes a comprehensive security overview. The window is converted to hours (1, 24, 168, or 720; unknown values default to 24). At most the most recent 1000 events of each kind are considered.
- Success Output (HTTP 200): a metrics object with five sections:
  - summary: total events, counts per severity (critical/high/medium/low), an events-per-hour rate (total events divided by hours, rounded), and a top-threats list (up to five event types by count, each with type, count, severity).
  - webhook security: total events, count maps by platform / by type / by severity, and a most-recent-events list (up to ten; each with id, type, severity, platform, source IP or null, timestamp).
  - CORS monitoring: total events, allowed-request count, rejected-request count, a top-rejected-origins list (up to five by count), and a recent-rejections list (up to ten; each with origin, path, timestamp).
  - trends: an hourly distribution (each hour-bucket with count and critical count, kept to the last 24 buckets) and a platform distribution (each with platform, count, percentage of total).
  - alerts: a count of high/critical events treated as alerts, an alerts-by-channel map (email/slack/webhook), and a (currently empty) recent-alerts list.
- Error Conditions: non-admin → HTTP 403; invalid time range → HTTP 400; processing failure → HTTP 500 with an error message.

#### Recent security events — GET /api/security/dashboard/events/recent
- Invocation: authenticated request.
- Inputs (query string): limit, default 50, capped at 200.
- Preconditions & Authorization: administrator role only; otherwise HTTP 403.
- Behavior: returns the most recent security events drawn from both webhook-security and CORS sources, merged and sorted newest-first, truncated to the requested limit.
- Success Output (HTTP 200): an object with an events array, a count, and the applied limit. Each event carries an id, a type, a category (`webhook` or `cors`), an optional severity, an optional platform, an optional origin, a timestamp, and a metadata object (webhook events expose source IP, integration identifier, and parsed details; CORS events expose method, path, user-agent, and parsed metadata).
- Error Conditions: non-admin → HTTP 403; a non-numeric or sub-1 limit → HTTP 400; processing failure → HTTP 500.

#### Security summary — GET /api/security/dashboard/summary
- Invocation: authenticated request.
- Preconditions & Authorization: administrator role only; otherwise HTTP 403.
- Behavior: computes the full 24-hour metrics set and returns a condensed view.
- Success Output (HTTP 200): the summary section, a webhook summary (total, by-platform map, the single top event type), and a CORS summary (total, allowed, rejected).
- Error Conditions: non-admin → HTTP 403; processing failure → HTTP 500.

### Data Concepts (neutral)
- Time window selector: a named relative range mapped to a concrete start/end pair relative to "now". Explicit start/end timestamps, when both present, take precedence. The named range also drives the reported aggregation granularity (raw / hourly / daily / weekly / monthly) and the bucketing of trend series.
- Conversation insight record: aggregate counts and averages over customer conversations (totals, active/closed counts, durations, response and resolution times, satisfaction, and breakdowns by channel/team/priority), plus the resolved reporting period.
- Message insight record: aggregate message counts, throughput rate, average response time, and distributions across message type, channel, and sentiment.
- User/agent insight record: counts of total and active users, session and activity averages, per-user performance scores (with handled-conversation counts, response times, satisfaction, resolution rate), and per-user workload (active conversations, daily messages, utilization, working hours).
- Performance insight record: average response time, throughput, error rate, uptime and system load, with derived bottleneck and recommendation lists.
- Numeric metric point: a named, timestamped numeric sample carrying a string-keyed tag map, an optional unit, and optional metadata; used as the raw material for time-bucketed aggregation and querying. Conceptually retained per a configurable retention policy by granularity.
- Aggregated metric bucket: a name, applied aggregation function, computed value, bucket timestamp, period label, tag map, and sample count.
- Comparison result: current value, prior value, absolute and percentage change, a trend classification, and the two compared periods (each with start/end and a human-readable label).
- Dashboard: an owned, named, scoped configuration carrying a layout descriptor, an ordered list of widgets, refresh settings, a permission descriptor (owner, viewers, editors), timestamps, and optional theme/tags.
- Widget: a typed visual element (metric/chart/table/gauge/progress/status) with a title, position/size, a data source (typed query against analytics/metrics/database/api/static/realtime), and optional metric bindings, filters, real-time flag, and refresh interval.
- Security event records: webhook-security events (id, type, severity, platform, source IP, integration reference, structured details, creation timestamp) and cross-origin request events (id, type allowed/rejected, origin, method, path, user-agent, structured metadata, timestamp). All counts/aggregates over these are derived, not stored.
- Soft-delete semantics: conversation-based comparison computations explicitly exclude records marked as deleted; engagement-rate computations count only active, non-deleted agents.

### State & Lifecycle
- Trend direction (comparison): `up` / `down` / `stable`, with `stable` assigned when the percentage change magnitude is below 5%. A zero prior value yields a reported percentage change of 100 (if current > 0) or 0.
- Overall multi-metric verdict: `positive` / `negative` / `neutral`, derived from how many metrics improved versus declined and whether either reaches at least half of the metric set.
- Export artifact lifecycle: created → advertised as valid for 24 hours → expired (the response carries explicit generated-at and expires-at markers and an initial zero download count).
- Conversation/message statuses referenced in comparison computation: conversations are counted as `active` or `closed` for the respective metrics; resolution/response averages consider only conversations that reached a closed/first-response state.

### Real-time / Event Behavior
- The real-time dashboard control endpoints emit two event kinds toward connected dashboard subscribers: a widget-update event (carrying a dashboard identifier, widget identifier, and the recomputed widget data) and a configuration-change event (carrying a dashboard identifier and the changed configuration). These are dispatched in response to explicit broadcast or trigger-update requests; the actual delivery to clients is performed by the separate real-time transport layer rather than by these HTTP handlers directly.
- Whole-dashboard trigger emits one widget-update event per widget on the dashboard.
- Connection-status and health endpoints expose counts of currently connected real-time clients (total, per dashboard, per user) sourced from the external transport layer.
- The security dashboard does not push events from this area; clients poll the recent-events and metrics endpoints. (A separate real-time event stream for security exists outside this area's HTTP handlers.)
- Authorization audience rules across this area: administrators may read all data and perform all dashboard/broadcast/status/cleanup actions; team-level users may create/clone/update/optimize widgets and broadcast but are scoped to their team's data on reads; agents have read-only access scoped to their own team and their own user activity; connection-status, cleanup, and all security-dashboard data endpoints are administrator-only.

## Reports

### Purpose
This area provides a reporting subsystem for a multi-channel customer-support platform. Authenticated staff can generate analytical reports about conversations, agent performance, and message activity; download generated report files; list, inspect, and delete past reports; preview report layouts with synthetic sample data before committing to generation; view aggregate usage statistics; perform bulk actions over multiple reports; and configure recurring (scheduled) report jobs that run automatically and can notify recipients. Many report "types" are advertised as a catalog of business report categories, but only a small subset is actually backed by live data queries; the remainder are reserved/template-only and are rejected at generation time while still being usable for preview/catalog purposes.

All endpoints are mounted under a common base path of `/api/reports`. All routes in this section are relative to that base.

### Operations

#### Health probe — GET /api/reports/health
- Invocation: unauthenticated client request.
- Inputs: none.
- Preconditions & Authorization: none (no auth required).
- Behavior: returns a static liveness indicator.
- Success Output (200): an object with a literal health status string, a module identifier, a server timestamp, and a version string.
- Error Conditions: none expected.

#### Module catalog/info — GET /api/reports/info
- Invocation: unauthenticated client request.
- Inputs: none.
- Preconditions & Authorization: none.
- Behavior: returns static descriptive metadata about the reporting capability.
- Success Output (200): `success=true` plus a data object containing: module identifier, version, human description, a feature list, a list of the report types that are actually backed by production data (each as a code plus its display name), an enumeration of available endpoints, and a description of permission tiers (administrator, team-scoped, agent). Includes a server timestamp.
- Note: the advertised list of generatable report types is limited to a small subset of the full report-type catalog (see Data Concepts). Advertised export formats are likewise a small subset.

#### Generate a report — POST /api/reports
- Invocation: authenticated client request.
- Inputs (JSON body):
  - report type code (required) — must be one of the full report-type catalog; additionally must be a member of the small "generatable" subset or generation fails downstream.
  - title (required, non-empty, max 200 chars after sanitization).
  - description (optional, max 1000 chars after sanitization).
  - output format (required) — must be one of the catalog formats AND must be supported by the chosen report type AND must be in the "generatable" formats subset to actually produce a file.
  - time range (required) — one of a fixed set of named ranges, or the literal "custom".
  - custom range start/end (required only when time range is custom; ISO-8601 date-times).
  - filters (optional) — see Filters in Data Concepts; arrays are capped (team identifiers max 10, agent identifiers max 50, tag list max 20; priority values restricted to a fixed set).
  - options (optional) — record cap between 1 and 100000; chart-type restricted to a fixed set.
- Preconditions & Authorization: valid bearer token; caller must hold "create report" permission. Additionally, certain report types (system-health, custom, team-analytics) are restricted to administrators only. Request body size capped at 2 MB. Subject to rate limiting.
- Behavior (observable order): request is validated; the caller's create permission and any special-type restriction are enforced; a concurrent-generation cap is enforced (a caller may not have more than a fixed number of reports simultaneously in the "generating" state); a new report record is created in "pending" state; it transitions to "generating"; underlying data is queried and serialized into the chosen format; the resulting file is stored; the record transitions to "completed" with a recorded file size, execution time, and a download path. On any failure during the pipeline the record transitions to "failed" with an error message recorded.
- Success Output (201): `success=true`, a data object representing the finished report (identifier, title, type, format, status, creator identifier, timestamps, download path, file size, etc.), a confirmation message, an estimated-time string derived from the report type's nominal generation time, and a server timestamp.
- Side Effects: a persisted report entry progressing through its lifecycle; a stored report file object; an operation audit log line.
- Error Conditions:
  - Missing/invalid bearer token -> 401 with an auth error.
  - Insufficient create permission -> 403.
  - Restricted report type requested by non-administrator -> 403.
  - Body over size limit -> 413.
  - Rate limit exceeded -> 429 (with retry-after and rate-limit headers).
  - Missing/invalid type, missing title, missing/unsupported format, missing/invalid time range, missing custom dates, custom start not before end, custom end in the future, over-length title/description, oversized filter arrays, invalid priority, invalid chart type, invalid JSON -> 400 with a specific message.
  - Report type valid in catalog but not in the generatable subset, or format valid but not in generatable formats subset -> generation rejected (surfaced as an invalid-parameters failure).
  - Server failure -> 500.

#### List reports — GET /api/reports
- Invocation: authenticated client request.
- Inputs (query params, all optional): report type, status (pending/generating/completed/failed/expired), format, creator identifier (must be UUID), start date, end date (ISO-8601), page (1–1000, default 1), page size (1–100, default 20), sort field (restricted to a fixed set such as creation time, completion time, title, type, status, format), sort order (asc/desc).
- Preconditions & Authorization: valid token; caller must hold "read report" permission.
- Behavior: returns a filtered, paginated list ordered by most recent first, plus pagination metadata and a summary count block.
- Success Output (200): `success=true`, data containing an array of report summaries, a pagination object (current page, page size, total count, total pages, has-next, has-prev), and a summary object (total, pending, completed, failed counts), and a server timestamp.
- Error Conditions: invalid query value -> 400; missing/invalid token -> 401; insufficient permission -> 403; server failure -> 500.
- Notes: soft-deleted reports remain countable in some summary paths but excluded where deletion filtering is applied (see soft-delete in Data Concepts).

#### Get report details — GET /api/reports/:id
- Invocation: authenticated client request.
- Inputs: path identifier (must be a UUID, or a legacy "report_"-prefixed UUID).
- Preconditions & Authorization: valid token; "read report" permission.
- Behavior: looks up a single report and returns an extended detail view.
- Success Output (200): `success=true`, data containing the report fields plus supplementary detail fields (a generation log, an execution-time value, a data-source descriptor including a record count, and a download-history list). Some of these supplementary detail fields are reported as fixed illustrative values within the current behavioral boundary rather than live values.
- Error Conditions: missing identifier or malformed identifier -> 400; not found -> 404; missing/invalid token -> 401; insufficient permission -> 403; server failure -> 500.

#### Download a report file — GET /api/reports/:id/download
- Invocation: authenticated client request.
- Inputs: path identifier (UUID or legacy-prefixed UUID).
- Preconditions & Authorization: valid token; caller must hold "export report" permission; additionally the caller must be the report's creator, OR an administrator, OR a member of the report's owning team. Subject to rate limiting.
- Behavior: verifies ownership/team access, confirms the report is completed and has an available file, then streams the stored file back as an attachment. A download-history entry is recorded.
- Success Output (200): the raw file body with content-type matching the format, a content-disposition attachment header whose filename derives from a sanitized report title plus an extension, a private short-lived cache header, a no-sniff header, and a content-length when known.
- Side Effects: a persisted download-history record (timestamp + downloading user); an operation audit log line.
- Error Conditions: malformed identifier -> 400; report not found, not completed, or backing file missing -> 404; access denied (not owner/admin/team member) -> surfaced as access-denied; missing/invalid token -> 401; rate limit exceeded -> 429; server failure -> 500.

#### Delete a report — DELETE /api/reports/:id
- Invocation: authenticated client request.
- Inputs: path identifier (UUID or legacy-prefixed UUID).
- Preconditions & Authorization: valid token; caller must hold "delete report" permission; additionally a caller may only delete their own reports unless they are an administrator. Subject to rate limiting.
- Behavior: verifies ownership, soft-deletes the report (marks a deletion timestamp), and removes the associated stored file objects.
- Success Output (200): `success=true`, a confirmation message, server timestamp.
- Side Effects: report marked soft-deleted; stored file objects removed; audit log line.
- Error Conditions: malformed identifier -> 400; not found or undeletable -> 404; access denied (not owner/admin) -> access-denied; missing/invalid token -> 401; insufficient permission -> 403; rate limit exceeded -> 429; server failure -> 500.

#### Report statistics — GET /api/reports/stats
- Invocation: authenticated client request.
- Inputs: optional time-range query param (defaults to the last-30-days named range when absent or unrecognized).
- Preconditions & Authorization: valid token; administrator role ONLY (non-administrators are rejected even if they hold other report permissions).
- Behavior: returns aggregate usage statistics over the chosen window, excluding soft-deleted reports.
- Success Output (200): `success=true`, data containing: total report count; counts broken down by report type, by format, and by status (all keys zero-initialized across the full catalogs); average generation time; a top-N "popular reports" list (type, count, average size); a per-user usage list (user identifier, display name, report count, last-generated time); and a monthly-trend list (month label, count generated, total size). Server timestamp included.
- Error Conditions: non-administrator -> 403; missing/invalid token -> 401; server failure -> 500.

#### Batch operation — POST /api/reports/batch
- Invocation: authenticated client request.
- Inputs (JSON body): a non-empty list of report identifiers (each must be a UUID; list capped at 50), an action (one of delete, regenerate, download, export), and optional options (a format restricted to the catalog set, and a boolean merge flag).
- Preconditions & Authorization: valid token; administrator role ONLY. Body size capped; rate limited.
- Behavior: processes each identifier independently and records a per-item result. For delete it soft-deletes (with the same ownership rules); for regenerate it re-runs generation reusing the original report's stored parameters; for download/export it resolves a download reference. Aggregates per-item successes/failures.
- Success Output (200): `success=true`, data containing an overall success flag (true only if no item failed), total requested count, success count, failed count, and a per-item results array (identifier, success flag, optional error message, optional download reference). Includes a message naming the action and a server timestamp.
- Side Effects: depending on action — soft-deletions, new generated reports, download-history entries; audit log line.
- Error Conditions: empty/oversized list -> 400; malformed identifier in list -> 400; invalid action -> 400; invalid options -> 400; non-administrator -> 403; missing/invalid token -> 401; over size -> 413; rate limit -> 429; server failure -> 500. Individual items that fail (e.g., not found) do not fail the whole request; they appear as failed entries.

#### Get report templates — GET /api/reports/templates/:type
- Invocation: authenticated client request.
- Inputs: path report-type code.
- Preconditions & Authorization: valid token; "read report" permission.
- Behavior: returns predefined option-presets ("templates") for the given report type. Only a couple of report types have presets; others return an empty list.
- Success Output (200): `success=true`, data containing a list of templates (each: name, description, an options object), the echoed report type, and a server timestamp.
- Error Conditions: report type not in the catalog -> 400; missing/invalid token -> 401; insufficient permission -> 403; server failure -> 500.

#### Preview a report — POST /api/reports/preview
- Invocation: authenticated client request.
- Inputs (JSON body): report type code (required, must be in the catalog) and a time range (required). Other generation fields are accepted but not required.
- Preconditions & Authorization: valid token; "read report" permission. Body size capped.
- Behavior: returns representative SAMPLE/synthetic data for the report type without touching live data or persisting anything. Sample payloads are generated fresh (some numeric fields are randomized) and are available for a broad set of report types; for any type without a sample generator, a "preview not available" message is returned instead.
- Success Output (200): `success=true`, a data field containing the sample payload (shape depends on report type — see Data Concepts) or a not-available message object, a confirmation message, and a server timestamp.
- Side Effects: none (no persistence, no file, no events).
- Error Conditions: missing/invalid type -> 400; missing time range -> 400; invalid JSON -> 400; missing/invalid token -> 401; insufficient permission -> 403; over size -> 413; server failure -> 500.

#### Create a scheduled report — POST /api/reports/scheduled
- Invocation: authenticated client request.
- Inputs (JSON body): name (required, non-empty, max 200 after sanitization); description (optional, max 1000); report type (required, in catalog); format (required, must be supported by the type); a schedule object — frequency (daily/weekly/monthly/quarterly), time in HH:mm 24-hour format, day-of-week 0–6 (required for weekly), day-of-month 1–31 (required for monthly); optional filters; optional options; optional recipient list (each requires a valid email and a non-empty name; list capped at 20).
- Preconditions & Authorization: valid token; caller must hold "schedule report" permission. Body size capped; rate limited.
- Behavior: validates the schedule, computes the next run time from frequency/time/day settings, persists an active scheduled-report definition owned by the caller.
- Success Output (201): `success=true`, data representing the created scheduled report (server-assigned identifier, name, type, format, schedule, filters, options, recipients, active flag, creator, creation time, computed next-run time), confirmation message, server timestamp.
- Side Effects: persisted scheduled-report definition; audit log line.
- Error Conditions: missing name/type/format/schedule, unsupported format for type, invalid frequency, invalid time format, missing/invalid day-of-week or day-of-month, over-length fields, too many recipients, invalid recipient email or missing recipient name, invalid JSON -> 400; missing/invalid token -> 401; insufficient permission -> 403; over size -> 413; rate limit -> 429; server failure -> 500.

#### List scheduled reports — GET /api/reports/scheduled
- Invocation: authenticated client request.
- Inputs: none.
- Preconditions & Authorization: valid token; "schedule report" permission. Administrators see all scheduled reports; other roles see only those they created.
- Behavior: returns non-deleted scheduled definitions, ordered by upcoming next-run time, scoped per the role rule above.
- Success Output (200): `success=true`, data array of scheduled-report definitions, a count of returned items, server timestamp.
- Error Conditions: missing/invalid token -> 401; insufficient permission -> 403; server failure -> 500.

#### Update a scheduled report — PUT /api/reports/scheduled/:id
- Invocation: authenticated client request.
- Inputs: path identifier (must be a UUID); JSON body with the same fields and validation as creation (re-validated each time).
- Preconditions & Authorization: valid token; "schedule report" permission; the caller must be the definition's creator (otherwise not authorized). Body size capped; rate limited.
- Behavior: applies the provided changes; if the schedule changed, recomputes the next run time; updates the modification timestamp; returns the updated definition.
- Success Output (200): `success=true`, the updated definition, confirmation message, server timestamp.
- Error Conditions: malformed identifier -> 400; same validation failures as creation -> 400; definition not found, or caller not the creator -> surfaced as an operation failure; missing/invalid token -> 401; insufficient permission -> 403; over size -> 413; rate limit -> 429; server failure -> 500.

#### Delete a scheduled report — DELETE /api/reports/scheduled/:id
- Invocation: authenticated client request.
- Inputs: path identifier (UUID).
- Preconditions & Authorization: valid token; "schedule report" permission; caller must be the creator. Rate limited.
- Behavior: soft-deletes the definition (marks a deletion timestamp and deactivates it).
- Success Output (200): `success=true`, confirmation message, server timestamp.
- Side Effects: definition soft-deleted and deactivated; audit log line.
- Error Conditions: malformed identifier -> 400; not found or undeletable -> 404; caller not the creator -> not-authorized failure; missing/invalid token -> 401; insufficient permission -> 403; rate limit -> 429; server failure -> 500.

#### Scheduled report execution
- Invocation: triggered automatically when scheduled report definitions become due.
- Behavior: finds all active, non-deleted scheduled definitions whose next-run time is due, and for each: records an execution attempt in "running" state, generates a report (always over a last-24-hours window, with a date-stamped title derived from the definition name), and on success marks the execution "success", links the generated report, computes and stores the next run time, increments an execution counter, and records last-run metadata. On failure it records the execution as failed with the error; if a retry ceiling is reached the definition is deactivated, otherwise the next run is rescheduled after a retry delay.
- Observable result: callers later observe new generated reports attributable to the schedule, advancing next-run times, last-run status, and (eventually) automatic deactivation of repeatedly-failing schedules.

### Data Concepts (neutral)

- **Report**: a generated analytical artifact. Carries a unique identifier (a UUID for new records; a legacy prefixed-UUID form is also accepted for older records), a title, optional description, a type code, an output format, a lifecycle status, the creating user's identifier, an optional owning-team reference, a set of timestamps (created, updated, generation-started, completed, expiry), an optional download path, an optional file size, an optional error message, and a metadata bag preserving the original generation parameters (time range, custom dates, filters, options). Reports expire after a fixed retention window (about 30 days). Reports are soft-deleted (a deletion timestamp is set; the record is retained).
- **Report type catalog**: a fixed set of ~24 business report categories spanning operational, enterprise, business-intelligence, and advanced-analytics groupings (e.g., conversation summary, agent performance, team analytics, customer satisfaction, platform usage, message statistics, response-time analysis, workload distribution, system health, custom, cost analysis, SLA compliance, anomaly detection, audit trail, resource utilization, trend forecast, customer insights, channel integration, goal achievement, automation effectiveness, security risk, knowledge base, call quality, executive summary). Each catalog entry carries a display name, a description, a list of supported output formats, a nominal generation-time estimate, and a list of required permission strings. Only a SMALL subset (conversation summary, agent performance, message statistics) is actually backed by live data and thus eligible for real generation; all others are accepted in the catalog (and may be previewed) but rejected at generation time.
- **Output formats**: a fixed set (structured-data, delimited-text, spreadsheet, document, web-markup). Each report type declares which formats it supports. Only two formats (structured-data and delimited-text) are backed by real serializers; requesting any other for actual generation is rejected even if the type "supports" it.
- **Filters**: an optional descriptor that may scope a report by teams, agents, customers, conversations, platforms, message types, priority levels (restricted set), tags, and arbitrary custom fields. Array sizes are capped.
- **Options**: optional rendering/processing preferences (include-charts/summary/details/raw-data flags, chart type from a restricted set, grouping, sort field/order, max records 1–100000, timezone, language).
- **Statistics view**: a derived, read-only aggregate (totals, breakdowns by type/format/status, average generation time, popular reports, per-user usage, monthly trends) computed over a chosen time window, excluding soft-deleted reports.
- **Scheduled report definition**: a recurring job carrying a unique identifier, a name, optional description, a target report type and format, a schedule (frequency, time-of-day, optional day-of-week/day-of-month), filters, options, a recipient list (email/name/role), an active flag, the creator's identifier, creation time, last-run time, and a computed next-run time. Soft-deleted (deletion timestamp + deactivation). Persisted retry settings (retry ceiling and retry delay) govern automatic execution behavior.
- **Scheduled execution record**: an attempt log for a scheduled definition, carrying its own identifier, the parent definition reference, start/completion timestamps, a status (running/success/failed), a duration, a link to any generated report, an error message on failure, and a retry counter.
- **Download-history entry**: a record that a particular user downloaded a particular report at a particular time (optionally with client metadata).
- **Sample/preview payloads**: per-type synthetic datasets produced solely for preview. Their shapes are rich and type-specific — e.g., conversation summaries carry period bounds, total/active/completed counts, average response/resolution times, breakdowns by platform/priority/team, hourly distribution, daily trends, and top tags; agent-performance carries agent and team metric arrays plus performance trends; other catalog types carry their own domain-specific structures (cost breakdowns, SLA compliance metrics and breach analysis, audit summaries and security incidents, anomaly detections, resource utilization, forecasts, customer insights, channel integration, goal achievement, automation effectiveness, security risk, knowledge-base efficacy, call quality, executive summary). Some numeric fields are randomized per request, so preview output is not stable across calls.

### State & Lifecycle

**Report status states**: pending -> generating -> completed, OR pending/generating -> failed. A separate "expired" state exists conceptually (tied to the retention window). Transitions are driven internally by the generation pipeline; clients do not set status directly. A completed report becomes downloadable; a failed report carries an error message. Soft-deletion is orthogonal: any report may be marked deleted (removing its backing files) regardless of status, subject to ownership rules.

**Scheduled definition states**: created active -> may be updated (re-validated, next-run recomputed if schedule changes) -> may be soft-deleted (deactivated). Automatic execution may deactivate a definition after its retry ceiling is exhausted. Only the creator may update or delete a definition; administrators may additionally view all definitions.

### Real-time / Event Behavior
This area does not emit WebSocket/real-time client events. Its only time-driven behavior is the internal scheduled-execution process (above), triggered by a platform scheduler rather than by clients. Observable downstream effects of that process — newly generated reports, advancing next-run times, updated last-run status, and automatic deactivation of repeatedly-failing schedules — become visible through the standard list/detail/statistics endpoints. Optional recipient notification on completion/failure is modeled in the scheduled-definition data (recipient list and notify flags) but is delivered out-of-band rather than as a real-time channel event.

### Invariants & Guarantees
- Authentication is by bearer token on all operations except the health and info probes.
- Permission model is tiered: generic create/read/export/delete/schedule permissions plus hard administrator-only gates on statistics, batch operations, and the restricted report types (system-health, custom, team-analytics).
- Ownership scoping: download requires creator/admin/team-member; delete and scheduled update/delete require creator (or admin for delete of reports).
- A per-user concurrency cap limits how many reports may be generating simultaneously.
- Mutating/expensive endpoints are rate-limited per a fixed window (about 30 requests/minute) and return standard rate-limit headers plus a retry-after on exceedance.
- Request bodies are capped (about 2 MB) for write/preview/batch/scheduled endpoints.
- String inputs (titles, descriptions, names) are sanitized (angle brackets, script-protocol, and inline event handlers stripped) and length-capped.
- Deletions are soft (a deletion timestamp is set); backing report files are physically removed on report delete.
- Batch operations are best-effort per item: a per-item failure does not abort the batch, and the overall success flag is true only when every item succeeded.
- Report identifiers are UUIDs (with backward-compatible acceptance of a legacy prefixed form); scheduled-report identifiers must be strict UUIDs.

## Monitoring & Health

### Purpose
This area provides operational visibility and control over the platform's runtime health. It exposes a public liveness probe, authenticated dashboards of system component health, real-time per-endpoint API traffic metrics, infrastructure-instance health monitoring (the real-time messaging/connection backend), threshold-based alerting, and manual operator controls over an emergency traffic circuit breaker. It also defines the conceptual data points and alert conditions that downstream dashboards and external alert channels consume.

> Routing note: For `GET /api/monitoring/metrics` and `GET /api/monitoring/alerts`, the infrastructure-monitoring behavior is what callers observe at those paths; the dashboard-oriented variants of those two paths are not reachable. All other dashboard paths are reachable. The behavioral spec below documents the reachable behavior per path.

---

### Operations

#### Public liveness / system health probe — GET /api/monitoring/health
- Invocation: Unauthenticated client request. No credentials required.
- Inputs: None.
- Preconditions & Authorization: Public. Anyone may call.
- Behavior: On each call, performs a fresh on-demand health sweep of the real-time infrastructure instances (messaging broadcaster, conversation rooms, user connections) and reads the current circuit-breaker state. Computes an aggregate status: reports `healthy` when at least 70% of discovered instances are healthy, otherwise `degraded`. Returns a composite snapshot. This endpoint is explicitly excluded from API traffic metrics collection (to avoid self-measurement feedback loops).
- Success Output: A JSON object containing: top-level `status` (`healthy` | `degraded`); `timestamp` (epoch milliseconds); a `components` group with (a) infrastructure-instance health (`status` of `healthy`/`degraded`, plus total/healthy/degraded/unhealthy instance counts), (b) circuit-breaker `status` (current state) and its statistics object, and (c) `alerts` with active and total counts; and a `summary` with total instance count, a breakdown of instance counts by infrastructure type, and a last-update timestamp. HTTP `200` when `healthy`; HTTP `207` (multi-status) when `degraded`.
- Side Effects: None persisted. May add warning/critical alert records to the rolling alert history as a side effect of the health sweep (see Alerting). No real-time events emitted to clients.
- Error Conditions: On server failure returns HTTP `500` with `{ status: "error", error: <message>, timestamp }`.
- Invariants & Guarantees: Each call triggers a fresh sweep; results are not cached between calls. Instance discovery is best-effort — instances that cannot be reached are counted as unhealthy rather than failing the whole probe.

#### Infrastructure metrics detail — GET /api/monitoring/metrics
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires a valid session token AND the caller's system role must be administrator. Non-admins are rejected.
- Behavior: Performs a fresh health sweep, then returns detailed per-instance metrics for every discovered real-time infrastructure instance plus circuit-breaker state and its most recent events.
- Success Output: HTTP `200` with: `timestamp`; an infrastructure group listing each instance (type, identifier, health status, active connection count, average latency, error rate, memory usage, uptime) plus a summary (total instance count, count-by-type map, average latency across instances, total active connections); and a circuit-breaker group (current state, statistics, and the most recent events, capped at the latest 20).
- Side Effects: Triggers a fresh health sweep (same side effects as the liveness probe).
- Error Conditions: Missing/invalid auth → standard auth rejection. Authenticated non-admin → HTTP `403` with `{ error: "Admin access required" }`. Server failure → HTTP `500` with `{ error, reason }`.

#### Active alerts list — GET /api/monitoring/alerts
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires a valid session token. Any authenticated role may call (no admin restriction on the reachable handler).
- Behavior: Returns all alerts currently attached to live infrastructure instances (i.e. conditions presently breaching thresholds at last sweep). Does not perform a new sweep; reflects state from the most recent sweep.
- Success Output: HTTP `200` with `{ count, alerts: [ { type, severity, message, timestamp, age (ms since raised), metadata } ], timestamp }`.
- Side Effects: None.
- Error Conditions: Missing/invalid auth → auth rejection. Server failure → HTTP `500` with `{ error, reason }`.

#### Infrastructure alert history — GET /api/monitoring/alerts/history
- Invocation: Authenticated client request.
- Inputs: `limit` (query, optional integer, default 100) — maximum number of most-recent historical alert records to return.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Returns the most recent alert records from the rolling alert history (history retains roughly the last hour of alerts).
- Success Output: HTTP `200` with `{ count, limit, alerts: [...], timestamp }`.
- Error Conditions: Non-admin → HTTP `403` `{ error: "Admin access required" }`. Server failure → HTTP `500` `{ error, reason }`.

#### Circuit-breaker status — GET /api/monitoring/circuit-breaker/status
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires valid session token. Any authenticated role.
- Behavior: Reads and returns the current circuit-breaker state and statistics without modifying it.
- Success Output: HTTP `200` with `{ state, stats, timestamp }`.
- Error Conditions: Auth rejection if unauthenticated. Server failure → HTTP `500` `{ error, reason }`.

#### Circuit-breaker manual reset — POST /api/monitoring/circuit-breaker/reset
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Resets the circuit breaker to its normal/closed operating state, allowing traffic to flow again.
- Success Output: HTTP `200` with `{ success: true, message: "Circuit breaker reset successfully", newState, timestamp }`.
- Side Effects: Changes global traffic-gating state observable by all subsequent requests routed through the breaker. Records an administrative log entry including the acting user identifier.
- Error Conditions: Non-admin → HTTP `403`. Server failure → HTTP `500` `{ error, reason }`.

#### Circuit-breaker emergency open — POST /api/monitoring/circuit-breaker/open
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Forces the circuit breaker open (emergency stop), causing traffic gated by the breaker to be short-circuited/rejected until reset.
- Success Output: HTTP `200` with `{ success: true, message: "Circuit breaker opened (emergency stop)", newState, timestamp }`.
- Side Effects: Changes global traffic-gating state. Records a critical-level administrative log entry including the acting user identifier.
- Error Conditions: Non-admin → HTTP `403`. Server failure → HTTP `500` `{ error, reason }`.

#### Infrastructure instances by type — GET /api/monitoring/instances/:type
- Invocation: Authenticated client request.
- Inputs: Path parameter `type` (required string) — an infrastructure instance category. Recognized categories include the real-time conversation room, the user-connection holder, the message broadcaster, and the delayed-message processor.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Performs a fresh health sweep, then filters and returns metrics for instances matching the requested category.
- Success Output: HTTP `200` with `{ type, count, instances: [ { id, status, connections, latency, errorRate, uptime, lastActivity, alerts } ], timestamp }`. An unrecognized `type` yields an empty list with count 0 (not an error).
- Side Effects: Triggers a fresh sweep.
- Error Conditions: Non-admin → HTTP `403`. Server failure → HTTP `500` `{ error, reason }`.

#### Manual infrastructure health check — POST /api/monitoring/health-check
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Forces an immediate full health sweep and returns the resulting aggregate statistics.
- Success Output: HTTP `200` with `{ success: true, stats: { totalInstances, instancesByType, healthyInstances, degradedInstances, unhealthyInstances, totalAlerts, activeAlerts, lastUpdate }, timestamp }`.
- Side Effects: May raise alerts into history during the sweep.
- Error Conditions: Non-admin → HTTP `403`. Server failure → HTTP `500` `{ error, reason }`.

#### Monitoring dashboard summary — GET /api/monitoring/dashboard
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Aggregates the application-level health monitor's rolling statistics with the current component/infrastructure health snapshot into a single dashboard payload. Uses cached component health if recent, otherwise runs all registered component checks.
- Success Output: Standard success envelope wrapping: `timestamp`; a `system` block (overall status, human-readable message, uptime expressed as a healthy-rate percentage string, and average response time as a millisecond string); `monitoring` stats (whether the background monitor is running, configured check interval, total/recent check counts); `health` stats (current status, average response time, healthy-rate percentage); `alerts` stats (total, last-24h count, critical count, warning count, unresolved count); a `components` array (each with name, status, message, last-check time, response time); an `infrastructure` block summarizing database and cache component status+message; and a `performance` block (average API response time, plus placeholder database query time and cache hit-rate fields).
- Error Conditions: Non-admin → admin-required rejection. Server failure → standard server-error envelope.

#### Application health history — GET /api/monitoring/health/history
- Invocation: Authenticated client request.
- Inputs: `limit` (query, optional integer, default 50).
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Returns the most recent recorded health-check cycles from the background monitor's rolling history.
- Success Output: Success envelope with `{ history: [ { timestamp, status, responseTime, issuesCount, issues: [...] } ], total }`.
- Error Conditions: Non-admin → rejection. Server failure → server-error envelope.

#### Update monitoring configuration — PUT /api/monitoring/config
- Invocation: Authenticated client request.
- Inputs: JSON body with optional configuration fields. The only validated field is the check interval, which when provided must be between 10,000 and 300,000 milliseconds (10 seconds to 5 minutes). Other config fields (alert thresholds, auto-remediation toggles, notification channels) are accepted and merged.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Merges the supplied values into the background monitor's configuration. If the monitor is currently running and the check interval changed, the periodic monitoring cycle is restarted with the new interval.
- Success Output: Success envelope with `{ updated: true }`.
- Side Effects: Alters the cadence and thresholds of the background monitoring loop, observably changing subsequent alert/check behavior.
- Error Conditions: Check interval out of allowed range → HTTP `400` with `{ success: false, error: "Check interval must be between 10 seconds and 5 minutes", timestamp }`. Non-admin → rejection. Server failure → server-error envelope.

#### Trigger application health check — POST /api/monitoring/health/check
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Runs every registered application-level component health check immediately and returns the freshly computed system health.
- Success Output: Success envelope wrapping a system-health object: `overall` (status, message, timestamp), `components` array, `infrastructure` (database, cache), and `performance` metrics.
- Error Conditions: Non-admin → rejection. Server failure → server-error envelope.

#### Prometheus-format metrics export — GET /api/monitoring/metrics (dashboard variant, not reachable)
- Invocation: Intended as an authenticated admin request returning plaintext Prometheus exposition.
- Inputs: None.
- Behavior (as designed): Emits a text/plain body with gauge/counter lines for overall health status (1/0), system response time, uptime percentage, total monitoring checks, critical/warning alert counts, unresolved alert count, and per-component health (1/0). On error returns plaintext `# Error generating metrics` with HTTP `500`.
- Note: This route is shadowed by the infrastructure metrics detail handler at the same path and is not reachable because the infrastructure metrics behavior is served at this path instead; documented for completeness.

#### Monitoring statistics — GET /api/monitoring/stats
- Invocation: Authenticated client request.
- Inputs: None.
- Preconditions & Authorization: Requires valid session token AND administrator role.
- Behavior: Returns the background monitor's raw statistics object.
- Success Output: Success envelope with the monitoring/health/alerts/auto-remediation statistics groups (same shape as the dashboard's stats subsections).
- Error Conditions: Non-admin → rejection. Server failure → server-error envelope.

#### API traffic metrics accumulation
- Invocation: each completed API request may contribute a best-effort, non-blocking metrics data point after the response is produced. Health/monitoring paths, the API-status path, and static asset paths are excluded from collection. Preflight (OPTIONS) and non-API paths are also excluded. Path identifiers are normalized so dynamic segments (UUIDs, numeric ids, long hex ids) collapse to a generic placeholder, grouping metrics per logical endpoint.
- Inputs (per data point): HTTP method, normalized path, response status code, response time in milliseconds, timestamp. Missing any of method/path/statusCode/responseTimeMs is rejected.
- Behavior: Maintains runtime running counters per logical endpoint (request count, error count, cumulative and maximum response time, a bounded rolling window of the most recent response times for percentile calculation, a per-status-code tally, and last-request time). Requests with status code 500 or higher increment the error count. Observable operations are: accept a data point, return a snapshot, reset all counters, and report liveness.
  - Snapshot: returns per-endpoint metrics (enriched with a human-readable category and description for known endpoints) plus a global rollup (total requests, total errors, average response time, p50 and p95 response time across the combined recent-window samples, started-at time, last-updated time, generation time).
  - Reset: clears all counters, resets the start time, and wipes persisted state.
  - Liveness: returns a healthy status with the count of tracked endpoints and uptime since start.
- Side Effects: Periodically makes an hour-bucketed traffic summary available for dashboard consumption with a 24-hour retention window, and preserves accumulator state for crash recovery. State is also preserved opportunistically after every Nth ingested data point. On restart it restores prior counters and start time.
- Error Conditions: Unknown sub-path → HTTP `404`. Missing required data-point fields → HTTP `400`. Internal error → HTTP `500`. A data-point delivery failure from middleware is swallowed and never affects the originating request.
- Invariants & Guarantees: Metric collection must never break or delay the user-facing request (best-effort, asynchronous). The recent-response-time window per endpoint is capped (bounded memory). Percentiles are computed over the combined recent windows, not full history.

---

### Data Concepts (neutral)

- **Infrastructure instance metric**: A point-in-time health record for one real-time backend instance. Carries: instance category (one of: real-time conversation room, user-connection holder, message broadcaster, delayed-message processor), instance identifier, derived health status (`healthy` | `degraded` | `unhealthy` | `unknown`), active connection count, total connections served, connection capacity, average latency, requests-per-second, error rate (0–1), memory usage (MB), CPU usage (%), uptime, last-health-check time, last-activity time, and a list of currently-breaching alerts. Records older than ~5 minutes since their last check are treated as stale and discarded.
- **Infrastructure alert**: A raised condition tied to an instance. Carries: alert type (high error rate, high latency, high memory, connection limit, instance unresponsive, instance crashed), severity (`warning` | `critical`), human-readable message, timestamp, and free-form metadata. Alert history is retained for roughly the last hour, then pruned.
- **Aggregate monitor statistics**: Totals across all tracked instances — total instances, count-by-category, healthy/degraded/unhealthy counts, total alerts, active alerts, last update time.
- **Application health-check result**: Per-registered-component outcome — status (`healthy` | `warning` | `critical` | `unknown`), message, timestamp, and response time. Components include conceptual infrastructure checks for the database and the cache.
- **System health snapshot**: Overall status derived from all component results (any critical → `critical`; else any warning → `warning`; else any unknown → `unknown`; else `healthy`), with overall message, per-component health array, infrastructure (database, cache) health, and performance metrics (average API response time; database query time and cache hit-rate are present but reported as zero within the current behavioral boundary).
- **Health-cycle record**: One background monitoring cycle's result — timestamp, overall status, response time, and extracted issue strings. Kept in a bounded rolling history.
- **Application alert record**: Generated by the background monitor — unique id, timestamp, level (`warning` | `critical`), message, affected component name, resolved flag (+ optional resolved-at), and an auto-remediated flag. Kept in a bounded rolling history.
- **Per-endpoint API traffic metric**: See the traffic accumulator operation above — counters keyed by method + normalized path.

Monitoring histories and counters are bounded by retention windows, size limits, explicit resets, and restart behavior. API-traffic summaries remain available for dashboard consumption for roughly 24 hours, and the traffic accumulator may restore prior counters after restart. There is no soft-delete concept here; history is bounded by time-window pruning and size trimming rather than deletion flags.

---

### State & Lifecycle

- **Derived instance health status**: Computed each sweep from thresholds. An instance is `unhealthy` when its error rate exceeds twice the configured error-rate threshold OR its latency exceeds three times the latency threshold; otherwise `degraded` when error rate exceeds the threshold, OR latency exceeds the threshold, OR memory exceeds the memory threshold; otherwise `healthy`. Unreachable or timed-out instances are recorded as unresponsive (treated as critical/unhealthy). Default thresholds: error rate 10%, latency 1000 ms, memory 100 MB, connection utilization 80%.
- **Aggregate system status**: `healthy` only when at least 70% of instances are healthy at sweep time; otherwise `degraded`. This maps to HTTP `200` vs `207` on the public probe.
- **Circuit breaker**: An operator-controllable gate with states reported as-is. Transitions observable via this area: forced-open (emergency stop) and reset (return to normal). Open state causes gated traffic to be short-circuited; reset re-enables it. Both transitions are admin-only and audit-logged.
- **Background application monitor**: Has running/stopped states. Starting begins an immediate cycle plus a recurring cycle on the configured interval; stopping cancels the recurring cycle. Updating the interval while running restarts the loop. Per-component consecutive-failure counters increment on each unhealthy cycle and reset to zero when a component recovers.

---

### Real-time / Event Behavior

This area does not broadcast events to end-user WebSocket clients. Its "events" are operational and consumed by operators and external systems:

- **Threshold alerts (infrastructure)**: During each sweep, breaching conditions generate alerts. High error rate, high latency, high memory, and high connection-utilization each raise an alert; severity escalates to `critical` when the breach is roughly double the threshold (or connection utilization exceeds 95%), else `warning`. Alerts are subject to a per-instance cooldown (default 5 minutes) so repeated breaches within the window do not re-emit, and are capped (default max 10 per hour) when alerting is enabled. Unresponsive/unreachable instances always raise a critical alert. Alerts surface via the active-alerts and alert-history endpoints and via log records (critical vs warning log levels).
- **Application-level alerts**: The background monitor raises alerts when overall system response time exceeds warning/critical response-time thresholds (defaults 2000 ms warning, 5000 ms critical), when a component accumulates a configured number of consecutive failures (default 3), and when overall status is critical. Each alert may trigger outbound notifications: if a notification webhook URL is configured, an HTTP POST is sent containing the alert, its details, and a system identifier; email and chat-channel notification hooks are configurable placeholders. A failure of the monitoring cycle itself raises a critical "monitoring system failure" alert.
- **Auto-remediation triggers**: When enabled and overall health is not healthy, the monitor may attempt remediation actions (cache clear when cache is unhealthy; database reconnection attempt when database is unhealthy) and re-checks health after a short delay; within the current behavioral boundary these remediation actions produce no externally observable state change beyond logging.
- **Periodic metrics flush**: The API-traffic accumulator makes an hourly-bucketed metrics summary available roughly every 60 seconds for asynchronous analytics/dashboard consumption rather than pushing it to clients.

## Notifications

### Purpose
This area provides the system's notification capability across three layers: (1) a per-user in-app notification inbox that staff members read, mark as read, and delete, with real-time push to their live sessions; (2) a set of internally-triggered notification events (new message, conversation assignment, mention, priority change, team removal, new customer, new conversation, task reminder, system announcement) that create inbox records and broadcast them live; (3) scheduled personal task reminders that fire notifications when due; and (4) an operational/security alerting subsystem that fans out alerts to external destinations (email, chat webhook, generic webhook, SMS, console) with severity gating and rate limiting. All inbox operations are scoped to the authenticated user; some administrative operations require an administrator role.

---

### Operations

The in-app notification operations below are exposed under a common base path `/api/notifications` and all require a valid authenticated session (bearer token). The acting user identity is taken from the session, never from the request body (except where an administrator explicitly targets others). Unless stated otherwise, a missing/invalid session yields an authentication failure (401-style "Authentication required").

#### List notifications — GET /api/notifications/
- Invocation: authenticated client request.
- Inputs (query): optional `type` (one of the notification type enum), `priority` (`low`|`normal`|`high`|`urgent`), `isRead` (`"true"`/`"false"` string interpreted as boolean), `dateFrom` and `dateTo` (date strings), `page` (positive integer, default 1), `pageSize` (1–100, default 20; values above the cap are clamped).
- Preconditions & Authorization: authenticated; results restricted to the caller's own notifications.
- Behavior: returns the caller's notifications matching the filters, newest first, excluding any that have passed their expiry moment. Validation runs first; invalid filter values produce a structured validation error before any data is returned.
- Success Output: a paginated list payload — an array of notification objects plus pagination metadata (`page`, `limit`, `total`). Each notification object carries: unique id, owning user id, type, title, content, optional structured `data` object, priority, read flag, read timestamp (if read), expiry timestamp (if set), creation timestamp, and last-updated timestamp. Status: success (200-style).
- Error Conditions: invalid filters → validation error response listing offending fields with machine codes (e.g. invalid type, invalid priority, invalid date range, invalid page/page size). No session → authentication failure.
- Invariants: ownership-scoped; expired records are never listed.

#### Get one notification — GET /api/notifications/:id
- Inputs: path `id` (notification identifier).
- Authorization: authenticated; only returns the record if it belongs to the caller.
- Behavior: fetches the single record by id constrained to the caller.
- Success Output: the notification object; success status.
- Error Conditions: not found or not owned by caller → not-found response; no session → authentication failure.

#### Create a notification — POST /api/notifications/
- Inputs (body): `type` (required, must be a valid notification type), `title` (required, non-empty, max 200 chars), `content` (required, non-empty, max 1000 chars), `data` (optional object), `priority` (optional, default `normal`), `channels` (optional list of delivery channels), `expiresAt` (optional future timestamp), and optionally `userId` to target another recipient.
- Authorization: authenticated. An administrator may create a notification for any recipient (via supplied recipient id, defaulting to self); a non-administrator may only create notifications for themselves (any supplied recipient id is ignored and replaced with the caller's id).
- Behavior: validates and sanitizes inputs (title/content trimmed; string values in `data` are stripped of HTML/script-like markup as a safety measure), persists a new record, then attempts delivery over the requested channels (or the default channel set when none specified). Real-time delivery success/failure does not block persistence.
- Success Output: an object containing the new notification's id; success status.
- Error Conditions: validation failures (missing/empty/over-length title or content, missing/invalid type, invalid priority, non-object data, non-future or non-date expiry, invalid channel names) → structured validation error. No identity at all → authentication failure.
- Invariants: ownership enforced server-side for non-admins; expiry must be in the future.

#### Bulk create notifications — POST /api/notifications/bulk
- Inputs (body): `notifications` (required array, 1–1000 items, each validated as a single create request), optional `batchId`.
- Authorization: administrator only; non-admins receive an authorization failure.
- Behavior: validates the whole batch (each item individually); on success persists the batch and attempts delivery. Processing is resilient — partial failures are reported rather than aborting the whole batch.
- Success Output: counts of successful and failed items, the list of created ids, and a list of per-item failures (each with the item's index and an error message); success status with a summary message.
- Error Conditions: not an array / empty / over the 1000 cap, or any item failing validation → structured validation error keyed per item index. Non-admin → authorization failure.

#### Mark one as read — PUT /api/notifications/:id/read
- Inputs: path `id`.
- Authorization: authenticated; only the caller's own record.
- Behavior: marks the record read and stamps a read time. Idempotent in effect.
- Success Output: success acknowledgement.
- Error Conditions: record not found / not owned → not-found response.

#### Mark all as read — PUT /api/notifications/mark-all-read
- Inputs (body, optional): `type` to restrict to a single notification type.
- Authorization: authenticated; affects only the caller's own unread records.
- Behavior: marks all of the caller's unread records (optionally of one type) as read.
- Success Output: a count of records updated; success message reflecting the count.

#### Delete a notification — DELETE /api/notifications/:id
- Inputs: path `id`.
- Authorization: authenticated; only the caller's own record.
- Behavior: permanently removes the record (hard delete).
- Success Output: success acknowledgement.
- Error Conditions: not found / not owned → not-found response.

#### Get statistics — GET /api/notifications/stats
- Authorization: authenticated; scoped to the caller.
- Behavior: returns aggregate counts for the caller across non-expired records.
- Success Output: an object with: total and unread totals; per-type breakdown (each with total and unread) covering all defined types (only message/assignment/mention/system carry real per-type counts; the remaining types report zeros); per-priority breakdown (high and urgent carry real counts; low/normal report zeros); time-range counts (today, this week, this month); and a per-channel delivery-stat scaffold (sent/delivered/failed per channel, currently zeroed). Success status.

#### Get unread count — GET /api/notifications/unread-count
- Inputs (query, optional): `type` to restrict to a single type.
- Authorization: authenticated; scoped to the caller.
- Behavior: returns the number of unread, non-expired notifications for the caller (optionally of one type).
- Success Output: an object with `count` and the `type` echoed (or `all`). Success status.

#### Get recent notifications — GET /api/notifications/recent
- Inputs (query): `limit` (default 10, capped at 50).
- Authorization: authenticated; scoped to the caller.
- Behavior: returns the caller's most recent unread, non-expired notifications, newest first.
- Success Output: an object with the notifications array, the count, and the effective limit. Success status.

#### Cleanup expired notifications — DELETE /api/notifications/cleanup
- Authorization: administrator only.
- Behavior: permanently removes all records system-wide whose expiry moment has passed.
- Success Output: a count of deleted records; success message. Non-admin → authorization failure.

#### Get channel statistics — GET /api/notifications/channels/stats
- Authorization: administrator only.
- Behavior: returns the current delivery-channel registry status — for each known channel: whether it is enabled, its type, and optional per-channel stats.
- Success Output: a map of channel descriptors; success status. Non-admin → authorization failure.

#### Test a delivery channel — POST /api/notifications/channels/:channelType/test
- Inputs: path `channelType` (must be one of: a persisted-record channel, a real-time channel, email, push, webhook, or SMS); body optional `message`.
- Authorization: authenticated.
- Behavior: sends a synthetic test notification to the caller over the named channel and reports the delivery result.
- Success Output: a delivery-result object (success flag, optional message id, optional timing/metadata, or an error message). Success status with a message naming the channel.
- Error Conditions: unknown/invalid channel type → validation error; channel disabled/unavailable → result with success=false and an explanatory error message.

#### Trigger a new-message notification — POST /api/notifications/new-message
- Inputs (body): recipient id, conversation reference, sender display name, message content, optional channel list.
- Authorization: authenticated.
- Behavior: creates a `new_message`-type notification for the recipient with a normal priority and a 24-hour expiry; title is a fixed "new message" label and content is the sender name plus a truncated (≈100 char) preview.
- Success Output: the new notification's id.

#### Trigger a conversation-assigned notification — POST /api/notifications/conversation-assigned
- Inputs (body): recipient id, conversation reference, customer display name, assigner name.
- Behavior: creates a `conversation_assigned`-type notification at high priority with a 7-day expiry, describing who assigned which customer's conversation.
- Success Output: the new notification's id.

#### Trigger targeted system notifications — POST /api/notifications/system
- Inputs (body): optional `userIds` list, required `title` and `content`, optional `data`, optional `broadcastToAll` flag.
- Authorization: administrator only.
- Behavior: resolves the recipient set (the supplied list, or — when `broadcastToAll` is set or no list is given — all active staff accounts), then creates a `system`-type notification (normal priority, 30-day expiry) for each and pushes each in real time. Title and content are required.
- Success Output: the created ids, their count, and whether it was a broadcast-to-all. Missing title/content → 400; no resolved recipients → 400; non-admin → authorization failure.

#### Broadcast a system announcement — POST /api/notifications/broadcast
- Inputs (body): required `title` and `content`, optional `priority`, optional `data`.
- Authorization: administrator only.
- Behavior: resolves all active staff accounts and creates+pushes a system notification to each; the broadcaster's identity and chosen priority are folded into the notification's `data`.
- Success Output: created ids, recipient count, broadcaster id, and a timestamp. Missing title/content → 400; no active users → 400; non-admin → authorization failure.

#### Module health / info — GET /api/notifications/health, GET /api/notifications/info
- Invocation: unauthenticated.
- Behavior/Output: a static health/status object (module name, status, timestamp, version) and a static capabilities/description object respectively.

---

#### Task reminders (personal scheduled reminders)
These operations live under base path `/api/reminders`, all require an authenticated session, and are scoped to the caller's own reminders.

##### Create a reminder — POST /api/reminders
- Inputs (body): `title` (required), `remindAt` (required, must be a valid future timestamp), optional `content`, optional conversation reference, optional `repeatType` (`none`|`daily`|`weekly`|`monthly`, default none), optional `repeatInterval` (numeric, default treated as 0/1).
- Behavior: persists a new reminder owned by the caller, not yet sent and not completed.
- Success Output: the new reminder's id; created status (201-style).
- Error Conditions: missing title or remindAt → 400 ("Title and remindAt are required"); unparsable remindAt → 400 ("Invalid remindAt date format"); remindAt in the past → 400 ("remindAt must be in the future").

##### List reminders — GET /api/reminders
- Inputs (query): `includeCompleted` (`"true"` to include completed ones; default excludes completed).
- Behavior: returns the caller's reminders ordered by their reminder time.
- Success Output: the reminder array plus a count.

##### Get upcoming reminders — GET /api/reminders/upcoming
- Inputs (query): `minutes` (look-ahead window, default 30).
- Behavior: returns the caller's not-completed, not-yet-sent reminders whose time falls within the window, ordered by time.
- Success Output: the reminder array, the count, and the look-ahead window.

##### Reminder statistics — GET /api/reminders/stats
- Behavior: returns counts for the caller: total, pending, completed, and overdue (overdue = past-due, not completed, not yet sent).
- Success Output: the four counts.

##### Get one reminder — GET /api/reminders/:id
- Behavior: returns the caller's reminder by id. Not found / not owned → 404.

##### Update a reminder — PUT /api/reminders/:id
- Inputs (body, all optional): `title`, `content`, `remindAt` (validated as a parseable date), `repeatType`, `repeatInterval`.
- Behavior: applies the provided fields to the caller's reminder. Changing the reminder time resets the sent flag so it can fire again.
- Success Output: success acknowledgement. Invalid remindAt → 400; not found / not owned → 404.

##### Mark a reminder complete — PUT /api/reminders/:id/complete
- Behavior: marks the caller's reminder completed and stamps a completion time. Not found / not owned → 404.

##### Delete a reminder — DELETE /api/reminders/:id
- Behavior: permanently removes the caller's reminder (hard delete). Not found / not owned → 404.

##### Process due reminders (manual) — POST /api/reminders/process
- Authorization: administrator only; non-admin → forbidden (403).
- Behavior: runs the due-reminder processing pass and reports how many were processed (see Scheduled processing below).
- Success Output: processed count and a message.

##### Scheduled reminder processing
- Invocation: a scheduled timer event (cron) invokes the same due-reminder pass.
- Behavior: finds all reminders that are due (reminder time at or before now), not completed, and not yet sent; for each it fires a task-reminder notification to the owner, marks the reminder as sent (stamping a sent time), and — if the reminder repeats — creates the next occurrence (daily/weekly/monthly advanced by the repeat interval). Failures on individual reminders are isolated and do not stop the pass.
- Side Effects: creates `task_reminder` inbox notifications (high priority, 24-hour expiry, title is a fixed reminder label, content combines the reminder title with a truncated ≈50 char preview) and pushes them in real time; spawns follow-up reminder records for repeating reminders.

---

#### Operational / security alerting triggers
Security events and monitoring conditions can trigger alerts that deliver to external destinations.

##### Send a multi-destination security alert
- Invocation: alert trigger with a title, message body, severity (`low`|`medium`|`high`|`critical`), and optional metadata (platform, integration reference, source IP, event type, free-form details, timestamp).
- Behavior: selects the configured external destinations that are enabled and whose minimum-severity gate is satisfied (severity order low<medium<high<critical), then dispatches to each in parallel: an email destination (requires email API configuration, else that destination fails), a chat-webhook destination (posts a richly formatted message), and a generic webhook destination (posts a JSON payload of title/message/severity/metadata/timestamp). Failures per destination are collected, not fatal.
- Output: a summary of success count, failure count, and a list of error strings.
- Configuration: destinations are assembled from environment configuration (which of email/chat-webhook/generic-webhook are enabled, their recipients/URLs/credentials, and each one's minimum severity). When a destination's required configuration is missing, that destination is skipped or fails gracefully.

##### Send a monitoring alert with rate limiting and escalation
- Invocation: alert trigger with an alert level (`info`|`warning`|`critical`|`emergency`), title, description, and optional metadata.
- Behavior: if alerting is globally disabled, the alert is recorded but not delivered. A per-hour rate limit is enforced (default cap 20 alerts/hour) except that `emergency`-level alerts always bypass the limit. For non-rate-limited alerts, it delivers to the channel set mapped to that level (defaults: info/warning → console; critical → console + webhook; emergency → console + webhook + chat). Each channel attempt is recorded with success/failure. The alert record is persisted (with a 30-day retention), the hourly counter is incremented, and for critical/emergency levels an escalation is scheduled (escalation is currently logged, not executed).
- Output: an alert record carrying a generated alert id, level, title, description, a timestamp, acknowledged/resolved flags (initially false), and the list of channel send attempts.
- Supporting operations:
  - Acknowledge an alert: given an alert id and an acknowledging actor, marks the stored alert acknowledged and stamps who/when; returns whether it succeeded (false if the alert id is unknown).
  - Resolve an alert: given an alert id, marks the stored alert resolved and stamps when; returns whether it succeeded.
  - Get/update alert configuration: read the current alerting configuration (or built-in defaults), or merge in partial configuration updates (persisted with long retention). Configuration covers: enabled flag, default channels, per-level channel mappings, rate-limiting settings (enabled, max per hour, cooldown minutes), and escalation settings (enabled, escalation delay minutes, escalation channels).
- Available external channels for this subsystem: console, generic webhook, chat webhook, email, and SMS. Each external channel requires its own configuration (destination URL/endpoint, credentials, recipient list); when missing, that channel attempt fails gracefully and returns failure.

---

### Data Concepts (neutral)

- **Notification record**: belongs to exactly one recipient (user identity, which may be a string or numeric identifier). Carries: a unique identifier; a type (see lifecycle/types below); a short title; a body content string; an optional structured data bag (arbitrary key/value context relevant to the type, e.g. a referenced conversation, sender/assigner name, team reference, platform, priority echo); a priority; a read flag plus an optional read timestamp; an optional expiry timestamp; a creation timestamp; and a last-updated timestamp. Records are hard-deleted (no soft-delete); expired records are excluded from reads and are reclaimable via the admin cleanup operation.
- **Notification type** (enumerated): new message; conversation assigned; conversation transferred; mention; system; priority changed; customer responded; task reminder; agent removed from team; customer followed (new customer joined); new conversation.
- **Priority** (enumerated): low, normal, high, urgent.
- **Delivery channel** (enumerated, for the inbox subsystem): a persisted-record channel, a real-time channel, email, push, webhook, SMS. Real-time is the default channel.
- **Task reminder**: belongs to one owner. Carries: a unique identifier; owner identity; title; optional content; a reminder time; optional conversation reference; a repeat mode (none/daily/weekly/monthly) and a repeat interval; a completed flag with optional completion time; a sent flag with optional sent time; a creation time.
- **Alert record** (monitoring subsystem): a unique identifier; severity level; title; description; a timestamp; acknowledged flag (with actor and time); resolved flag (with time); and a list of per-channel send attempts (channel, time, success, optional error). Retained ~30 days.
- **Alert/destination configuration**: enabled flag, per-level channel mappings, rate-limit parameters, escalation parameters, and per-destination connection settings.

### State & Lifecycle

- **Notification read state**: created unread → marked read (single, all, or all-of-a-type). Marking read stamps a read time and is effectively idempotent. There is no transition back to unread. Records may also reach an "expired" state implicitly once their expiry timestamp passes — expired records are filtered out of all reads and are eligible for permanent cleanup. Deletion (by owner, or by admin cleanup for expired) is terminal.
- **Task reminder lifecycle**: created as pending (not sent, not completed) → may be updated (and editing the reminder time re-arms it by clearing the sent flag) → when due it is fired (sent flag set, sent time stamped) → if repeating, a new pending occurrence is spawned for the next interval. Independently, an owner may mark a reminder completed (terminal for that reminder, with a completion time) or delete it (terminal). "Overdue" is a derived state (past-due, not completed, not yet sent), not a stored status.
- **Alert lifecycle (monitoring)**: created (delivery attempted unless globally disabled or rate-limited) → optionally acknowledged → optionally resolved. Acknowledge and resolve are independent flags. Critical/emergency alerts additionally schedule an escalation.

### Real-time / Event Behavior

- **`notification` real-time event**: emitted to a specific recipient's live sessions whenever a notification is created via the internal triggers, the targeted-system endpoint, the broadcast endpoint, or task-reminder firing. Payload includes the notification's id, type, title, content, priority, optional data bag, and a creation timestamp. The event is addressed by recipient identity; only that recipient's connected sessions receive it. Real-time delivery is best-effort — if it fails, the persisted notification still exists, so the recipient sees it on next inbox fetch.
- **Notification kinds and audiences triggered by system behavior**:
  - Priority changed → the responsible agent; priority of the notification mirrors the new conversation priority (urgent/high/normal); 7-day expiry; high-priority effects.
  - Agent removed from team → the affected agent; high priority; 7-day expiry; the data bag includes team reference, who removed them, and the conversations they can no longer see (intended to prompt the client to refresh and close affected views).
  - Mention → the mentioned agent; high priority; 7-day expiry; data includes the conversation and mentioner.
  - Customer followed (new customer joined) → all active administrators plus, when a team is specified, that team's members, otherwise all active agents; per-recipient, high priority, 7-day expiry; one failed recipient does not block others.
  - New conversation → same audience rule as customer-followed (admins + team members, or admins + all agents); created in bulk, high priority, 7-day expiry, broadcast in parallel without blocking.
  - System / broadcast → the targeted users or all active staff; normal priority; 30-day expiry.
  - Task reminder → the reminder owner; high priority; 24-hour expiry.
- **Team-change broadcasts (no inbox record)**: member-added/member-removed and team-updated are emitted as real-time events to keep management views' member counts and team info current; these intentionally do not create inbox notification records.
- **External alert events**: the alerting subsystems emit to external destinations only (email, chat webhook, generic webhook, SMS, console) per severity/level gating and rate limits; they do not produce in-app inbox notifications or in-app real-time events.

## Background Queue Processing

### Purpose
This area decouples slow or unreliable outbound third-party messaging-platform (LINE) operations from the synchronous request/response path. Two kinds of background work are handled by a single message queue: (1) delivering agent-authored outbound messages to the messaging platform, and (2) downloading and persisting inbound media attachments that arrived from the messaging platform. The originating operation returns immediately; the work is then performed in the background, persists observable state changes, emits real-time events, retries transient failures with progressively increasing delay, and moves terminally failed work to a dead-letter holding area. A set of authenticated read-only monitoring endpoints expose processing health and (fixed) performance figures.

### Operations

#### Asynchronous outbound delivery (observable behavior)
- Invocation: A side effect of an agent sending an outbound message (not directly client-facing of its own). The send is accepted and actual platform delivery happens in the background.
- Inputs (the message and delivery parameters carried for background delivery):
  - message identifier (string, required) — the persisted message this job will deliver.
  - conversation identifier (string, required).
  - recipient platform identifier (string, required) — the destination platform user ID.
  - content (string, required) — text body.
  - message kind (string enum: text | image | file | flex, required).
  - attachments (optional array); each attachment carries: an identifier, a kind (image | video | audio | file), a URL, and optional filename, MIME type, and byte size.
  - metadata (object, required): originating agent identifier (required), optional agent display name, an enqueue timestamp (epoch ms), an optional retry counter (defaults to 0), and an optional originating request identifier.
  - optional platform send options: a flag to suppress recipient push notification, and a custom aggregation unit label.
  - An optional discriminator marking the job as an outbound message; jobs with no discriminator are also treated as outbound (backward compatible).
- Outbound background-delivery payloads include an enqueue timestamp, a retry counter initially at 0, and the optional originating request identifier when supplied.
- Preconditions & Authorization: Triggered by trusted background-delivery behavior; no per-call user authentication is performed.
- Behavior: Accepts the message for background delivery. The observable result is only an immediate acknowledgment; actual delivery to the platform happens later.
- Success Output: A small status object — a success boolean true, with no error field.
- Side Effects: The message is queued for background delivery. No delivery-state change is observable until that processing runs.
- Error Conditions: If the queue submission throws, returns a status object with success false and a human-readable error string (the failure is swallowed, never thrown to the caller).
- Invariants & Guarantees: Acceptance is fire-and-forget from the sender's perspective; delivery is at-least-once via background retries.

#### Asynchronous inbound media processing (observable behavior)
- Invocation: Triggered when an inbound platform message carries an image/file/video/audio attachment (location and sticker kinds excluded). One unit of background work runs per media item.
- Inputs (the parameters carried for each media-processing unit):
  - a discriminator value identifying this as a media-processing job (required).
  - persisted message identifier (string, required).
  - conversation identifier (string, required).
  - optional owning-team numeric identifier (for team-scoped broadcast).
  - platform-side message identifier (string, required) — used to fetch the media content from the platform.
  - platform media kind (string, required) — e.g. image / file / video / audio.
  - optional original filename (present for file attachments).
  - an enqueue timestamp (epoch ms).
- Preconditions & Authorization: Triggered during webhook handling by trusted background-processing behavior.
- Behavior: Submits one media-processing job per attachment. The inbound message itself is already persisted before enqueue, so enqueue failure is non-critical.
- Success Output: None to an external caller (logging only).
- Side Effects: One media-processing unit per attachment is queued for background processing.
- Error Conditions: A submission failure is logged and tolerated; the message remains stored and the frontend has a metadata fallback. No exception propagates.

#### Consume queued work batch — background processor (runtime-invoked)
- Invocation: Invoked automatically by the platform runtime whenever a batch of queued jobs is ready. Batches are bounded by a maximum batch size (10) and a maximum wait window (5 seconds) before a partial batch is delivered.
- Inputs: A batch object containing an array of individual jobs; each job exposes its body (one of the two payload kinds above), a unique runtime job identifier, and a current attempt counter.
- Preconditions & Authorization: Runs in trusted backend context; no user auth.
- Behavior (per job, observable outcomes):
  - The consumer inspects the discriminator. A media-processing job is routed to media handling; everything else is treated as an outbound-message job.
  - Outbound-message job: builds the platform message set, sends it, and on success acknowledges the job and marks the message delivered; on a reported send failure it requeues the job for another attempt; an unexpected thrown error also requeues the job.
  - Media-processing job: performs the download-and-store work; on success acknowledges the job; if the work yields no stored attachment it raises, causing the job to be retried.
  - Jobs are processed independently within a batch — one job's failure does not abort the others.
- Success Output: None returned to a caller; success is observable through delivery-status persistence and emitted real-time events.
- Side Effects: See the two sub-operations below.
- Invariants & Guarantees: At-least-once delivery. A job is retried until it succeeds, is explicitly acknowledged, or reaches the maximum retry count (3), after which the runtime routes it to the dead-letter queue. Acknowledgment removes a job from the queue permanently.

#### Outbound message delivery — within consumer
- Behavior (observable order of effects):
  1. The platform message set is assembled from the job: a text bubble is included only when content is present AND content is not merely a file-description placeholder (placeholders such as "Sent a file: …", "Sent N files", or a bracketed file/image label are recognized and suppressed so they are not sent as visible text). Image attachments are sent as native image bubbles; non-image attachments are sent as a structured file card carrying filename, MIME type, and size.
  2. If the assembled set is empty, delivery is reported as a failure with reason "no messages to send".
  3. The platform imposes a hard cap of 5 bubbles per send. Sets of 5 or fewer are sent in one call; larger sets are split into chunks of 5 and sent sequentially with a brief pause (~100 ms) between chunks. If any chunk fails, the overall send is considered failed.
  4. On overall success: the persisted message's delivery state is set to "delivered" and a delivery-success real-time event is broadcast.
  5. On send failure (platform returned failure or an exception): a delivery-failure real-time event is broadcast carrying the failure reason and the retry counter from metadata; the job result is marked failed (leading the consumer to requeue).
- Success Output: A per-message result object: message identifier, conversation identifier, success boolean, and on success a delivered-at timestamp; on failure an error string (and, in the failure broadcast, the retry counter).
- Side Effects: Delivery-state field of the persisted message updated to delivered or failed; one WebSocket delivery-status event emitted per outcome. Delivery-state update failures and broadcast failures are caught and logged but do NOT fail the delivery (best-effort side effects).
- Error Conditions: Empty message set, platform-reported failure, or any thrown error all yield a failed result; the consumer then requeues the job.

#### Inbound media processing — within consumer
- Behavior (observable order of effects):
  1. The media content is fetched from the platform using the platform-side message identifier, stored in file storage, and a persisted attachment record is created linking it to the message.
  2. If no attachment record results (download/store failed), the job raises so the runtime retries it.
  3. On success, a "message updated" real-time event is broadcast to the global/conversation-list audience, carrying the new attachment data, at high priority.
  4. A direct best-effort notification is also sent to the per-conversation real-time channel so an open conversation-detail view refreshes; failure of this direct notification is logged and tolerated (the media is already stored).
- Side Effects: One stored media object, one persisted attachment record, one global "message updated" broadcast, and one best-effort per-conversation notification.
- Error Conditions: Download/store yielding no attachment causes a retry; the per-conversation notification failing does not fail the job.

#### Generic queue-job framework behavior (shared base)
- Invocation: A shared background-processing behavior used to standardize per-job observable outcomes.
- Behavior (observable outcomes for any job built on it):
  - Measures processing time per job and records running statistics: total processed, success count, error count, retry count, average processing time, and last-processed timestamp.
  - On a successful job result: acknowledges the job.
  - On a failed result or thrown error: classifies the failure from its message text into categories (network, timeout, rate-limit, validation, temporary-failure, or a default system error) and decides retry vs. give-up.
  - Retry decision: a job is retried only if its attempt count is below the configured maximum AND its error category is in the retryable set (by default: network, timeout, rate-limit, temporary, and system errors; validation and permanent errors are NOT retried).
  - Retry delay uses progressively delayed retry: a base delay grows by a multiplier per attempt, capped at a maximum delay; the job is requeued with that delay (expressed in whole seconds).
  - When not retryable or max attempts reached: the job is acknowledged (dropped, no further retry) and the failure is logged.
- Configurable parameters (defaults): maximum retries 3, base delay 1000 ms, maximum delay 30000 ms, backoff multiplier 2.
- Helper behaviors: bounded-concurrency batch processing of arbitrary items (default chunk size 5, all items in a chunk processed concurrently, a chunk failure aborts remaining chunks), and a timed-operation wrapper that logs duration on success or failure.
- Statistics access: callers can read a snapshot of the running statistics and can reset them to zero.

#### Queue monitoring: unified statistics — GET /api/queues/stats
- Invocation: Authenticated client HTTP request.
- Preconditions & Authorization: Requires a valid authentication token; otherwise the auth layer rejects with an unauthorized error.
- Behavior: Returns a snapshot of queue monitoring data. The figures are fixed values within the current behavioral boundary, not live counters.
- Success Output (200): A success envelope wrapping: a summary (total queue count, healthy queue count, total messages, overall status), a per-queue block for the message queue (name, queue label, purpose text, status, metric fields for messages-in-queue / processing-rate / error-rate / average processing time, and configuration showing maximum batch size 10, maximum batch timeout 5, and a retry-policy label of "exponential-backoff"), and a system-health block (an uptime number and a last-check timestamp).
- Error Conditions: On unexpected error, a standardized API error response.

#### Queue monitoring: health check — GET /api/queues/health
- Invocation: Authenticated client HTTP request.
- Preconditions & Authorization: Valid auth token required.
- Behavior: Returns a fixed healthy status block indicating queue availability and a sub-100 ms processing-latency claim, plus an overall status and timestamp.
- Success Output (200): A success envelope with the health block described above.
- Error Conditions: Standardized API error response on failure.

#### Queue monitoring: performance metrics — GET /api/queues/performance
- Invocation: Authenticated client HTTP request.
- Preconditions & Authorization: Valid auth token required.
- Behavior: Returns performance metrics that are fixed within the current behavioral boundary: throughput (messages-per-second, peak, average processing time) and reliability (success rate, error rate, retry rate), with a timestamp.
- Success Output (200): A success envelope wrapping the metrics object.
- Error Conditions: Standardized API error response on failure.

#### Queue monitoring: maintenance operation — POST /api/queues/maintenance
- Invocation: Authenticated client HTTP request with a JSON body.
- Inputs: a body field naming the maintenance operation to perform (string).
- Preconditions & Authorization: Valid auth token required.
- Behavior: Dispatches on the requested operation. The only recognized operation reports current queue status as healthy. Any unrecognized operation name yields a 400-class response that lists the available operations.
- Success Output (200): For the recognized operation, a success envelope with the queue status. For an unknown operation, a response carrying an error note, the list of available operations, and a 400 status.
- Error Conditions: Standardized API error response if body parsing or processing fails.

### Data Concepts (neutral)
- Outbound-message job: a unit of deferred work describing one platform message to deliver — references a persisted message and conversation, names the destination platform user, carries the textual content, the message kind, optional attachments, tracking metadata (originating agent, enqueue time, retry counter, originating request id), and optional platform send options.
- Inbound media-processing job: a unit of deferred work describing one inbound attachment to fetch and persist — references a persisted message and conversation, names the platform-side message and media kind, optionally an owning team and original filename, and an enqueue time.
- Attachment descriptor: identifier, media kind (image / video / audio / file), source URL, optional filename, MIME type, and byte size.
- Per-job result: message and conversation identifiers, a success flag, an optional delivered-at timestamp, an optional error string, an optional retry counter, and an optional platform-side message id.
- Running statistics snapshot: total processed, successes, errors, retries, average processing time, and last-processed timestamp.
- Failure category taxonomy: network, timeout, rate-limit, validation, business-logic, system, temporary-failure, permanent-failure — used to decide retryability.
- Persisted message delivery state: a per-message status that this area transitions to "delivered" or "failed" as an observable side effect (uses soft, status-style updates; no records are hard-deleted by this area).
- Two queues exist: a primary work queue and a dead-letter queue that receives jobs exhausting all retries.

### State & Lifecycle
- Job lifecycle: enqueued -> in-flight (consumed, attempt N) -> either acknowledged (terminal success or terminal give-up) or requeued (attempt N+1, after a backoff delay). After the maximum retry count (3) is exceeded, the runtime routes the job to the dead-letter queue (terminal). Acknowledgment is terminal and permanently removes the job.
- Message delivery state (observable on the persisted message): begins in its pre-delivery state, transitions to "delivered" on successful outbound send, or to "failed" on send failure. These transitions are driven only by the consumer; the state update itself is best-effort and its own failure does not change the job's success/retry outcome.
- Retry eligibility (framework path): only transient categories (network, timeout, rate-limit, temporary, system) are retried; validation and permanent failures are dropped immediately (acknowledged without retry). The live LINE consumer additionally requeues on any send failure or thrown error up to the runtime's retry cap.

### Real-time / Event Behavior
- Delivery-success event: emitted after a successful outbound send. Carries message id, conversation id, a success flag (true), and a delivered-at timestamp. Audience: subscribers of the message/conversation real-time channel.
- Delivery-failure event: emitted after an outbound send failure or exception. Carries message id, conversation id, success flag (false), the error reason, and the retry counter. Audience: same real-time channel.
- "Message updated" event (media): emitted at high priority after inbound media is stored. Carries conversation id, message id, and the newly created attachment data. Audience: the global/conversation-list real-time channel.
- Direct per-conversation notification (media): a best-effort message-updated notification delivered straight to the per-conversation real-time channel so an open conversation-detail view refreshes; failure is tolerated and not retried.
- All event emissions and delivery-state writes are best-effort: their failure is logged but never fails or retries the underlying job.

## System Settings & Administration

### Purpose
This area is the administrative and operational-observability control plane for the multi-channel support platform. It lets operators read the platform's runtime health and configuration status, view aggregate operational statistics and metrics, read and persist tunable system settings, validate live messaging-channel credentials, manage alert notification channels, tune and probe internal data/cache optimization, monitor request-rate and cache optimization, capture customer satisfaction feedback, manage per-user task reminders, record client-side user-experience telemetry, and run one-off administrative data migrations. Many endpoints are pure read-only diagnostics; a smaller set perform privileged configuration changes restricted to administrators.

### Behavioral Boundary (Under-specified)
The operations documented below constitute the entire administrative surface of the current contract. The following commonly-expected administrative capabilities lie outside the current observable behavior and induce no state change within this boundary; a conforming implementation is not required to provide them:
- **Platform data backup and restore.** Distinct from the per-action reversible-restore feature documented in the Activity area, and from the messaging-credential store/clear actions documented elsewhere. No whole-system backup or restore operation is part of the current contract.
- **Operator-initiated cache clearing / cache flush.** Read-only cache-optimization probes and bounded cache tuning are documented below; an explicit "clear all caches" action is not part of the current contract.
- **Service restart, reboot, or maintenance-mode toggling.**
Where a status field or counter for one of these areas is nonetheless surfaced by a documented endpoint, it is reported with a fixed default value (for example, zero or a static "available") and triggers no other effect.

### Operations

> Path note: some method+path combinations are claimed by more than one operation below. Where that happens, only one is reachable at that path; both are documented because either may be the reachable one in different deployments.

#### Basic health probe — GET /api/system/health
- Invocation: unauthenticated client/load-balancer request.
- Inputs: none.
- Behavior: probes the primary datastore with a trivial connectivity query and reports a coarse health verdict.
- Success Output (200): an object with overall status string ("healthy"/"unhealthy"), a current timestamp, a datastore connectivity indicator ("connected"/"disconnected"), and a version label.
- Error Conditions: on probe failure returns 500 with status "unhealthy", a timestamp, an error message, and version.

#### API descriptor — GET /api/system/api
- Invocation: unauthenticated request.
- Inputs: none.
- Behavior: returns a static catalog describing the public API (service name, version, a map of well-known endpoint method+path strings, and a timestamp). Informational only.
- Success Output (200): descriptor object as above.

#### Detailed system status — GET /api/system/system/status
- Invocation: authenticated request.
- Preconditions & Authorization: requires a valid auth token.
- Inputs: none.
- Behavior: probes datastore connectivity and returns a structured status snapshot enumerating subsystem availability (datastore, key-value cache, file storage, real-time coordination components) plus environment label and version. Availability of non-datastore subsystems is reported as static "available".
- Success Output (200): nested status object (overall verdict, timestamp, version, per-service status records, environment).
- Error Conditions: unauthenticated → auth error; server failure → standardized error envelope.

#### Aggregate operational statistics — GET /api/system/stats
- Invocation: authenticated request.
- Inputs: none.
- Behavior: computes a dashboard summary over current data: total message count, total customer count, total conversation count, today's message count, count of conversations marked resolved today, count of agents active within the last five minutes, an average first-response time rendered as a localized human string, and a customer-satisfaction percentage (share of feedback rated 4 or 5 over the last 30 days). If the underlying computation fails, all values gracefully default to zeros / default values rather than erroring.
- Success Output (200): a success envelope wrapping the metrics object plus a timestamp.

#### Recall statistics — GET /api/system/messages/recall-stats
- Invocation: authenticated request.
- Behavior: returns recall counters that are reported as zero within the current behavioral boundary.
- Success Output (200): success envelope with counters and timestamp.

#### Message replies lookup — GET /api/system/messages/:messageId/replies
- Invocation: authenticated request.
- Inputs: a message identifier in the path.
- Behavior: returns the set of reply messages associated with the given message and a count.
- Success Output (200): success envelope with the message identifier, the reply list, and count.

#### Conversation message tree — GET /api/system/conversations/:conversationId/message-tree
- Invocation: authenticated request.
- Inputs: a conversation identifier in the path.
- Behavior: returns the conversation's messages and a parent→children reply mapping plus total message count.
- Success Output (200): success envelope with conversation identifier, message list, reply mapping (keyed by message identifier), and total.

#### Conversation session statistics — GET /api/system/conversations/:conversationId/sessions
- Invocation: authenticated request.
- Inputs: a conversation identifier in the path.
- Behavior: returns session analytics for the conversation.
- Success Output (200): success envelope with the analytics object and timestamp.

#### System information — GET /api/system/info
- Invocation: authenticated request.
- Behavior: returns static descriptive runtime info (version, environment, last-update timestamp, datastore/cache status flags, a nominal uptime value).
- Success Output (200): success envelope with the info object.

#### Read system settings — GET /api/system/settings
- Invocation: authenticated request.
- Inputs: none.
- Behavior: returns the effective settings tree. The response begins from built-in defaults for three groups — general (system display name, contact email, timezone, language), integrations (per-channel connection status only), and advanced (message-queue size, message timeout, cache expiry, session expiry, and boolean toggles for rate-limit, logging, metrics) — then overlays any persisted overrides on top. Sensitive integration credentials are intentionally NOT included; only connection-status fields are exposed for channels.
- Success Output (200): success envelope with the merged settings object.
- Invariants: persisted values are stored as flattened dotted keys and reconstructed into the nested tree; values that look like JSON are decoded, otherwise returned as raw strings.

#### Update system settings — PUT /api/system/settings
- Invocation: authenticated request.
- Inputs: a partial settings object; any subset of the general / integrations / advanced groups, each with optional fields. When the optional validation layer is applied: general.systemName must be a 1–100 char string; general.contactEmail must be a valid email; general.timezone must be a recognizable timezone; general.language must be one of en/zh-TW/zh-CN/ja; per-channel credential fields must be non-empty strings when present; channel status must be one of connected/disconnected/error; advanced.messageQueueSize 1–10000; advanced.messageTimeout 1000–300000; advanced.cacheExpiry 60–86400; advanced.sessionExpiry 300–604800; the three enable* flags must be booleans; at least one of the three groups must be present.
- Behavior: flattens the nested object into dotted keys and upserts each key (insert-or-replace), recording an update timestamp per key. An empty input results in a no-op success.
- Success Output (200): success envelope ("Settings updated successfully", or "No settings to update" when nothing supplied).
- Side Effects: persists each provided setting; emits an administrative audit activity record of type "settings update" against the system resource, capturing the actor (user id, name, role), the set of changed keys/values, the count of changed keys, and the caller's client IP and user-agent.
- Error Conditions: with validation applied, invalid input → 400 with an error message and a details array of per-field messages; malformed JSON → 400; server failure → standardized error envelope.
- Invariants: this is one of the operations flagged for sensitive-operation audit logging and (where the rate-limit guard applies) capped at roughly 20 requests per 5-minute window per user.

#### System metrics — GET /api/system/metrics
- Invocation: authenticated request.
- Behavior: returns live counts of agents active within the last hour, total conversations, and total messages, plus figures reported as fixed values within the current behavioral boundary for average response time, system load, and error rate.
- Success Output (200): success envelope with the metrics object.

#### Test a channel integration — POST /api/system/integrations/:platform/test
- Invocation: authenticated request.
- Inputs: a platform identifier in the path (must be one of the two supported messaging channels); a JSON body with that channel's credential fields. For one channel: channel id, channel secret, access token. For the other: app id, app secret, page id, page token, and optionally a test recipient identifier. Any missing credential is backfilled from securely stored encrypted credentials for that channel if available.
- Behavior: performs live validation calls against the external channel's API: verifies the access token by fetching bot/page info, optionally checks webhook activation, validates app-secret/token correspondence, checks page permissions, and (if a test recipient is supplied) attempts a test message send. Returns a structured pass/fail result. Unsupported platform yields a failure result rather than a hard error.
- Success Output (200): success envelope wrapping a result object: a status ("success"/"error"), a human message, and on success a details object (e.g., resolved bot/page display name and id, channel id/app id, webhook/messaging status, test time).
- Error Conditions: incomplete credentials → result status "error" with a "please complete settings" message; external auth failure surfaces channel-specific messages (invalid/expired token, insufficient permission, rate-limited, service unavailable); thrown errors → standardized error envelope.

#### Composite API status dashboard — GET /api/system/api-status
- Invocation: authenticated request.
- Behavior: assembles a real-time operational dashboard by concurrently gathering per-endpoint metrics from the metrics-collection component, latency probes against datastore / key-value cache / file storage / real-time components, and connectivity checks for each messaging channel. It derives an infrastructure list with green/orange/red latency grades, a channel list (connected/disconnected/error), a derived webhook-delivery indicator, a synthesized event log (error/warning/info entries), aggregate endpoint stats (totals, healthy/warning/error counts, average response time), and an overall status of operational/degraded/outage.
- Success Output (200): success envelope with overall status, endpoint list, infrastructure list, channel list, event list, stats, and timestamp.

#### Configuration check — GET /api/system/config-check
- Invocation: authenticated administrator request (registered as a high-priority endpoint).
- Preconditions & Authorization: requires a valid token AND administrator role.
- Behavior: reports whether critical deployment environment configuration (e.g., front-end and back-end URLs needed for cross-origin operation) is present and valid.
- Success Output: configuration-status object; HTTP 200 when configuration is satisfactory, 503 when not. Echoes permissive cross-origin headers reflecting the caller's origin to aid post-deploy checks.

#### Unified health endpoints — under GET /api/health/*
- Invocation: mixed; some public, some authenticated.
- Operations:
  - GET /api/health/health — public liveness-style verdict (status, service name, timestamp, version, environment).
  - GET /api/health/status — public aggregate system-health report; returns 200 for healthy/warning, 503 for critical, 500 on internal error.
  - GET /api/health/system — authenticated full health report (same status-code mapping).
  - GET /api/health/infrastructure — authenticated infrastructure-tier health (overall + per-component).
  - GET /api/health/services — authenticated service-tier health (200 healthy/warning, 503 critical).
  - GET /api/health/stats — authenticated health distribution statistics (counts and percentages by status tier, an uptime ratio string, performance figures).
  - GET /api/health/component/:component — authenticated single-component check; component path segment required (400 if absent); status-code maps healthy/warning→200, critical→503, else 500.
  - GET /api/health/metrics — authenticated; returns a plain-text metrics exposition (gauge lines for overall status, per-component status, response time, cache hit rate).
  - GET /api/health/ready — readiness verdict; 200 when healthy/warning, 503 otherwise.
  - GET /api/health/live — liveness verdict (always reports alive).
  - POST /api/health/check/all — authenticated; triggers a full on-demand health check and returns the report.
- Success/Error Output: success envelopes for healthy states; degraded/critical states return success:false bodies carrying the health data with the mapped status code.

#### Customer feedback — under /api/feedback/*
- Operations:
  - POST /api/feedback — authenticated; submit feedback. Inputs: required conversation identifier, customer identifier, and a rating; optional agent identifier, free-text comment, feedback type (defaults to a satisfaction type), and arbitrary metadata. Validation: missing required fields → 400; rating outside 1–5 → 400; non-existent conversation → 404. On success persists a new feedback record with a generated identifier and timestamps. Success (200): success envelope with the new identifier, conversation identifier, rating, and creation time.
  - GET /api/feedback/stats — authenticated; query: timeRange one of 24h/7d/30d/all (default 30d). Returns aggregate satisfaction stats: overall satisfaction percentage (share rated 4–5), total feedback count, average rating (one decimal), and a per-rating distribution (1–5). Returns zeroed structure when no data. Success (200).
  - GET /api/feedback/conversation/:conversationId — authenticated; returns all feedback for a conversation (with joined customer/agent display names), newest first, plus a count. Success (200).
  - GET /api/feedback — authenticated; paginated feedback list. Query: page (default 1), pageSize (default 20). Returns the page of feedback (with joined names), newest first, plus pagination metadata (page, pageSize, total, total pages). Success (200).
- Error Conditions: failures return a standardized error envelope.

#### Task reminders — under /api/reminders/*
- Operations:
  - GET /api/reminders/health — public module health verdict.
  - GET /api/reminders — authenticated; list the caller's reminders. Query: includeCompleted ("true" to include completed). Returns list and count.
  - POST /api/reminders — authenticated; create a reminder. Inputs: required title and a future reminder time; optional content, associated conversation identifier, repeat type, repeat interval. Validation: missing title/time → 400; unparseable time → 400; past time → 400. On success returns the new identifier with 201.
  - GET /api/reminders/upcoming — authenticated; query minutes (default 30) defines a look-ahead window. Returns the caller's reminders due within the window, a count, and the window size.
  - GET /api/reminders/stats — authenticated; returns the caller's reminder statistics.
  - GET /api/reminders/:id — authenticated; fetch one of the caller's reminders; 404 if not found/owned.
  - PUT /api/reminders/:id — authenticated; update one of the caller's reminders; if a reminder time is supplied it must be parseable (400 otherwise); 404 if not found/owned or update fails.
  - PUT /api/reminders/:id/complete — authenticated; mark complete; 404 if not found/owned.
  - DELETE /api/reminders/:id — authenticated; delete; 404 if not found/owned.
  - POST /api/reminders/process — authenticated AND administrator only (403 otherwise); manually triggers processing of all due reminders; returns the count processed.
- Invariants: all per-record operations are scoped to the calling user as owner; reminder processing also runs automatically on a scheduled trigger (see Real-time / Event Behavior).

#### Alert notification channel configuration — under /api/alert-config/*
- Authorization: every operation requires administrator role (non-admins receive 403).
- Operations:
  - POST /api/alert-config/channels/slack — configure a chat-webhook channel. Inputs: required webhook URL (must match the expected chat-webhook URL pattern, else 400), optional flag to send a test message. Persists the URL and a configuration-change log entry. Success returns configuration confirmation and, if requested, the test-send result.
  - POST /api/alert-config/channels/email — configure email alerts. Inputs: required mail-server host, sender address, credentials, and a non-empty recipient list; optional port (default provided), sender display name, test-message flag. Validation: any missing required field → 400; recipients must be a non-empty array → 400; sender and every recipient must be valid email addresses → 400. Persists the config and a change-log entry; optional test send. Success returns a sanitized configuration summary (host, port, sender, name, recipient count) and the test result.
  - POST /api/alert-config/channels/webhook — configure a generic outbound webhook. Inputs: required webhook URL (must parse as a URL, else 400), optional custom headers map, optional test flag. Persists URL and headers; optional test send.
  - GET /api/alert-config/channels/status — report which channels are configured (chat-webhook, email with recipient count, generic webhook) and metadata.
  - GET /api/alert-config/logs — return recent configuration-change log entries with a count (content reported as empty/fixed within the current behavioral boundary).
  - POST /api/alert-config/test-alert — send a synthetic alert across configured channels. Inputs: alert level (default a warning level; must be a recognized level, else 400), optional title and description. Returns the synthesized alert summary and per-channel notification results.
- Side Effects: persists channel configuration with long expiry and change-log entries with shorter expiry; may dispatch real notifications to external services when a test is requested.

#### Data/cache optimization administration — under /api/data-optimization/*
- Authorization: administrator role required for all except the health probe (others return 403 for non-admins).
- Operations:
  - GET /api/data-optimization/config — return current optimization configuration.
  - PUT /api/data-optimization/config — update configuration (partial). Validation bounds: cache TTL 60s–24h; max cache entries 100–100,000; batch size 10–1000; flush interval 1s–60s; retention days 1–365. Out-of-range → 400.
  - GET /api/data-optimization/stats — return query-performance stats enriched with cache-hit-rate %, batch-efficiency %, a letter performance grade, and recommendation strings.
  - POST /api/data-optimization/test-cache — benchmark cache read/write. Input: test size (default 100, must be 10–1000, else 400), optional supplied data. Returns timing and hit-rate results.
  - POST /api/data-optimization/cleanup — run cleanup. Input: force flag; if automatic cleanup is disabled and force is not set → 400; otherwise returns cleanup results.
  - POST /api/data-optimization/test-batch — benchmark batch operations. Inputs: operation count (default 50, must be 10–500, else 400), operation type one of set/get/delete/mixed (else 400). Returns timing and success-rate results.
  - POST /api/data-optimization/indexes — build a temporary query index. Inputs: required index name and field; required non-empty sample-data array (missing/empty → 400). Returns created index summary.
  - GET /api/data-optimization/indexes/:indexName/:field — query an index. Inputs: index name and field in path; required value query parameter (else 400). Returns matching records and a count.
  - GET /api/data-optimization/health — public-ish module health probe; computes a health score and grade from config flags and stats, returns status (healthy/degraded/unhealthy), component checks, statistics, recommendations, and suggested next steps.
  - POST /api/data-optimization/initialize-baseline — seed baseline statistics; if statistics already exist, returns a warning instead of reinitializing.

#### User-experience monitoring — under /api/user-experience/*
- Operations:
  - POST /api/user-experience/metrics — authenticated; record client UX metrics. Caller identity is auto-attached. Validation: requires a session identifier and a timestamp (else 400).
  - POST /api/user-experience/behavior — authenticated; record a behavior event. Validation: requires event type and timestamp (else 400).
  - GET /api/user-experience/survey/invitation — authenticated; query: required session identifier (else 400). Returns whether/which survey to present.
  - POST /api/user-experience/survey — authenticated; submit a survey. Validation: requires session identifier and an overall-satisfaction score; all five satisfaction sub-scores must be 1–5 (else 400). Returns a thank-you acknowledgment.
  - GET /api/user-experience/report — administrator only (403 otherwise); query: time range in hours (default 24, must be 1–720, else 400). Returns the aggregated UX report.
  - GET /api/user-experience/ab-tests/:testId/assignment — authenticated; returns the caller's assigned variant for the named experiment.
  - POST /api/user-experience/ab-tests/:testId/metrics — authenticated; record an experiment metric. Inputs: metric name (string) and numeric value (else 400).
  - POST /api/user-experience/ab-tests — administrator only (403 otherwise); create an experiment from a supplied configuration.
  - GET /api/user-experience/personal-dashboard — authenticated; returns a personal UX summary (values reported as fixed within the current behavioral boundary).
  - GET /api/user-experience/health — administrator only (403 otherwise); component health verdict for the UX subsystem.

#### storage-efficiency monitoring — under /api/monitoring/kv/*
- Authorization: every operation requires authentication AND administrator role.
- Operations:
  - GET /api/monitoring/kv/activity-cache — return activity-cache statistics, an optimization descriptor, and a derived health verdict.
  - GET /api/monitoring/kv/request-frequency — return per-user request-frequency tracking: top active users, flagged high-frequency users, and summary aggregates.
  - GET /api/monitoring/kv/savings — return projected/aggregate key-value-operation savings versus a prior approach.
  - GET /api/monitoring/kv/health — return an overall optimization-health verdict (healthy/warning/error) with metrics, issues, warnings, and recommendations.
  - POST /api/monitoring/kv/reset — reset all runtime monitoring counters; returns a confirmation.
- Invariants: counters are runtime-local and not durably persisted.

#### Administrative data migration — POST /api/admin/migrations/backfill-legacy-filenames
- Invocation: authenticated administrator request.
- Preconditions & Authorization: requires authentication AND administrator role (enforced for the entire migrations router).
- Inputs (query): dryRun — defaults to dry-run; only the literal value "false" performs real mutations. limit — batch size, default 50, capped at 200; must be a positive integer (else 400). cursor — opaque resume token (last processed identifier); omit for the first batch.
- Behavior: scans a bounded batch of legacy stored-file records whose stored filename lacks a usable extension, derives a corrected filename, and (unless dry-run) rewrites both the stored-object download metadata and the persisted filename so saved files open correctly. Idempotent (already-correct records are skipped) and resumable via the returned cursor.
- Success Output (200): a success flag plus a stats object: number scanned, fixed, skipped, missing-in-storage, errors, last processed identifier, next cursor, a done flag, the dry-run flag, a sample of changes, and error details.
- Error Conditions: invalid limit → 400; server failure → standardized error envelope.
- Invariants: dry-run is the safe default; a real run requires the explicit opt-in value.

#### QR join page / deactivation (auxiliary) — under the QR handler
- GET .../join — public HTML page rendering a team-join invitation. Input: a team QR token as a query value. Missing token or unknown/expired token render informational HTML pages (no JSON). Valid token renders an invitation page with the team's name, optional description, and identifier (output is HTML-escaped).
- DELETE .../:token — authenticated; deactivates a QR token; returns a success confirmation.

### Data Concepts (neutral)
- System setting: a key/value pair forming a configuration tree (groups: general identity, channel-integration connection state, advanced tuning toggles/limits). Stored as flattened dotted keys with per-key last-updated timestamps. Sensitive channel credentials are stored encrypted and never returned by read endpoints — only connection status is exposed.
- Channel credential set: per-messaging-channel secrets (tokens/ids/secrets), stored encrypted in the key-value store and decrypted on demand for live integration testing; falls back to environment-provided values when not stored.
- Customer feedback record: ties a conversation, a customer, an optional agent, a 1–5 rating, an optional comment, a feedback type, optional metadata, and timestamps. Joined to customer/agent display labels on read.
- Task reminder: owned by a single user; carries a title, optional content, a due time, optional associated conversation, optional repeat type/interval, and a completion state. Has lifecycle states pending → completed; supports overdue/upcoming categorization.
- Health/metrics snapshots: ephemeral, computed at request time from live probes and an in-process metrics collector; not durably owned by this area.
- Alert channel configuration: per-channel delivery settings (chat-webhook URL, email server/recipients, generic webhook URL+headers) persisted in the key-value store with expiry, plus change-log entries.
- Optimization configuration & stats: tunable caching/batching/indexing/cleanup parameters and derived performance statistics; indexes are temporary and reset with the runtime.
- Administrative audit activity: a record of privileged operations (notably settings updates) capturing actor identity, action type, target resource type, changed-key summary, client IP, and user-agent.

### State & Lifecycle
- Reminder lifecycle: created (must be future-dated) → optionally updated → completed (terminal for non-repeating) or deleted (terminal). Repeating reminders may regenerate per their repeat configuration. Only the owning user (or, for bulk processing, an administrator/scheduler) may transition a reminder.
- Channel integration status: connected / disconnected / error, surfaced by read/test operations; settable via settings update (validated to that enum).
- System/health overall verdict: healthy → warning/degraded → critical/outage, with HTTP status mapping (healthy/warning → 200; critical → 503; internal error → 500).
- Optimization health verdict: healthy / degraded / unhealthy derived from configuration flags and runtime stats.

### Real-time / Event Behavior
- Audit/activity emission: a successful system-settings update emits an administrative activity record (actor, changed keys, count, client IP, user-agent) consumable by the activity/audit area.
- Scheduled processing: due task reminders are processed automatically on a recurring scheduled trigger (in addition to the admin-only manual processing endpoint), dispatching reminder notifications/effects to their owners.
- Alert dispatch: configuring a channel with the test flag, or invoking the test-alert operation, dispatches real notifications to the configured external destinations and returns per-channel delivery results.
- Integration testing performs live outbound calls to external messaging-channel APIs and may send a real test message to a supplied recipient.

### Invariants & Guarantees
- Authorization tiers: most diagnostic reads require any authenticated caller; the dedicated system-administration access guard and the alert-config, data-optimization (except its health probe), KV-monitoring, UX report/experiment-create/health, reminder bulk-processing, config-check, and admin-migration operations require administrator role, returning 403 for non-admins and 401 for unauthenticated callers.
- Settings persistence is upsert-based (insert-or-replace per key); an empty update is a successful no-op.
- Settings reads never disclose channel secrets.
- Rate limiting (where the system guards apply): a general per-user window (about 120 requests/minute) plus tighter per-endpoint caps for sensitive operations (e.g., settings update ~20 per 5 minutes); exceeding a cap returns 429 with a retry hint.
- Request-size guards reject oversized bodies with 413.
- The legacy-filename migration is idempotent, resumable via cursor, and defaults to a non-mutating dry-run.
- storage-efficiency counters reset on process restart or via the explicit reset operation.
- Statistics endpoints degrade gracefully to zeroed/default values on server query failure rather than erroring.

## Rate Limiting & Mutual-Exclusion Guarantees

### Purpose
This area provides two cross-cutting infrastructure capabilities. First, it throttles incoming client traffic per caller and per logical endpoint group using a time-windowed allowance, returning standard throttling signals so clients can back off. Second, it defines the mutual-exclusion and data-consistency guarantees the system requires when critical sections may run concurrently across compute instances. The throttling capability is exposed through the operations below; the mutual-exclusion capability is expressed purely as required guarantees (this specification states what must hold, not how it is achieved).

---

### Operations

#### Rate-Limit Check (middleware-enforced) — applied to protected HTTP routes
- Invocation: Runs automatically as request-processing middleware in front of selected route groups before the route handler executes. Not a directly callable endpoint; it wraps any HTTP request reaching a guarded route.
- Inputs (derived from the incoming request, not from an explicit body):
  - Caller identity: by default the client network address, resolved by preferring a trusted edge-provided origin-address header, then the first entry of a forwarded-for header, then a real-IP header, else the literal value "unknown". A custom identity extractor may be configured per guard.
  - Endpoint group key: a short label classifying the route family (e.g. general API, authentication, login, upload, websocket, admin, or a custom label). Used to keep counters for different route families independent.
  - Policy: a maximum-requests allowance and a window length in milliseconds. Selected from named presets or supplied explicitly.
- Preconditions & Authorization: None of its own; it executes regardless of authentication and typically runs before auth. If the backing throttling capability is unavailable, the request is allowed through (fail-open) without counting.
- Behavior (observable order): The middleware evaluates allowance for the endpoint group and the individual caller identity, then either (a) attaches throttling headers and lets the request proceed, or (b) short-circuits with a throttled response. On any server failure during the check, default behavior is fail-open (request proceeds); a guard may instead be configured to fail-closed.
- Success Output (request allowed): The downstream handler runs normally. The response carries headers: the configured limit, the remaining allowance in the current window, and the window-reset time (epoch-millis-style numeric value). No body is added by the middleware on the allowed path.
- Throttled Output (request blocked): HTTP 429 with a JSON body containing: a generic error label indicating the limit was exceeded, a human-readable message stating how many seconds to wait, the configured limit, the window length expressed as a seconds string, and a retry-after seconds value. The same limit/remaining/reset headers are set (remaining shown as 0) plus a retry-after header in seconds (minimum 1). An optional per-guard callback is invoked on block (used for security logging of auth/login throttling).
- Side Effects: Updates transient per-caller throttling state; no database writes are observable on the request path. Diagnostic snapshots may be exposed through monitoring.
- Error Conditions:
  - Backing throttling capability unavailable -> request allowed (fail-open), warning logged.
  - Any exception during the check, default config -> request allowed (fail-open).
  - Any exception during the check, fail-closed config -> HTTP 503 with a JSON body containing a service-unavailable error label and a try-again message.
- Invariants & Guarantees: Allowance is counted per individual caller identity (the resolved client address, or a custom identity if configured) and per endpoint group; different callers and different endpoint groups never share allowance. All callers that cannot be identified collapse to a single shared "unidentified" identity and share one allowance. Throttling allowance recovers gradually rather than resetting abruptly at a fixed boundary, so traffic is smoothed across the boundary.

##### Named Rate-Limit Policies (presets)
These are the standard allowance/window pairs available to guards:
- General API: 100 requests per 60 seconds.
- Authentication: 10 requests per 60 seconds.
- Login (strictest): 5 requests per 5 minutes.
- Upload: 20 requests per 60 seconds.
- Websocket: 30 requests per 60 seconds.
- Admin: 200 requests per 60 seconds.
- High-frequency: 500 requests per 60 seconds.
Pre-built guards exist for authentication, login, upload, websocket, and admin route families; the auth and login guards additionally emit security warnings on each block (login labeled as possible brute-force).

---

The throttling subsystem keeps runtime counters that are not directly addressable by external clients. Their only externally observable manifestations are:

- **On every checked request:** the `X-RateLimit-Limit`, `X-RateLimit-Remaining`, and `X-RateLimit-Reset` response headers (and, when blocked, a `Retry-After` header of at least 1 second).
- **On a blocked request:** HTTP 429 with the JSON body documented above (an error label, a human-readable wait message, the limit, the window in seconds, and a retry-after in seconds).
- **Via an operational statistics view:** a JSON snapshot reporting the number of distinct callers currently tracked, cumulative total checks, cumulative blocked count, the number of currently tracked entries, the time of the last persistence checkpoint, and uptime.

Operational facilities also exist to prune idle counters and to reset counters; these have no client-facing contract beyond the statistics view above.

---

### Mutual-Exclusion Guarantees (Concurrency & Data Consistency)

Certain critical sections may execute concurrently across multiple compute instances. For these, the system MUST provide the following mutual-exclusion and data-consistency guarantees. This specification deliberately states only the required guarantees; it does NOT prescribe any mechanism. A conforming implementation may satisfy them by any means.

1. **Single-holder guarantee.** For any resource identified by a unique key (for example a specific customer or a specific conversation), at any given instant at most one execution context (thread or node) may perform a mutating change to that resource. Concurrent contenders for the same resource are serialized; their changes are never applied in parallel.

2. **Lease & expiration guarantee.** Any exclusivity obtained over a resource MUST be time-bounded by an explicit maximum lifetime (a lease). On unexpected interruption — process crash, network partition, or timeout — the exclusivity MUST be released automatically when its lease lapses. Permanent deadlock is prohibited: a resource can never become exclusively held indefinitely.

3. **Safe-release guarantee.** An exclusivity may be ended early only by the same logical owner that obtained it, or reclaimed by the expiry mechanism once its lease has lapsed. No other actor may release an exclusivity it does not own (no cross-owner release).

4. **Observable contention behavior.** While a resource is exclusively held, a subsequent contender for the same resource MUST be made to wait for a bounded time for the holder to finish; if that bounded wait elapses without the contender obtaining exclusivity, the attempt is abandoned (it reports failure) rather than proceeding without exclusivity. (Answering a contender immediately with a standard concurrency-conflict result — for example HTTP 409 or 429 — instead of waiting is a permitted alternative.) In no case may a contended request corrupt the integrity of the already-held resource or partially apply its change.

5. **Guaranteed relinquish.** A critical section performed under this guarantee MUST relinquish its exclusivity afterward in every outcome — on success, on operation error, and on exceeding any execution-time limit.

Critical sections that depend on these guarantees are identified where they occur in this specification — for example, per-customer serialization of conversation creation (so concurrent inbound deliveries never produce duplicate open conversations) and single-processor draining of each event queue (so a queued event is delivered by at most one processor at a time).

### Data Concepts (neutral)

- **Caller counter (rate limiting):** Per protected scope, the system tracks enough recent caller activity to report remaining allowance, reset time, total checks, and total blocks. Activity older than the recovery horizon stops affecting allowance; retained diagnostic state is bounded and not user data. Counters are not user data and carry no soft-delete semantics — they simply expire.

- **Rate-limit policy:** A pairing of maximum-requests and window-length, chosen from named presets or supplied per guard. Applied independently per endpoint-group label and per individual caller identity.

- **Resource-exclusivity (conceptual):** The mutual-exclusion guarantees above operate over named resources identified by a unique key. This specification describes no record structure, holder representation, or lease store for them — only the guarantees in the preceding subsection, which a conforming implementation may realize however it chooses.

---

### State & Lifecycle

**Caller counter lifecycle:** new -> actively tracked within a rolling window -> (idle past threshold: pruned). There is no terminal "blocked" state; a caller blocked now becomes allowed again once enough of the rolling window has advanced that its tracked activity falls below the limit.

---

### Real-time / Event Behavior

- No client-facing real-time/WebSocket events are emitted by this area. Its outputs are synchronous HTTP responses and headers.
- **Throttling signal to clients:** On block, callers receive HTTP 429 plus a retry-after seconds value (and header), the configured limit, the window expressed in seconds, and a wait message — enabling client back-off.
- **Rate-limit accounting background activity:** Periodically persists its counters so they survive restarts, and periodically prunes inactive or over-cap entries.
- **Security observability:** The authentication and login throttling guards emit warning-level security log entries on each block (login blocks annotated as possible brute-force attempts), carrying the caller address, request path, and retry-after value.


---

# 7. Cross-cutting Backend

## Request Pipeline, Routing & Conventions

### Purpose
This area defines the cross-cutting contract every HTTP request to the platform passes through: the cross-origin policy, the fixed ordering of pipeline stages a request traverses, the precedence rules that decide which handler answers a given path, the uniform shape of success and error responses, the catalog of machine-readable error codes and status codes returned to callers, request metrics collection, and the global numeric limits (sizes, counts, timeouts, rate limits) that bound all operations. It does not implement any single business feature; it is the wire-level convention layer that all other operations inherit.

### Operations

The following are pipeline-level behaviors and a small set of generic infrastructure endpoints. Domain endpoints are documented in their own sections; here only the conventions they all obey are specified.

#### Cross-Origin Preflight Handling — OPTIONS (any path under the API origin)
- Invocation: any browser-issued CORS preflight (HTTP method OPTIONS) to any path.
- Inputs: the request's origin header.
- Preconditions & Authorization: none; preflight is processed before authentication.
- Behavior: the origin is checked against the allowed-origins policy (see Data Concepts). If permitted, an empty preflight acknowledgment is returned that advertises the allowed methods, allowed request headers, credential support, and a preflight cache lifetime; the response is explicitly marked non-cacheable at the edge. If not permitted, a structured cross-origin rejection is returned instead.
- Success Output: empty body, status 204, with cross-origin acknowledgment headers (allowed methods: GET, POST, PUT, DELETE, OPTIONS, PATCH; allowed headers include content type, an authorization header, a requested-with header, an accept header, a session identifier header, a conversation-context header, and a team-context header; credentials allowed; preflight cache lifetime 86400 seconds).
- Error Conditions: disallowed origin -> status 403 with a structured cross-origin error body (see below) and a header echoing the machine error code.
- Invariants & Guarantees: preflight responses are never edge-cached; the cross-origin check runs ahead of all business logic and all authentication.

#### Cross-Origin Response Decoration — every non-OPTIONS request
- Invocation: automatic, wraps every request that is not a preflight.
- Behavior: the business handler runs first; afterward, if the request's origin is permitted, the response is decorated with an allow-origin header echoing the caller's origin and a credentials-allowed header. A disallowed origin does not block the request from executing but the permissive cross-origin headers are omitted (so a browser will block the caller from reading the result).
- Invariants & Guarantees: cross-origin headers are added after the handler completes; same-origin and non-browser callers (no origin header) are unaffected.

#### Cross-Origin Rejection Detail (structured error body)
- Invocation: returned when a browser origin is rejected at preflight time.
- Success Output: not applicable.
- Error Conditions: status 403, JSON body containing: a human-readable error label, a machine code that is one of `CORS_ORIGIN_NOT_ALLOWED` (origin simply not on the list) or `CORS_CONFIGURATION_MISSING` (server-side configuration is incomplete in a production deployment), a descriptive message, the rejected origin, the current list of permitted origins, a boolean flagging whether this is a server-misconfiguration vs. a policy rejection, a remediation object (ordered human steps plus a documentation link), and a timestamp. A response header also carries the machine code. The body is non-cacheable.

#### Configuration Completeness Guard
- Invocation: a guard stage that may front requests when enabled.
- Preconditions: only enforced in a production deployment; in non-production it never blocks.
- Behavior: verifies that the mandatory deployment settings (a frontend origin URL and a backend origin URL) are present. If any are missing in production, the request is rejected with a configuration-incomplete error, EXCEPT for an allow-list of always-reachable health/diagnostic paths (the system health path, the configuration-check path, and the WebSocket health/liveness/readiness paths), which are permitted to proceed even when configuration is incomplete. A separate optional setting (a public storage URL) only produces a warning, never a block.
- Error Conditions: missing required production settings on a non-exempt path -> status 503 with a JSON body containing a `CONFIGURATION_INCOMPLETE` error label, the detected environment name, the list of missing settings, any warnings, an ordered remediation guide with a documentation link, the requesting origin, and a timestamp; response carries a configuration-status header and is non-cacheable.

#### Request Metrics Capture
- Invocation: automatic for every request whose path is under the API namespace, positioned after the cross-origin stage and before authentication.
- Behavior: preflight (OPTIONS) requests are skipped; non-API paths are skipped; a fixed set of monitoring/health and static-asset paths are skipped to avoid feedback loops. For all other API requests, the handler runs and then, without blocking or delaying the response, a metrics record is emitted asynchronously containing the request method, a normalized path (dynamic identifier segments — numeric ids, UUID-form ids, and long hex ids — are collapsed to a single placeholder segment so cardinality stays bounded), the final response status code, the elapsed handling time in milliseconds, and a timestamp.
- Invariants & Guarantees: metrics emission can never alter or fail the request; any internal error in this stage is swallowed and the request proceeds normally. Path normalization guarantees that per-entity routes aggregate into one metric key.

#### Generic Rate Limiting (reusable policy applied to selected route families)
- Invocation: a configurable guard that selected route families opt into; keyed by caller IP (derived from the connecting-IP header, then a forwarded-for header, then a real-IP header, falling back to an "unknown" bucket) within a named scope.
- Inputs (policy parameters): a maximum request count and a sliding time window; named presets exist — a standard preset (100 requests/60s), an authentication preset (10/60s), a stricter login preset (5/300s), an upload preset (20/60s), a websocket preset (30/60s), an admin preset (200/60s), and a high-frequency preset (500/60s).
- Behavior: each qualifying request consumes from a per-scope, per-caller sliding counter. Every response (allowed or not) carries rate-limit headers: the limit, the remaining allowance, and the window reset time. When the allowance is exhausted, the request is rejected and additionally carries a retry-after header.
- Success Output: when within limit, the request proceeds normally with informational rate-limit headers attached.
- Error Conditions: over limit -> status 429 with a JSON body containing an error label, a human message naming the seconds to wait, the limit, the window expressed in seconds, and the retry-after seconds.
- Invariants & Guarantees: if the rate-limit backend is unavailable, the default behavior is fail-open (request allowed) unless a strict mode is selected, in which case unavailability yields status 503. Counting is enforced over a rolling time window and is tracked per individual caller; different callers do not share an allowance (callers that cannot be identified collapse to a single shared "unknown" identity).

#### Security Header Decoration — every request
- Invocation: automatic, applied to every response after the handler runs.
- Behavior: a standard set of protective response headers is attached: content-type-sniffing protection, frame-embedding denial, legacy XSS protection, a referrer policy, a permissions policy disabling camera/microphone/geolocation, and a content-security policy. A strict transport-security header is added only when the request was served over a secure transport.

#### Service Root Probe — GET /
- Invocation: unauthenticated client request.
- Behavior: returns a liveness greeting.
- Success Output: status 200, JSON with a greeting message, a timestamp, and a version string.

#### Unknown Route Fallback — any unmatched path
- Invocation: automatic when no registered route matches.
- Success Output: not applicable.
- Error Conditions: status 404, JSON body with an error label of "Not Found", a message stating the requested endpoint was not found, and a timestamp. (Note this fallback envelope differs from the standard envelope — it has no `success` flag — and applies only to fully unmatched paths.)

#### Global Error Trap — any thrown error
- Invocation: automatic; catches any error thrown by any handler in the pipeline.
- Behavior: the error is logged with request path and method, then mapped to a uniform error response based on its type (see Error Taxonomy in State & Lifecycle). Recognized typed errors map to their dedicated status code and machine code; unrecognized errors map to a generic internal error.
- Success Output: not applicable.
- Error Conditions: see the Error Taxonomy table.

### Data Concepts (neutral)

**Canonical success envelope.** Every conventional success response is a JSON object carrying: a boolean success flag (true), an optional data payload of arbitrary shape, an optional human message string, a timestamp, and a per-response correlation identifier (an opaque request id string). Default success status is 200; creation flows use 201; empty results use 204.

**Canonical paginated envelope.** A specialization of the success envelope where the data payload is an object carrying: the list of items, the current page number, the page size (exposed under both a "page size" field and a "limit" field), the total item count, the total number of pages, and two booleans indicating whether a next/previous page exists. Also carries a timestamp and correlation id.

**Canonical error envelope.** Conventional error responses carry: a boolean success flag (false), an error message string, and a timestamp and correlation id. Validation failures additionally embed, under the data payload, a validation error code plus an array of field-level problems (each problem names the offending field, a message, and optionally the offending value).

**Allowed-origins policy.** A dynamically computed set of permitted browser origins. In non-production deployments it auto-includes a fixed local-development set (localhost and loopback on common dev ports, plus the local worker dev port, over both http and https). In all deployments it includes the configured frontend origin, the configured backend origin, an optional public-storage origin, and optionally a comma-separated list of extra origins and a preview-deployment origin from environment configuration. Additionally, any origin matching the platform preview-deployment hostname pattern (a hex commit prefix on a project preview domain) is accepted. In production with no origins configured, requests warn and effectively reject browser callers.

**Correlation identifier.** Each conventional response carries an opaque, unique-per-response id used purely for log correlation; it is not a durable resource handle.

**Global numeric limits (bounds every operation inherits).**
- Sizes: default upload max 5 MB; absolute upload max 10 MB; inbound webhook payload max 5 MB; generic API request body max 1 MB; message content max ~10 KB.
- Platform message length caps: one messaging platform 5000 chars, another 2000 chars, system-originated messages 10000 chars.
- Pagination: default page size 20; min page size 1; max page size 100; max page number 1000; a list view default of 50; search result cap 100. Out-of-range page/size values are clamped into the valid range rather than rejected.
- Bulk operation caps: bulk create/delete up to 100 items; forward fan-out up to 20 targets; tags-per-item up to 10; export between 1 and 5000 records.
- Timeout behavior (informative): caller-visible requests and real-time interactions are bounded; slow operations fail or degrade rather than blocking indefinitely. File uploads and batch operations may take materially longer than ordinary API calls.
- Retry behavior (informative): transient failures may be retried a small number of times with increasing delay; exhausted retries surface as the documented operation failure.
- Rate limits: default window 60s; max requests per window 100 in production, 1000 in development; websocket message rate window 1s with max 10 messages/second.
- Cache lifetimes: session cache 24h; default cache 1h; cross-origin preflight cache 24h; auth token lifetime 24h.
- String length caps: filename 30 (display 25), username 50, email 255, short description 100, long description 1000, title 200, tag name 50.

### State & Lifecycle

**Request pipeline stage order (fixed).** A request is processed through these stages in this exact order, and this order is load-bearing for correctness:
1. Cross-origin stage: handles preflight outright; otherwise records the origin decision for later response decoration.
2. Metrics stage: scoped to the API namespace, captures timing/status non-blockingly.
3. Public/priority endpoints (public health endpoints, signed file/storage proxies, webhooks, real-time connection endpoints, and other explicitly mounted routers) — these take precedence so they cannot be shadowed by broader patterns.
4. The routes for the majority of domain modules.
5. Fine-grained individual routes and additional routers.
6. Lazy one-time initialization stage and an optional first-request warm-up stage.
7. A general error-trap stage.
8. Security-header decoration stage (runs post-handler).
9. Service root, static template responses, and the unknown-route fallback.

**Route precedence rule (critical invariant).** Stated as observable precedence: public endpoints (those that must bypass authentication — webhooks verified by signature, public health checks, signed public file/storage proxies, real-time connection upgrades, public team-join and invite-link pages) MUST remain reachable without authentication. When a public path overlaps a broader authenticated pattern, the documented public response takes precedence over the authenticated catch-all; if that precedence is not preserved, such a request is wrongly answered with an authentication rejection (401) instead of the intended public response. Within authenticated families, authentication applies to both the base path and all of its nested sub-paths.

**Error taxonomy (typed error -> wire result).** The global error trap maps recognized error types to fixed outputs:
- Validation failure -> status 422, machine code `VALIDATION_ERROR`, with field-level problem array embedded.
- Authentication required / invalid -> status 401, machine code `UNAUTHORIZED`.
- Permission denied -> status 403, machine code `FORBIDDEN`.
- Resource not found -> status 404, machine code `NOT_FOUND`, message names the missing resource.
- Bad request -> status 400, machine code (validation-class).
- Conflict / duplicate -> status 409, machine code `CONFLICT`.
- Too many requests -> status 429, machine code `TOO_MANY_REQUESTS`, includes a retry-after value and header when known.
- Any other application error -> its declared status code with its declared machine code.
- Unrecognized/unexpected error -> status 500, machine code `INTERNAL_ERROR`, with a generic internal-error message.

**Full machine error-code vocabulary** exposed to callers: `UNAUTHORIZED`, `FORBIDDEN`, `TOKEN_EXPIRED`, `INVALID_CREDENTIALS`, `VALIDATION_ERROR`, `REQUIRED_FIELD`, `INVALID_FORMAT`, `NOT_FOUND`, `ALREADY_EXISTS`, `RESOURCE_CONFLICT`, `INTERNAL_ERROR`, `SERVICE_UNAVAILABLE`, `RATE_LIMIT_EXCEEDED`, plus the cross-origin codes `CORS_ORIGIN_NOT_ALLOWED` and `CORS_CONFIGURATION_MISSING`, and the configuration code `CONFIGURATION_INCOMPLETE`.

**Route-parameter validation (reusable guards).** Reusable guards can front a route to validate path parameters before the handler runs:
- Positive-integer parameter guard: the named path segment must parse to an integer greater than zero. Missing -> status 400 with a "missing required parameter" message naming the parameter; non-integer or non-positive -> status 400 with an "invalid <name>: must be a positive integer" message echoing the raw value. On success the parsed value is made available to the handler.
- Non-empty string parameter guard: the named path segment must be present and non-blank, must not exceed a maximum length (default 255), and may optionally be required to match a format pattern. Missing/blank -> status 400 "missing required parameter"; too long -> status 400 "exceeds max length"; pattern mismatch -> status 400 "invalid format". On success the value is made available to the handler. (These guard responses use the minimal `{success:false, error}` shape rather than the full canonical envelope.)

**Infrastructure/diagnostic endpoints (cross-cutting).**
- A route-registry health/inventory endpoint reports overall routing health, configuration validation, route documentation, and counts of total/enabled/disabled routes.
- A configuration-check endpoint (admin-only) reports per-setting presence and remediation guidance.
- A signed public storage proxy requires a valid signature and expiry on the URL; a bare key without a valid signature is treated as not found (404), and on success streams the object with appropriate content headers and cross-origin headers for permitted origins.

### Real-time / Event Behavior
This conventions layer itself emits no domain WebSocket events, but it governs the entry points: real-time connection-upgrade endpoints are public (no JWT) and take precedence over broader authenticated patterns so the upgrade handshake is not intercepted by authenticated catch-alls, while sensitive real-time metrics/detail endpoints in the same namespace are individually guarded by authentication. The metrics stage records per-request observability data (method, normalized path, status, elapsed time, timestamp) asynchronously and never affects the response. Scheduled and background-processing entry points exist alongside the HTTP entry point and follow the same global limits and error conventions, but emit no synchronous response to an external caller.

## Conceptual Data Model & Credential Security

### Purpose
This area defines the persistent information model for the entire multi-channel customer-support platform and the protection of secret integration credentials while stored. It establishes the conceptual entities (organizational units, support staff, end customers, conversations, messages, labels, channel connections, reporting artifacts, automated-reply configuration, and audit trails), how they relate, their lifecycle and recoverability semantics, and which data is held in an encrypted, tamper-evident form rather than as readable text. It is not itself a set of network endpoints; it is the shared data foundation that all other operations read and write, plus the guarantees governing how secret integration credentials are protected at rest.

### Credential Protection (at rest) — guarantees

Secret integration credentials (platform access tokens, signing secrets, and similar) are protected while stored. This area is internal (no client-reachable endpoints); the specification states only the observable guarantees, not how protection is implemented. A conforming implementation may use any mechanism that satisfies them.

1. **Not readable at rest.** A stored credential is held in a protected, tamper-evident form from which the original value cannot be recovered without the configured protection key, and is never returned in any client-facing response.
2. **Non-deterministic protection.** Protecting the same input twice yields different protected values, so stored secrets cannot be correlated by equality.
3. **Tamper detection.** If a protected value is altered, an authorized read fails rather than returning corrupted or partial data.
4. **Authorized read returns the original.** A read performed with the correct key returns the exact original value.
5. **Mixed-format tolerance.** Both protected values and historically stored plaintext values can be read, so protection may be adopted incrementally; whether a newly stored value is protected depends only on whether protection is configured (when it is not, the value may be stored unprotected, accompanied by a warning).
6. **Documented error behavior.** If protection is not configured, or the supplied key/material is invalid, the documented error is reported and no partial result is returned.

A setup facility can also produce a fresh random protection key.

### Data Concepts (neutral)

All persisted records use string-based timestamps. Most core entities carry an automatic creation timestamp and many carry an update timestamp. The following describes each conceptual entity with renamed, neutral terms and its carried information.

- **Organizational unit (team).** A support group. Carries a numeric identifier, a name, an optional description, an optional onboarding image reference, an "enabled" flag distinguishing temporary disablement from removal, and soft-delete marker. Many other entities reference an organizational unit.

- **Support staff member (agent).** A human operator. Carries a string identifier, a unique login email, a one-way-hashed password digest (a non-reversible hash, not reversible encryption), a display name, a system-level role (administrative or ordinary operator — a two-tier scheme), an enabled flag, a password-change policy indicator, last-active and last-login timestamps, and a soft-delete marker. A staff member's team membership is not stored on the staff record itself; it is expressed through the membership link entity below.

- **Staff-to-unit membership (junction).** Connects one staff member to one organizational unit, allowing a staff member to belong to many units at once. Carries the staff identifier, the unit identifier, a role-within-unit indicator (ordinary member, lead, or supervisor), a "primary unit" flag marking the staff member's main unit, and a join timestamp. A staff member may not be linked to the same unit twice (uniqueness enforced on the staff/unit pair). Removing either side cascades to remove the membership.

- **End customer (customer).** A person reaching out from an external messaging platform. Carries a numeric identifier, a platform name (e.g., a chat platform), the platform-specific user identifier, an optional display name, an optional avatar reference, optional email and phone (noted as personal data candidates for protection), an optional originating-unit reference, and a free-form structured metadata blob (may contain personal data). Soft-deletable. The combination of platform plus platform-user-identifier is unique (one customer record per external identity per platform).

- **Onboarding code (QR code).** A scannable code tied to an organizational unit for acquiring customers. Carries a string identifier, the owning unit, a unique token, an external link, an image reference, an optional campaign label, an optional description, a usage counter, an optional maximum-use cap, an enabled flag, and an optional expiry timestamp.

- **Onboarding-code scan record.** An append-only log of a scan event. Carries a string identifier, the related onboarding code, an optional resolved customer, the platform, the optional platform-user-identifier, optional scan metadata, and a scan timestamp.

- **Unit-level onboarding link (LIFF-style).** A persistent onboarding link/image per organizational unit. Carries a string identifier, the owning unit (one per unit — uniqueness enforced), a full onboarding link, an image reference, a scan counter, and an enabled flag.

- **Customer-to-unit assignment record.** Tracks which unit a prospective customer was routed to from an onboarding scan, recorded by platform-user-identifier even before the customer record fully exists. Carries a string identifier, the platform-user-identifier, the unit, an optional originating onboarding link, a source indicator (onboarding scan, manual, import, or inbound event), an optional display name, an assignment timestamp, and metadata. The combination of platform-user-identifier plus unit is unique.

- **Conversation.** A support thread with one customer. Carries a string identifier, the related customer (which cannot be removed while a conversation references it), an optional assigned organizational unit, a status, a priority, first-response and closed timestamps, last-message and last-viewed timestamps, and a soft-delete marker. Conversations are assigned at the unit level only (no individual-staff assignment).

- **Message.** A single communication within a conversation. Carries a string identifier, the parent conversation (removed in cascade if the conversation is removed), a sender category (customer, staff, or system-generated), an optional customer-sender reference, an optional staff-sender reference, the textual content, a content type (default textual), an optional platform-side message identifier, a "recalled" flag with an associated recall-deadline and recalled timestamp, a "sent" flag with sent timestamp, a delivery-status indicator, an optional reference to another message it replies to (a self-reference enforced only by application logic, not by a database constraint), optional thread and session grouping identifiers with a per-session sequence number, a metadata blob, a persisted sender-name snapshot (preserved even if the sending account is later renamed or removed), a record of which staff have read it, and a soft-delete marker plus update timestamp.

- **Scheduled/delayed outgoing message.** A message queued to send later. Carries a string identifier, the target conversation, the authoring staff member, content and content type, a scheduled-send time, sent and cancelled timestamps, a status, and metadata. Cascades away if the conversation or staff member is removed.

- **File attachment.** A stored file associated with a message and/or conversation. Carries a string identifier, optional links to a message and a conversation (both detach rather than cascade on removal), original filename, content type, file size, an optional readable file link, an internal object-storage key, an upload-status indicator, and an optional uploader reference.

- **Conversation session.** A logical activity window within a conversation. Carries a string identifier, the parent conversation, a session type, an optional topic, start/end and last-activity timestamps, a message count, and an active flag.

- **Conversation transfer record.** An append-only record of moving a conversation between organizational units. Carries a numeric identifier, the conversation, optional source and destination units, an optional reason, the staff member who performed it (which cannot be removed while referenced), and a transfer-type indicator. Only unit-to-unit transfers are recorded (no individual-staff transfers).

- **Message-recall audit record.** An append-only record of recall actions on a message. Carries a numeric identifier, the message, the acting staff member, and an action descriptor.

- **Notification.** An in-app notice to a staff member. Carries a string identifier, the recipient staff member, a type, a title, body content, an optional structured data blob, a read flag with read timestamp, and an optional expiry timestamp.

- **Label (tag).** A categorization marker. Carries a numeric identifier, a name, a color (default a blue shade), an optional description, an optional owning unit, an enabled flag, and the creating staff member (which cannot be removed while referenced). Soft-deletable. The combination of name plus owning unit is unique.

- **Customer-label link and Conversation-label link (junctions).** Each associates a label with a customer or a conversation respectively, recording who assigned it (cannot be removed while referenced) and when. Removing either linked entity or the label cascades to remove the link. Uniqueness is enforced by the composite of the two linked identifiers.

- **Activity audit record.** An append-only audit trail entry. Carries a numeric identifier, the acting staff member (cannot be removed while referenced), a snapshot of the actor's name and role, an action verb, the affected resource type and optional resource identifier, an optional details blob, and the originating network address and client descriptor.

- **System setting.** A key/value configuration entry keyed by a setting name with a stored value.

- **Operational metric.** A numeric measurement sample. Carries a numeric identifier, a metric name, a numeric value, a numeric timestamp, optional structured tags, and an optional unit.

- **Channel integration (multi-tenant channel connection).** The connection between an organizational unit and an external messaging platform. Carries a numeric identifier, the owning unit, the platform name, a non-sensitive configuration blob (platform-specific public settings), a **sensitive credentials blob held in protected/encrypted form** (e.g., access tokens and signing secrets), a webhook configuration blob (inbound URL and token), a consolidated usage-statistics blob, an enabled flag, a verified flag with last-verified timestamp, the configuring staff member (detaches on removal), a configuration-metadata blob, a last-error blob, and an error counter. New platforms can be supported without structural changes because per-platform data lives inside the flexible blobs. Removing the owning unit cascades to remove its integrations.

- **Inbound-channel security event.** An append-only record of a webhook-related security concern. Carries a string identifier, an event type, a severity (one of low/medium/high/critical), the platform, an optional related integration (cascades on integration removal), an optional source network address, and a details blob.

- **Cross-origin-access event.** An append-only record of a browser cross-origin decision. Carries a string identifier, an outcome type (one of: allowed, rejected, preflight, streaming-connection, credentials-used), the requesting origin, optional method/path/client-descriptor/network-address, a timestamp, and a metadata blob.

- **Report artifact.** A generated report. Carries a string identifier, a title and optional description, a report type, an output format (structured data, spreadsheet, document, web, or delimited), a status, the owning staff member (cannot be removed while referenced) and optional owning unit, time-range and filter/option descriptors, generation/completion/failure timestamps, an error message, an execution duration, an output link and size, a download counter with last-download timestamp, an expiry timestamp, and a soft-delete marker.

- **Scheduled report definition.** A recurring report configuration. Carries a string identifier, a name and description, report type/format/parameters, a schedule type (daily/weekly/monthly/custom) and schedule configuration with timezone, an enabled flag, retry settings (max attempts, delay), owning staff member (cannot be removed while referenced) and optional unit, notification preferences (notify-on-success, notify-on-failure, and a list of recipient emails), and execution-tracking fields (next/last run times, last status, run count). Soft-deletable.

- **Scheduled-report execution record.** An append-only run history entry. Carries a string identifier, the parent schedule (cascades on removal), start/completion timestamps, an execution status (running/success/failed/cancelled), a duration, an optional reference to the report artifact produced, an error message, and a retry counter.

- **Report-download history record.** An append-only record of a report download. Carries a string identifier, the related report (cascades on removal), the downloading staff member (cannot be removed while referenced), download timestamp, originating network address and client descriptor, a download-method indicator (manual/scheduled/programmatic), and a download size.

- **Report template.** A reusable report layout. Carries a string identifier, a name and description, a report type, a template-configuration blob, an optional preview-image reference, an optional category and label list, system-template and public flags, the creating staff member (cannot be removed while referenced) and optional owning unit, and usage tracking (use count, last-used time). Soft-deletable.

- **Task reminder.** A staff-facing reminder. Carries a string identifier, the owning staff member (cascades on removal), a title and optional content, a remind-at time, an optional related conversation, a repeat type (none/daily/weekly/monthly) and interval, completed and sent flags, and completion/sent timestamps.

- **Customer feedback.** A satisfaction record. Carries a string identifier, the related conversation and customer (both cascade on removal), an optional related staff member, a numeric rating constrained to a 1–5 range, an optional comment, a feedback-type indicator (satisfaction, service quality, response time), and a metadata blob.

- **Automated-reply rule.** A rule that auto-responds to inbound messages. Carries a numeric identifier, an optional owning unit, a name, a trigger type (welcome / keyword / off-hours / fallback), a priority where a lower number means higher precedence, an enabled flag, an opt-in flag controlling whether a fallback delivery method may be used when the primary delivery fails (defaults off to conserve quota), and the creating staff member. Soft-deletable.

- **Automated-reply condition.** A matching criterion attached to a rule (one rule to many conditions). Carries a numeric identifier, the parent rule (cascades on removal), a condition type (exact / contains / pattern / message-type), a comparison value, a case-sensitivity flag, and a combination mode (match-any vs match-all).

- **Automated-reply action.** A response action attached to a rule (one rule to many actions). Carries a numeric identifier, the parent rule (cascades on removal), an action type (textual reply / image reply / rich reply), a structured content blob, and an ordering index.

- **Automated-reply business-hours schedule.** Per-unit, per-weekday operating hours. Carries a numeric identifier, the owning unit (cascades on removal), a day-of-week (0=Sunday through 6=Saturday), start and end times in HH:MM form, a timezone, and an enabled flag. Each unit may have only one schedule per weekday (uniqueness on unit plus weekday).

- **Automated-reply audit log.** An append-only record of an auto-reply that fired (no soft-delete). Carries a numeric identifier, optional references to the rule, conversation, and customer (all detach on removal), the triggering inbound content, the response content sent, a record of which condition matched, the platform, and the delivery method used (primary reply vs fallback push).

- **Automated-reply delivery ledger.** An idempotency ledger for webhook-triggered auto-replies. Carries a numeric identifier, the platform, the platform-side message identifier, optional rule/conversation/customer references (detach on removal), a status (pending/success/failed), the delivery method, an attempt counter, a last-error string, last-attempt and sent timestamps. The combination of platform plus platform-side message identifier is unique, preventing the same inbound message from being auto-replied to more than once.

- **Authenticated session descriptor.** A logical authenticated session carrying the user identifier, username, role, optional primary-unit identifier, and an expiry instant. (Conceptual; no particular storage model is implied.)

- **Protected-secret representation.** The on-storage form of an encrypted secret is an opaque protected object from which the plaintext cannot be derived without the configured key. Stored values for channel-integration credentials are either this protected form or, in legacy cases, plaintext, and readers tolerate both.

### State & Lifecycle

**Soft-delete and recoverability conventions.** A common pattern distinguishes two orthogonal lifecycle aspects:
- An "enabled/disabled" flag means temporarily disabled but recoverable; the record remains a live, normal record.
- A soft-delete marker means logically removed; such records should be treated as absent by normal reads but remain physically present for audit and possible restoration.

Entities supporting soft delete include: organizational units, staff members, customers, conversations, messages, labels, report artifacts, scheduled report definitions, report templates, and automated-reply rules. Append-only logs and audit/event/junction records generally do NOT support soft delete (activity audit, recall audit, security events, cross-origin events, scan records, auto-reply audit logs, execution histories, download histories, transfer records). The automated-reply audit log is explicitly append-only.

**Conversation status lifecycle.** A conversation moves through statuses including: active, assigned, pending, in-progress, waiting, and closed. The default initial status is active. A closed status records a closed timestamp; closure is the effective terminal state. Conversations also track a priority (low / normal / high / urgent, defaulting to normal).

**Message recall lifecycle.** A message starts as sent and not recalled, with a delivery status (default delivered). It may transition to recalled within a recall deadline window, which sets a recalled timestamp and the recalled flag; recall actions are independently audited.

**Report lifecycle.** Report artifacts progress through pending → generating → completed or failed, recording the corresponding timestamps and, on completion, an output link and size. Scheduled-report runs progress through running → success / failed / cancelled.

**Delayed-message lifecycle.** A scheduled outgoing message starts pending and reaches a sent or cancelled outcome, recording the respective timestamp.

**Auto-reply delivery lifecycle.** A delivery-ledger entry starts pending and reaches success or failed, accumulating attempt counts and last-error/last-attempt details; uniqueness on the platform message identifier guarantees at-most-once auto-reply per inbound message.

**Referential removal behavior (observable consequences).**
- Cascade (dependent records vanish with the parent): removing an organizational unit removes its memberships, onboarding-related links, channel integrations, business-hours schedules; removing a conversation removes its messages, sessions, transfers, delayed messages, label links, feedback; removing a rule removes its conditions and actions; removing a staff member removes their notifications, reminders, and delayed messages.
- Detach/null (reference cleared, record survives): originating-unit on customers, assigned-unit on conversations, configuring staff on integrations, sender references on messages, optional rule/conversation/customer references on auto-reply logs and ledgers, and several optional report ownership references.
- Restrict (parent cannot be removed while referenced): a customer with conversations, and creator/actor references on labels, audit records, transfers, reports, templates, schedules, and download histories.

### Real-time / Event Behavior

This area does not itself broadcast real-time events; it is the persistence and protection substrate. However, the model captures the observable artifacts of real-time and audit behavior consumed and produced elsewhere:
- Append-only audit and event records (activity audit, message-recall audit, inbound-channel security events, cross-origin-access events, scan records, auto-reply audit logs, report execution/download histories) provide the durable evidence trail of actions and security decisions that other areas emit.
- The auto-reply delivery ledger's per-message uniqueness is the durable mechanism guaranteeing idempotent handling of webhook-triggered automated replies (the same inbound platform message is acted upon at most once).
- Channel-integration credential blobs are the only data class deliberately stored in a protected, tamper-evident, encrypted form; all other sensitive markers (passwords) are stored as non-reversible one-way hashes rather than encrypted (and thus are never recoverable as plaintext). Personal-data fields on customers (email, phone, free-form metadata) are stored as readable text in the current model, flagged conceptually as candidates for future protection.


---

# 8. Frontend Behavior

## Frontend State Model

### Purpose
This area defines the browser-side reactive state containers that the single-page application uses to hold, mutate, and synchronize all user-facing data. Each container manages a slice of application state (the signed-in operator session, conversation lists and their open conversation, messages, team membership, notifications, tags, automated-reply rules, channel QR codes, and local UI/system preferences) plus a single shared real-time channel manager. These containers implement the optimistic-update pattern (apply a change to local state immediately, call the server in the background, reconcile with the authoritative response on success, and revert to the prior snapshot on failure), local caching with time-based freshness, and real-time reconciliation from server-pushed events. They are purely client state: they call the documented HTTP endpoints and consume real-time events, but define no server endpoints themselves.

### Operations

#### Sign In — invokes POST authentication/login endpoint
- Invocation: triggered by the login UI.
- Inputs: a credentials object (the operator's login identifier and password) as required by the login endpoint.
- Preconditions & Authorization: none; this establishes the session.
- Behavior: a busy flag is raised and the prior error cleared; the login request is sent. If the server indicates a forced password change, the operation returns a result carrying a "must change password" flag, a short-lived temporary credential, and the operator profile, and does not establish a normal session. Otherwise the session credential, optional renewal credential, operator profile, and a computed session expiry (7 days ahead) are stored in memory and in browser persistence; multi-team data (the set of team identifiers the operator may access and the operator's role within each team) is derived from the session credential; the active team context is set to the operator's primary team; the session state becomes "authenticated".
- Success Output: boolean true on a fully established session, or the "must change password" result object described above.
- Side Effects: persists session credential, renewal credential, expiry, operator profile, and active team context to browser storage; sets the outgoing authorization header for subsequent requests.
- Error Conditions: an unsuccessful server result clears session state, sets a human-readable error message, sets session state to "unauthenticated", and returns false; a thrown/network error does the same with a generic network message.
- Invariants & Guarantees: the busy flag is always cleared at the end. Session expiry is a 7-day sliding window separate from the credential's own embedded expiry.

#### Sign Out — optionally invokes the logout endpoint
- Invocation: triggered by the user, or internally when the session is detected invalid.
- Inputs: an optional flag controlling whether the server logout endpoint is called (default: call it).
- Behavior: if requested and a session exists, the logout endpoint is called (failures are ignored). All active session fields are cleared; all persisted session keys are removed; additionally all locally cached conversation lists, conversation metadata, analytics comparison data, and generic cache entries are purged to prevent cross-operator data leakage; the authorization header is removed; session state becomes "unauthenticated".
- Side Effects: emits a browser-level "auth state changed" signal so the routing layer can invalidate its auth cache; navigates the browser to the login page unless already there.
- Invariants & Guarantees: clearing local caches on sign-out is mandatory to isolate operators sharing a device.

#### Restore Session On Load — no network call unless needed
- Invocation: runs once when the session container is first created (page load) and again via an explicit "initialize session" action.
- Behavior: on creation, persisted session data is read; the session is restored only if the stored expiry is still in the future AND the stored credential is structurally valid (three-part token, carries an operator identifier and role, and its embedded expiry, if present, is in the future). When valid, the credential, renewal credential, expiry, derived multi-team data, persisted active team context, and operator profile are restored. When invalid or expired, all persisted session data and the active authorization header is cleared. The "initialize session" action sets session state to "pending", then: if no credential, sets "unauthenticated"; if the session fails validation, signs out silently and sets "unauthenticated"; if a valid cached operator profile already exists, sets "authenticated" without any network call; otherwise it fetches the current operator profile and sets "authenticated" on success, signs out silently on an unauthorized response, or sets "unauthenticated" otherwise.
- Invariants & Guarantees: session restoration prefers cached data and avoids redundant network calls; validation functions are side-effect-free (callers decide whether to sign out).

#### Fetch Current Operator — invokes the "current operator" endpoint
- Invocation: explicit action.
- Inputs: an optional force-refresh flag.
- Behavior: returns immediately if no credential; if a valid cached profile exists and refresh is not forced, returns without a network call; otherwise fetches the profile, updates memory and persistence on success, and signs out silently on an unauthorized response.

#### Switch Active Team
- Invocation: explicit action.
- Inputs: a target team identifier.
- Preconditions & Authorization: administrators may switch to any team; non-administrators may switch only to teams in their allowed-team set.
- Behavior: on permission failure returns false without changing state; otherwise sets the active team context (in memory, in the shared request client, and in persistence) and returns true.

#### Session Validity & Token Helpers (local, no network)
- Validate session: returns whether a credential exists, is structurally valid, and is within the session expiry window; pure (no logout side effect).
- Extend session / auto-extend session: pushes the session expiry to a new 7-day window; auto-extend does so only when the remaining window is under one day.
- Token expired / token valid: report on the credential's own embedded expiry and structural validity.
- Should refresh token: true only when both a credential and renewal credential exist, the credential is not yet expired, and it is within 30 minutes of expiring.
- Proactive token refresh: when the above is true, performs a token renewal and, on success, extends the session expiry.
- Computed flags exposed: whether the operator is an administrator, whether the operator is a standard agent, and an "authenticated" flag (credential present, session valid, profile present).

#### Renew Session Credential — invokes the token-renewal flow
- Invocation: explicit action or in response to a credential-renewed signal.
- Behavior: requests a new credential; on success updates the active credential and renewal credential, re-derives multi-team data, and triggers a real-time channel reconnect with the new credential. Returns a success/failure result object.
- Real-time / Event Behavior: listens for a browser-level "credential renewed" signal and adopts the new credential and multi-team data when it differs from the current one.

#### List Conversations — invokes the conversation-list endpoint
- Invocation: explicit action; also reused by refresh, silent refresh, and pagination.
- Inputs: optional filter set (status, channel/platform, team identifier, free-text search, tag identifiers, customer name, updated-after/updated-before bounds); a page number (default 1); an append flag (default false).
- Preconditions & Authorization: requires an authenticated session and authorization header.
- Behavior: when filters are supplied they become both the working filters and the "active filters" reused by background paths. A busy flag is raised. Only non-empty filters are sent. On success: when appending, only conversations whose identifiers are not already present are added to the end; otherwise the list is reconciled in place (entries unchanged keep their existing object, changed entries are replaced, new entries added) and pagination metadata recorded.
- Success Output: the list and pagination metadata (page, page size, total, total pages) populate local state.
- Error Conditions: an unsuccessful or thrown result sets a transient error message (auto-cleared after 5 seconds).
- Invariants & Guarantees: all busy/refresh/update/load-more flags are cleared at the end. Incremental reconciliation preserves object identity for unchanged rows. A row is considered "changed" only when one of its display/sort-relevant fields differs.

#### Refresh Conversations / Silent Refresh / Load More / Refresh
- Refresh and silent refresh: re-fetch page 1 using the active filters (non-append). Silent refresh is intended for background use.
- Load more (two variants): fetch the next page in append mode; one variant is gated so it does nothing past the last page.
- Set active filters: records the filter set used by all background fetch paths.

#### Fetch One Conversation — invokes the conversation-detail endpoint
- Inputs: a conversation identifier.
- Behavior: if the conversation already exists locally it is shown immediately; then the detail is fetched and, on success, becomes the open conversation and is merged back into the list. Errors set a transient message.

#### Fetch Messages — invokes the message-list endpoint
- Inputs: a conversation identifier; an optional append flag.
- Behavior: loads the message list for the conversation (replacing or appending), and clears any pending optimistic messages. Errors set a transient message.

#### Send Message — invokes the message-send endpoint
- Invocation: explicit action. Present in two containers (a dedicated message container and the conversation container) with equivalent observable behavior.
- Inputs: a conversation identifier; message text; an optional channel/platform (default "line"). The message container also accepts an object form and validates/sanitizes content: empty or whitespace-only text is rejected; text over 10,000 characters is rejected; HTML is sanitized to a small safe formatting subset.
- Preconditions & Authorization: a conversation identifier is required; the conversation container additionally requires a signed-in operator.
- Behavior (optimistic): a placeholder message (temporary identifier, sender = current operator/agent, the given text, current timestamp, channel, type "text") is appended immediately and a sending flag raised. The send request is then issued.
  - On success: the placeholder is removed and the authoritative message appended; in the conversation container the open conversation's last-message preview is updated and merged back into the list.
  - On failure: the conversation container removes the placeholder and surfaces a transient error. The message container instead keeps the placeholder visible but marks it as failed (carrying the error text) so the user can see and retry.
- Success Output: the authoritative message object (message container) or boolean true (conversation container).
- Error Conditions: validation failures and send failures surface transient error messages; the operation returns false (or the failed placeholder remains, per container).
- Invariants & Guarantees: the sending flag is always cleared at the end.

#### Mark Message Read — invokes the message read endpoint
- Inputs: a message identifier.
- Behavior: finds the message, calls the endpoint, and on success flags it read locally. Errors surface a transient message and return false.

#### Edit Message — invokes the message-edit endpoint
- Inputs: a message identifier and updated text.
- Behavior: only text edits are supported; sends the edit and, on success, merges the authoritative result into local state and refreshes the search index entry. Failures surface a transient message.

#### Recall/Delete Message — invokes the message-recall endpoint
- Inputs: a message identifier.
- Behavior: sends the recall; on success removes the message from local state. Failures surface a transient message.

#### Mark Conversation Read — invokes the conversation read endpoint
- Inputs: a conversation identifier.
- Behavior: on success sets that conversation's unread count to zero in the list and, if open, on the open conversation. Failures surface a transient message.

#### Message Filtering & Search (local)
- Set filter / clear filters: maintain a filter set over conversation, sender role, channel, and message type; a derived filtered view applies them.
- Derived views: a combined-and-time-sorted list of stored plus optimistic messages; an unread subset; a per-conversation accessor.
- Search messages: queries a browser-local message index (built lazily during browser idle time and debounced). A cleanup hook stops the index watcher and clears the index.

#### Assign Conversation To Team — invokes the team-assign endpoint
- Invocation: explicit action; administrator-facing.
- Inputs: conversation identifier, target team identifier, optional team display label.
- Preconditions & Authorization: requires both identifiers; intended for administrators.
- Behavior (optimistic with rollback): a deep snapshot of the conversation is captured; the conversation is immediately updated locally (status "assigned", target team identifier, team summary) in both the list and the open conversation; the assign request is issued. On success the authoritative result (or a fallback detail fetch) replaces the optimistic value. On failure or exception the snapshot is restored in both places and a transient error surfaced.
- Success Output: boolean true/false.

#### Unassign Conversation — invokes the unassign endpoint
- Inputs: conversation identifier, optional reason. Same optimistic/rollback pattern, applying status "pending" and clearing team assignment, with a fallback detail fetch on missing data.

#### Transfer Conversation To Team — invokes the transfer endpoint
- Inputs: conversation identifier, optional source team identifier, target team identifier, optional target label, optional reason. Same optimistic/rollback pattern, applying the new team identifier and summary.

#### Deprecated Individual Assignment
- The single-operator assignment action is retired: it always surfaces an error ("use team assignment") and returns false without contacting the server.

#### Optimistic Conversation Update (generic) — optional background endpoint
- Inputs: conversation identifier, partial updates, optional background request function.
- Behavior: applies updates locally and to the local conversation cache immediately. If a background request is provided, on success it replaces the local entry only when the authoritative result differs; on error it rolls back to the captured snapshot (state and cache) and surfaces a transient message. Returns a success/rollback result object; returns a not-found result if the conversation is absent.

#### Cache-First Conversation Load — optionally invokes the list endpoint
- Inputs: filter set, page number.
- Behavior: serves a locally cached list immediately (scoped per operator identifier to prevent cross-operator contamination); if the cache is fresh, returns without a network call; otherwise performs a background refresh, reconciling incrementally when cache-backed or replacing when not, and re-writes the per-operator cache. Returns a descriptor indicating cache/freshness/count.

#### Preload Next Page / Preload Adjacent Conversation Messages
- Preload next page: prefetches the following page's list into a generic prefetch cache (no-op past the last page).
- Preload adjacent messages: prefetches the first short page of messages for the conversations immediately before and after the open one, scheduled during browser idle time. Failures are logged and ignored.

#### Conversation Statistics (local + optional endpoint)
- Update stats from conversations: recomputes totals from the current list — total count, count active, count in-progress (labeled "assigned"), count pending, and summed unread.
- Load stats: attempts a stats endpoint and, on failure or absence, falls back to the locally computed stats.

#### Initialize / Teardown Real-time For Conversations
- Initialize: idempotently subscribes the conversation container to the shared real-time "conversations" channel (connecting the shared channel first if needed), starts the pending-cleanup timer and the background-sync timer, and registers a page-visibility listener.
- Teardown: unsubscribes, stops both timers, removes the visibility listener, and clears transient error/update flags.

#### Background Sync & Pending Cleanup (timers)
- Background sync: every 30 seconds, when the page is visible, performs an HTTP poll of the conversation list; skipped while the page is hidden.
- Visibility handling: when the page becomes visible after being hidden, triggers an immediate poll.
- Reconnection sync: triggers an immediate poll after a real-time reconnect.
- Pending cleanup: every 30 seconds removes provisional ("pending") conversations older than 60 seconds and recomputes stats.
- Poll: fetches the list (respecting active status/channel/team filters), reconciles incrementally, records the last-update time, and recomputes stats; on error sets a transient sync-failure message.

#### Tags — Fetch Tags — invokes the tag-list endpoint
- Inputs: optional page, page size, team identifier, search text, include-global flag.
- Behavior: raises a busy flag, fetches the tag list into local state, returns the response, and re-throws on error (after recording the error message); always clears the busy flag. Provides a reset action.

#### Auto-Reply Rules / Schedules / Logs — invoke corresponding endpoints
- Fetch rules: loads the rule list and its pagination (loud, sets busy flag); re-throws on error.
- Fetch schedules: loads the schedule list.
- Fetch logs: loads the log list, pagination, and a today's-reply-count total.
- Silent fetch rules / silent fetch logs: identical data loads without the busy flag (for background stats polling); swallow errors silently.
- Upsert rule: inserts a created rule at the front (incrementing the total) or replaces an existing rule in place by identifier.
- Reset: clears all auto-reply state.

#### Toggle Auto-Reply Rule Active — invokes the rule-update endpoint
- Inputs: a rule identifier.
- Behavior (optimistic with double-click guard): if the rule is already mid-toggle, returns false immediately; otherwise flips the rule's active flag locally at once, marks it in-flight, and sends the update. On failure reverts the flag. Always clears the in-flight marker. A helper reports whether a given rule is currently toggling.

#### Notifications — Fetch List — invokes the notification-list endpoint
- Inputs: optional filters (type, priority, read-state), page (default 1), append flag (default false).
- Behavior: raises the appropriate busy flag (load vs. load-more), fetches, and either appends de-duplicated entries or replaces the list, recording pagination. Errors surface transient messages.
- Related reads: refresh (page 1), load-more (gated past the last page), fetch recent (a short most-recent list), fetch unread count, fetch stats (also updates unread count).

#### Notifications — Mark Read / Mark All Read / Delete (optimistic with rollback)
- Mark read: optimistically flags one notification read, stamps a read time, and decrements the unread count; reverts all three on failure.
- Mark all read: optionally scoped to one type; snapshots the list and unread count, optimistically flags matching notifications read, recomputes unread count; restores the snapshot on failure.
- Delete: optimistically removes the notification and decrements unread count if it was unread; reinserts at the original position and restores the count on failure.
- Each surfaces a transient error and returns false on failure.

#### Notifications — Create / Add (push) / Polling / Filters / Reset
- Create: invokes the create endpoint, then refreshes recent list and unread count on success; returns the new identifier or null.
- Add (real-time push): inserts a not-already-present notification at the front of both the full and recent lists (recent capped at 10) and increments unread count if unread.
- Polling: a 30-second timer (configurable) that immediately and then repeatedly refreshes unread count and recent list; start/stop actions manage it.
- Filter setters (type/priority/read-state) and clear-filters each re-fetch page 1.
- Reset: stops polling and clears all notification state.
- Derived views: unread subset, has-unread flag, grouped-by-type map, urgent-unread subset, can-load-more flag.

#### Teams — Load Members / Load Teams / Load All — invoke team endpoints
- Behavior: each raises a shared busy flag governed by a counter (so parallel loads do not prematurely clear it) and loads members or teams; failures record an error message. Load-all runs both in parallel.
- Derived stats: total member count, active team count, administrator count.

#### Teams — Add Member — invokes the add-member endpoint
- Inputs: login identifier, optional name/email, password, role (administrator or agent), optional group, active flag.
- Behavior: on success appends the returned member and returns it; on failure records an error and throws.

#### Teams — Update Member Role / Status / Remove (optimistic with rollback)
- Update role: applies the new role locally at once; on failure reverts to the prior role, records an error, and throws.
- Update status: applies active/inactive locally at once; on success, when set inactive, emits a browser-level "account disabled" signal (carrying the member identifier and display name) so other tabs can force-sign-out that operator; on failure reverts and throws.
- Remove: optimistically removes the member; on failure restores the prior list and throws.

#### Teams — Bulk Delete / Bulk Update (optimistic with rollback)
- Bulk delete: optimistically removes the given members, clears selection, and exits selection mode; on failure restores the prior list and throws.
- Bulk update: optimistically applies role and/or active-state changes to the given members, clears selection, exits selection mode; on failure restores the prior list and throws.

#### Teams — Reset Password / Reset Password With Policy / Update Member — invoke endpoints
- Reset password: triggers a server-side reset; records an error and throws on failure.
- Reset password with policy: sets a new password with a policy (changeable / unchangeable / must-change); error/throw on failure.
- Update member: sends partial updates and merges the authoritative result into the local member on success; error/throw on failure.

#### Teams — Selection State (local)
- Toggle selection mode (clears selection when leaving it), toggle one member, select-all (with optional exclusions), deselect-all. A derived selected-count is exposed. A reset action clears all team state.

#### Teams — Local-Only Updates & Real-time Handlers
- Local-only member/team update helpers apply field changes without any network call (used after a successful API call elsewhere) and stamp an updated time; return whether the target was found.
- Real-time handlers update a team's member count on member-added/removed events and update a team's name/description/active-state/member-count on a team-updated event.

#### QR Codes — Load / Generate / Stats / Prefetch — invoke team QR endpoints
- Load: serves a per-team cached QR code if still within a 5-minute freshness window (unless forced); otherwise sets that team's loading flag, fetches, and caches the result (or caches an explicit "no QR" entry). Errors record a message and clear loading.
- Generate: unless regenerating, first checks for an existing QR and returns it if present; otherwise marks the team as generating, requests generation, caches the new QR, and clears the generating marker. Errors record a message.
- Stats: fetches scan statistics for a team; returns null on failure.
- Prefetch: silent background load used on hover; skipped if cache is fresh or already loading.
- Cache management: invalidate one team, clear all, clear error, full reset.
- Derived accessors (per team): the cached QR (null if expired), loading flag, generating flag, cache-valid flag.

#### System Settings & Stats (mostly local)
- Load settings: reads persisted local preferences and merges them over defaults.
- Save settings: merges partial changes into local state and persists them; returns success/failure. (Server persistence is not currently performed.)
- Load stats: fetches dashboard statistics from the system endpoint into local state (total conversations, active agents, response time, satisfaction).
- Update theme: saves the theme and immediately applies/removes the dark style on the document, honoring an "auto" mode that follows the OS preference.
- Update language / notifications / auto-refresh / display: persist the respective partial settings; enabling desktop notifications requests browser notification permission.
- Reset settings: restores all defaults.
- Online-status and theme listeners: register browser listeners and return cleanup functions.
- Derived: dark-mode flag, current language, notifications-enabled flag, an online flag.

#### Shared Real-time Channel — Connect / Disconnect / Reconnect / Subscribe / Unsubscribe / Send
- Connect: refuses without a session credential; no-ops if already connected/connecting; creates the underlying client, wires message/connection/error/heartbeat handlers, and connects. On error sets an error state and schedules reconnection (up to 3 attempts, delay scaling 5s × attempt number).
- Disconnect: clears the reconnect timer, tears down the client, and sets the state to disconnected.
- Reconnect: disconnects, waits briefly, and reconnects.
- Subscribe: registers a handler on a named channel, returns a unique subscription identifier, and indexes it by channel.
- Unsubscribe: removes the subscription and prunes empty channels.
- Send: transmits a message only when connected; otherwise warns and drops it.
- Clear all subscriptions: empties all subscription state (test/teardown use).
- Derived: connection state, is-connected, is-connecting, subscription count, channel count, last error, reconnect attempts, latency, and counters (sent/received/reconnects/uptime). Incoming messages increment a received counter and are routed only to subscribers of the channels an event router maps them to; handler exceptions are caught and logged without disrupting other handlers.

#### Preload Cache (Teams) — invokes the team-list endpoint
- Init / warmup: idempotently preloads teams (deduplicated; concurrent calls share one in-flight promise) and marks initialized.
- Preload teams: returns cached teams if fresh; otherwise fetches (deduplicating concurrent fetches) with a 30-minute freshness window.
- Get teams (synchronous, stale-while-revalidate): returns fresh cache immediately; if stale, returns the stale data and refreshes in the background; if absent, triggers a background load and returns an empty list.
- Ensure-loaded (async): returns fresh cache or awaits a load.
- Refresh: clears the cache and reloads.
- Cleanup / clear-all / reset: remove expired or all cache and state.
- Derived: cache-valid flag, has-cache flag, cache statistics.

#### Shared Browser Query Behavior
- Invocation: browser state code requests data with a cache key, fetch operation, optional freshness time, cache lifetime, retry count, retry delay behavior, and revalidate-on-stale flag.
- Preconditions & Authorization: requires an active team context; throws if none.
- Behavior: keys all cache entries by the active team identifier for tenant isolation; serves fresh cache directly; serves stale cache while revalidating in the background when enabled; otherwise executes the fetch with retries (progressively delayed retry). On total failure it degrades to a longer-lived stale copy, and only throws if even that is absent. A companion method returns per-key hit/miss/error/revalidation/latency metrics.

### Data Concepts (neutral)
- Operator session: a session credential, an optional renewal credential, the operator profile (identifier, contact identifier, display label, system role of administrator or agent, primary team), a session expiry timestamp, a session lifecycle state, the set of teams the operator may access, the operator's role within each such team, and the currently active team context.
- Conversation: a unique identifier, the customer it belongs to (identifier, display name, channel, channel-side user identifier, optional avatar), channel/platform, status, optionally assigned team (identifier + summary), unread count, last-message preview, last-message time, first-response time, created/updated times. A provisional ("pending") variant carries extra channel-onboarding metadata (a channel-side user identifier, a pending flag, a scan timestamp) used to display a row before the server has the real conversation, later reconciled away.
- Message: identifier (real or a temporary client-side identifier for optimistic rows), parent conversation, sender identifier and sender role (customer or agent), text content, channel, message type, timestamps, delivery status, and free-form metadata including read and failure flags.
- Team and team member: a team (identifier, name, optional description, active flag, member count, timestamps) and a member (identifier, login identifier, display name, contact identifier, system role, active/inactive status, team memberships with per-team role and primary flag, updated time).
- Notification: identifier, type (one of a fixed set such as new message, assignment, transfer, mention, system, priority change, customer responded, task reminder, removed-from-team, customer followed, new conversation), priority (including an urgent level), read flag and read time, plus its display payload.
- Tag, auto-reply rule/schedule/log, and channel QR code: tags (collection with pagination); auto-reply rules (each with an active flag), schedules, and logs (with pagination and a today's-count total); QR codes per team (identifier, link, image link, scan count, active flag, timestamps) with scan statistics.
- Local preferences and statistics: a settings object (theme, language, notification toggles, auto-refresh, display density and toggles) persisted in browser storage; dashboard statistics held in memory.
- Caches: per-operator conversation list/detail caches, a per-team QR cache (5-minute freshness), a preload teams cache (30-minute freshness), and a tenant-scoped query cache (configurable freshness with a longer stale fallback). Soft-deletion is honored implicitly: removed conversations/members disappear from lists on the next authoritative reconciliation rather than via hard local deletion alone.

### State & Lifecycle
- Session lifecycle: pending → authenticated (on valid restore or successful sign-in) | unauthenticated (no credential, validation failure, or sign-out). A separate "restored" state exists in the state vocabulary. Forced-password-change is a side branch that does not reach "authenticated".
- Conversation status: observable values include active, in-progress, pending, and assigned. Team assignment moves a conversation to "assigned" with a team; unassignment moves it to "pending" and clears the team; transfer changes the team. Real-time unassignment events move a conversation to "active" and clear the team. Provisional conversations are terminal-by-timeout: removed automatically after 60 seconds if never reconciled, or replaced when the real conversation arrives.
- Member status: active ↔ inactive; setting inactive triggers a cross-tab force-sign-out signal for that operator.
- Real-time connection: disconnected → connecting → connected; on error → error then up to three scheduled reconnect attempts → reconnecting; explicit disconnect returns to disconnected. The conversation container's exposed sync status maps these to connected / connecting / error / disconnected.
- Auto-reply rule active flag and notification read flag are simple two-state toggles with optimistic flips and rollback.

### Real-time / Event Behavior
- Single shared channel: one real-time connection is maintained for the whole application; consumers subscribe to named channels and receive only events an event router maps to those channels.
- Consumed conversation events: new-message / message-sent / message-delivered update the matching conversation's last-message preview, timestamps, first-response time (for agent messages), and unread count (incremented only for customer-sent messages), then move it to the top; absent a conversation identifier or matching row, an HTTP poll is used as a fallback. Conversation-updated / status-changed / assigned events apply status and team changes. Unassigned events clear the team and set status active. Transferred events distinguish three sub-actions: "removed" (the conversation left the operator's team — removed from the list, cache invalidated, and if open, a "transferred away" banner state is set, with guards so members of the destination team or not in the source team ignore it), "assigned" (the conversation entered the operator's team — added to the top with reconciliation of any matching provisional row and de-duplication, and if open a "transferred in" banner state is set unless it was just transferred away), and "team-changed" (in-room team relabel). Batch-update, message-updated, and message-deleted events trigger an HTTP poll to resync. Customer-profile-updated events update cached customer name/avatar on affected conversations and invalidate the relevant cache.
- Consumed team events: member-added/removed update a team's member count; team-updated updates a team's fields.
- Emitted browser-level signals (for other parts of the app and other tabs): an "auth state changed" signal on sign-out; a "credential renewed" signal consumed to adopt a refreshed credential; an "account disabled" signal when a member is set inactive so other tabs force-sign-out that operator.
- Push-style notification arrival: a new notification is inserted at the front of the lists and increments the unread count if unread, with de-duplication by identifier.
- Heartbeat/latency: heartbeats update an uptime counter and a measured/estimated latency value.

## Frontend Views & User Flows

### Purpose
This area is the operator-facing single-page web application for a multi-channel customer support platform. It presents a set of authenticated screens (and one public sign-in screen) through which support agents and administrators sign in, view a metrics dashboard, browse and handle conversations across messaging channels, manage customers and labels, administer teams and channels, configure system settings, review activity and notifications, and edit their own profile. Navigation, screen access, and the visibility of administrative areas are governed by the signed-in user's authentication state and system role. The application maintains persistent layout chrome (a collapsible side navigation, a top bar with page title and a notification entry point) around a routed content region.

### Operations

#### Application Navigation Shell
- Invocation: Rendered around every authenticated screen.
- Inputs: None directly; reacts to the current route path and the signed-in user's role.
- Preconditions & Authorization: Requires an authenticated session. The shell exposes a base set of destinations to all authenticated users and an extended set only to administrators.
- Behavior: Displays a side navigation listing destinations, a top bar showing the human-readable title of the current screen, and a content region for the active screen. The currently active destination is visually highlighted by matching the current path. The side navigation can be collapsed/expanded; on narrow viewports it becomes a slide-out drawer toggled by a menu button and dismissed by tapping outside or an overlay. Some destinations are grouped with expandable sub-menus that auto-expand when the active path falls under their section.
  - Base destinations (all authenticated users): Dashboard, Conversation Management, Label/Tag Management, Reports (expandable: dashboard, templates, generate report), Data Management (expandable: export conversation records).
  - Administrator-only destinations (added to the above): Team Management, Channel Management, Auto-Reply, Activity Log, API Monitoring, System Settings (expandable: general, LINE integration, Facebook integration, advanced, health check).
- Success Output: The shell with active-destination highlighting and the routed screen content.
- Side Effects: Sidebar collapsed/expanded preference is remembered. Selecting a notification from the top bar routes the user to a relevant screen (see Notification Top-Bar Navigation).
- Error Conditions: None observable beyond routing failures, which are logged and do not crash the shell.
- Invariants & Guarantees: Administrative destinations never appear for non-administrators. The active-highlight always reflects the current path. Page title in the top bar is derived from a fixed mapping of path to title; conversation detail paths show a generic conversation title; unknown paths show a generic page title.

#### Route Guarding & Session Bootstrapping
- Invocation: Runs automatically before every in-app navigation.
- Inputs: Target route and its access metadata (whether it requires authentication, whether it is guest-only, page title).
- Preconditions & Authorization: Evaluates the current authentication state, waiting for session initialization to complete if it is still pending.
- Behavior, in order:
  1. Sets the browser document title from the route's declared title (or a default) immediately.
  2. If navigating to the same path, allows it without further checks.
  3. May serve a short-lived cached authentication decision to avoid re-checking on rapid navigations.
  4. Otherwise waits for session validation to finish, then: a guest-only screen (the sign-in screen) redirects an authenticated user to the dashboard; a screen requiring authentication redirects an unauthenticated user to the sign-in screen; other screens are allowed.
- Success Output: Navigation proceeds to the requested screen or to the redirect target.
- Side Effects: Document title updated. An authentication-state-change signal clears the cached decision (e.g., on sign-out).
- Error Conditions: If the guard throws, navigation is still allowed to continue (fail-open to avoid a blank screen) and the failure is recorded as an unauthenticated cached state.
- Invariants & Guarantees: Routes default to requiring authentication unless explicitly marked public. The root path redirects to the dashboard. Unknown paths render a "page not found" screen. Administrator-only screens are declared with an administrator requirement in their metadata in addition to authentication.

#### Sign-In Screen — /login
- Invocation: Public screen; reached when unauthenticated, or directly. Authenticated users are redirected away to the dashboard.
- Inputs: Email (required, must match a standard email pattern), Password (required, minimum eight characters, must contain at least one lowercase letter, one uppercase letter, and one digit). A light/dark appearance toggle.
- Preconditions & Authorization: No authentication required; guest-only.
- Behavior: Presents a branded card with email and password fields, an inline validation message per field, and a submit control that is disabled while the form is invalid or a submission is in progress. On submit the form is validated; if valid, credentials are sent for authentication. On success the user is routed to the dashboard. If the response indicates a forced password change, a modal is shown to set a new password (carrying a temporary credential and the agent's basic identity) and on completion the modal closes. On failure an error banner is shown. A repeated-failure lockout is enforced client-side: after enough failed attempts the account is locked for a period and a lock message is shown instead of attempting sign-in; a successful sign-in resets the failure counter. A disabled third-party (Google) sign-in button and links toward account creation and password recovery are present.
- Success Output: Navigation to the dashboard; or the forced-password-change modal.
- Side Effects: Appearance preference persisted locally. Failed-attempt count and lockout state persisted client-side. On success an authenticated session is established.
- Error Conditions: Invalid fields block submission and show per-field messages. Authentication failure shows an error banner and records a failed attempt. Lockout shows a lock message and blocks further attempts until it expires.
- Invariants & Guarantees: Submission is suppressed while one is already running. Form is revalidated on mount.

#### Forced Password Change (post-login)
- Invocation: Triggered when sign-in indicates the user must change their password before continuing.
- Inputs: A new password (subject to the system's password rules), provided through a modal dialog; uses a temporary credential issued by sign-in.
- Behavior: Blocks normal entry until a new password is set; on success the modal closes and the user proceeds.
- Side Effects: User's password is updated server-side.
- Invariants & Guarantees: The user cannot bypass this step to reach protected screens until completed.

#### Dashboard — /dashboard
- Invocation: Authenticated screen; default landing after sign-in.
- Inputs: None (auto-loads its data).
- Preconditions & Authorization: Authentication required.
- Behavior: Shows a brief skeleton placeholder on first load, then a welcome banner with the user's display name and current date plus a manual refresh control; a row of summary tiles (count of pending/unassigned conversations, count of in-progress/assigned conversations, today's message count, count of online agents); a recent-conversations card (selecting an entry navigates to that conversation); a live activity feed card showing recent important activity with a connection indicator and a manual reconnect control; a performance metrics card (response time, satisfaction rate, resolved-today count); and a deferred analytics/trend comparison section that loads after the primary content. Refresh re-fetches conversations and statistics together.
- Success Output: The populated dashboard.
- Side Effects: Starts a background session-token refresh check and user-activity tracking while the screen is mounted; stops both on leaving. Subscribes to a real-time activity stream for the feed.
- Real-time: The activity feed reflects live activity events and exposes connection state and a reconnect action.
- Invariants & Guarantees: Returning to the dashboard refreshes its data. Numeric tiles default to zero when data is unavailable.

#### Conversation List — /conversations
- Invocation: Authenticated screen.
- Inputs: A filter panel (including label/tag filters and a last-message text search) and a manual refresh control. Available labels are loaded for the filter dropdown.
- Preconditions & Authorization: Authentication required.
- Behavior: Displays the conversation collection as a table on wide viewports and as cards on narrow viewports. Shows a loading indicator when empty and loading, an empty-state message when there are no conversations, and an update/sync indicator during background refreshes. Most filters trigger a debounced server fetch; the last-message text search filters the already-loaded list locally. Selecting a conversation navigates to its detail screen. A subtle highlight animation indicates additions/removals in the list.
- Success Output: The filtered conversation list.
- Side Effects: On mount, loads with cache (shows cached data immediately while silently refreshing) and starts real-time synchronization managed centrally so all consumers receive live updates; cleans up the subscription on leaving.
- Error Conditions: A load failure is handled without blanking the screen.
- Real-time: List entries update live as conversations change.

#### Conversation Detail — /conversations/:id
- Invocation: Authenticated screen; reached by selecting a conversation.
- Inputs: Conversation identifier from the path. User interactions: send message, attach files (via picker or drag-and-drop, up to ten files and a per-file size cap), reply-to a message, copy a message, recall/withdraw a message (with confirmation), select quick-reply canned texts, search within messages, scroll to load older history, refresh, export the conversation, and navigate back.
- Preconditions & Authorization: Authentication required. Message sending is disabled when the conversation has been transferred away from the current user's team.
- Behavior: Shows a header (with back, refresh, in-message search toggle, and export actions), optional banners when the conversation was transferred out of or into the current team, a toggleable search panel, a virtualized message list with skeleton placeholders during initial load and a confirmed empty-state when there are no messages, a new-message notification chip (clicking it scrolls to newest), a message composer with quick replies, and (in a debug mode only) a connection-status bar. Sending, pending, upload-progress, confirmation, and failure of outgoing messages are reflected in the list; failed messages can be retried. Typing indicators are shown. Older messages load when scrolling up. Copying a message places its text on the clipboard and confirms via a toast. Recall asks for confirmation, then withdraws the message and reports success or failure. Forwarding shows an "in development" notice. Export opens a dialog scoped to the conversation.
- Success Output: The live conversation view; success/error toasts for actions.
- Side Effects: Subscribes to real-time updates for the conversation and the central conversation store (to receive transfer events); cleans up on leaving and clears transferred/received banner state. Sends messages and uploads attachments to the backend. Copy writes to the system clipboard.
- Error Conditions: Initialization failure shows an error toast prompting a page refresh. Refresh failure shows an error toast. Recall failure shows an error toast. Drag-and-drop validation errors (too many or too large files) surface as error messages.
- Real-time: Receives new messages, typing indicators, delivery/confirmation status, and conversation transfer events live.
- Invariants & Guarantees: When the conversation is transferred away from the team, the composer is replaced by an informational, non-editable notice. The message list is only revealed after the initial scroll-to-bottom completes, to avoid visible jumping. Empty-state display is delayed briefly to avoid flicker during sync.

#### Notification Top-Bar Navigation
- Invocation: Selecting a notification from the top bar's notification control.
- Inputs: The selected notification's type and associated data.
- Behavior: Routes to a relevant screen based on type: message/customer-response/assignment/transfer/priority-change/mention/task-reminder notifications that carry a conversation reference open that conversation; system notifications and any unrecognized type open the notification center.
- Side Effects: In-app navigation only.

#### Notification Center — /notifications
- Invocation: Authenticated screen.
- Inputs: Filters (by type, by priority, by read/unread status); per-notification actions (open, mark read, delete); a mark-all-read action; a settings entry; a load-more control; keyboard navigation across the list.
- Preconditions & Authorization: Authentication required.
- Behavior: Shows a header with mark-all-read and settings entry, a stats overview, a filter panel, and the notification list with per-item read/delete actions and animated insert/remove. Shows a loading state while initially loading, and an empty state (offering to clear filters) when none match. Supports paginated loading of more notifications. Clicking a notification triggers the same type-based routing as the top bar.
- Success Output: The filtered, paginated notification list.
- Side Effects: Marking read/all-read and deleting update server-side notification state; an unread indicator drives the mark-all-read availability.
- Real-time: New notifications appear live.

#### Label/Tag Management — /customers/tags
- Invocation: Authenticated screen.
- Inputs: Create, edit, delete a label; bulk-select and bulk-delete labels; text search/filter; view a label's statistics; view conversations carrying a label; choose a label color from a predefined palette.
- Preconditions & Authorization: Authentication required.
- Behavior: Shows a header with a create action, a stats overview (including total label count), a toolbar with search and bulk-selection controls, and a grid of labels. Provides a create/edit modal (with color picker), a single-delete confirmation modal, a bulk-delete confirmation modal, and a label-conversations modal. Uses optimistic updates with caching and supports keyboard shortcuts and bulk operations.
- Success Output: The label grid reflecting changes.
- Side Effects: Label create/edit/delete and bulk-delete persist server-side; conversation-count figures are shown per label.
- Invariants & Guarantees: Bulk operations act on the current selection; selection can be cleared.

#### Team Management — /team
- Invocation: Administrator-only authenticated screen.
- Inputs: Member operations: add member (with email-availability pre-check and a duplicate/reactivation flow), change member role, toggle member active status, reset member password, remove member; bulk member selection with bulk-edit and bulk-delete; sorting (by field, order, and custom ordering). Team operations: add team (selecting members), edit team (managing current members), toggle team status, remove team; team sorting.
- Preconditions & Authorization: Requires authentication and administrator role; non-administrators reaching it are redirected to the dashboard.
- Behavior: Shows a header with refresh, a stats overview, a member-management section (list, sorting, selection mode, role controls, status toggles, password reset, removal, bulk actions), and a team-management section (list, sorting, add/edit/remove, status toggle). Provides modals for adding a member (with password visibility toggle and email-blur availability check), duplicate-member detection (with reactivate option), adding a team (with member selection and select-all), and editing a team. The current user is identified so self-targeting bulk actions are handled appropriately.
- Success Output: Updated member/team lists.
- Side Effects: Member and team create/update/delete/status-change operations persist server-side; password reset issues new credentials.
- Error Conditions: Duplicate email surfaces the duplicate-member modal offering reactivation.

#### Channel Management — /channels
- Invocation: Administrator-only authenticated screen.
- Inputs: Add a channel; filter channels by platform (all, LINE, Facebook, WhatsApp); refresh.
- Preconditions & Authorization: Requires authentication and administrator role.
- Behavior: Shows a header with an add-channel action, a platform filter bar with per-platform counts and a total count, a loading state, an empty state (prompting to add the first channel), and a grid of channel cards. Adding a channel opens a dialog.
- Success Output: The filtered channel grid.
- Side Effects: Channel configuration persists server-side.

#### Auto-Reply Management — /auto-reply
- Invocation: Administrator-only authenticated screen.
- Inputs: Tab selection (rules, business-hours schedules, execution logs); rule create/edit/search/filter-by-trigger-type; refresh.
- Preconditions & Authorization: Requires authentication and administrator role.
- Behavior: Shows a header with refresh, a stats overview (active rule count, today's reply count, current business-hours status, success rate), and a tabbed area: a rules list with inline create/edit, a business-hours schedule editor, and a paginated execution-log view. Tab badges show counts of rules and of logs.
- Success Output: The active tab's content reflecting current data.
- Side Effects: Rule create/edit and schedule changes persist server-side.

#### Activity Log — /activities
- Invocation: Administrator-only authenticated screen.
- Inputs: Filters (type/category and others), a date-range selector with custom range, a user filter; apply/clear filters; export records; refresh; pagination; per-activity restore action.
- Preconditions & Authorization: Requires authentication and administrator role. A statistics overview is shown only to administrators.
- Behavior: Shows a header with export and refresh actions, an admin stats overview, a filter pill bar, and a timeline of activities; supports a loading state, an empty state, and pagination. Some activities can be restored (reversed) from the timeline. Export downloads the activity records. An error toast surfaces failures.
- Side Effects: Export produces a downloadable file; restore reverses the corresponding action server-side; refresh re-fetches.

#### System Settings Shell — /settings (with nested pages)
- Invocation: Administrator-only authenticated screen.
- Inputs: Nested page selection via the sidebar sub-menu; per-page settings forms; refresh.
- Preconditions & Authorization: Requires authentication and administrator role for the shell and every nested page.
- Behavior: Shows a settings header with a refresh action and a status message area, and a content region that renders the active nested page with a fade transition. The nested pages are: general settings, LINE integration, Facebook integration, advanced settings, and a system health check page. Visiting the settings root redirects to general; visiting the integrations root redirects to the LINE integration page; visiting the maintenance root redirects to the health page. A single settings controller is shared across all nested pages.
- Success Output: The selected settings page with its current values.
- Side Effects: Settings load on entry and clean up on leaving; saving a page persists configuration server-side and reflects a status message.

#### Reports Section — /reports (with nested pages)
- Invocation: Authenticated screen (container is a pass-through layout; navigation handled by the sidebar sub-menu).
- Inputs: Nested page selection: report dashboard, report templates, report generator, and a report viewer for a specific report identifier. The export sub-path redirects to the data-export screen.
- Preconditions & Authorization: Authentication required.
- Behavior: Visiting the reports root redirects to the report dashboard. The container simply renders the selected nested report page.
- Success Output: The selected report page.

#### Data Management / Export — /data → /data/export
- Invocation: Authenticated screen.
- Inputs: Export configuration provided on the export page.
- Preconditions & Authorization: Authentication required.
- Behavior: Visiting the data root redirects to the export page, which lets the user export conversation records.
- Side Effects: Produces a downloadable export.

#### Profile / Personal Settings — /profile
- Invocation: Authenticated screen (self-service).
- Inputs: Display name (required, length-limited) editable by the user; email, role, and team are shown read-only (managed by administrators). Password change form: current password (required), new password (required, with a live strength meter), confirm new password (required, must match).
- Preconditions & Authorization: Authentication required; users may edit only their own display name and password.
- Behavior: A two-card layout. The basic-info card lets the user change their display name with reset and save controls that are enabled only when the form has changed and is not saving; save shows a saving state. The security card lets the user change their password with inline per-field validation and a strength indicator. Changing the password signs the user out and requires re-authentication.
- Success Output: Updated profile; a forced sign-out after a successful password change.
- Side Effects: Display-name and password changes persist server-side; password change invalidates the current session.
- Error Conditions: Per-field validation messages for invalid display name, missing current password, weak new password, or mismatched confirmation.

#### Monitoring & Admin Utility Screens
- Invocation: Administrator-only authenticated screens (except where noted): API Monitoring (/monitoring/api), WebSocket migration administration (/admin/websocket), and a component test page (/test/components). A WebSocket performance monitoring screen (/monitoring/websocket) requires authentication but not administrator role.
- Behavior: These present operational dashboards/utilities for system observability and administration. They follow the same shell, authentication, and (where applicable) administrator-role gating as other admin screens.

#### Not-Found Screen
- Invocation: Any unmatched path.
- Inputs: None.
- Behavior: Renders a standalone error screen with a "page not found" message and two actions: return to the dashboard, and go back (returns to the previous history entry if one exists, otherwise navigates to the dashboard).
- Authorization: Rendered without an authentication requirement on the route itself.

### Data Concepts (neutral)
- Signed-in user / agent: carries a unique identifier, a display name, an email, a system role (administrator or agent), and team association(s). Email, role, and team are administrator-managed; the user may self-edit only display name and password.
- Conversation summary: a unique identifier, an associated customer (with a display name), the latest message preview/content, assignment state (unassigned/pending vs. assigned/in-progress), label associations, and a transfer state relative to the viewer's team (transferred out, transferred in, or neither).
- Message: a unique identifier, textual content, a sender side, a delivery/confirmation lifecycle (pending, sent/confirmed, failed, withdrawn/recalled), optional attachments, and a timestamp.
- Label/tag: a unique identifier, a human-readable name, a color, and an associated conversation count.
- Team: a unique identifier, a name, an active/inactive status, and a membership set.
- Channel: a unique identifier, a platform designation (LINE, Facebook, WhatsApp, etc.), and configuration; counted per platform.
- Notification: a unique identifier, a type, a priority, a read/unread state, an optional associated conversation reference, and content.
- Activity record: an actor, an action type/category, a timestamp, and (for some) a reversible/restorable flag.
- Auto-reply rule / schedule / execution log: rules with trigger types and active status, business-hours schedules, and a paginated log of executions with success/failure outcomes.
- Soft-delete behavior: removed members can be detected as duplicates and reactivated rather than recreated, indicating that member records persist after removal and can be restored.

### State & Lifecycle
- Session state: pending (initializing) → authenticated or unauthenticated. Guards wait for resolution before deciding access. Sign-out clears the session and the guard's cached decision.
- Sign-in lockout: unlocked → locked (after repeated failures, for a timed period) → unlocked (on expiry or successful sign-in).
- Forced password change: after sign-in flags it, the user is in a blocked state until a new password is set, after which normal access resumes.
- Outgoing message lifecycle (observable in the conversation view): pending (optimistic) → sent/confirmed, or → failed (retryable). A confirmed message may later be withdrawn/recalled (terminal display change).
- Conversation transfer state relative to viewer's team: when transferred out, the composer is replaced by a non-editable notice and an out-transfer banner appears; when transferred in, an informational in-transfer banner appears (dismissible) while the conversation remains usable.
- Profile password change: success forces session termination, returning the user to the unauthenticated state and the sign-in screen.

### Real-time / Event Behavior
- Conversation list and conversation detail subscribe to live updates so that new conversations, conversation changes, and conversation transfer events appear without manual refresh; a central store manages the connection and broadcasts to all subscribers.
- The conversation detail screen receives live new messages, typing indicators, and delivery/confirmation status, and shows a new-message chip when the user is scrolled away from the newest message.
- The dashboard subscribes to a live activity stream feeding its activity feed, exposing connection state, reconnect attempts, latency, and a manual reconnect action.
- The notification center and the top-bar notification control reflect live notification arrivals; selecting a notification routes the user based on its type (conversation-related types open the referenced conversation; system and unrecognized types open the notification center).
- While the dashboard is active, background processes maintain session-token freshness and track user activity; these stop when leaving the dashboard.

## Frontend Real-time Client & Sync

### Purpose
This area is the browser-side layer that keeps the support agent's (and the customer's) view of conversations and messages live. It establishes and maintains authenticated real-time socket connections to the server, validates and refreshes the session credential before/around each connection attempt, automatically reconnects with backoff on transient failures, classifies and routes incoming real-time notifications to interested UI areas, and reconciles local state against the server (both via socket notifications and a periodic safety-net fetch). It also offloads heavy list processing and full-text message search to background threads so the UI stays responsive.

### Operations

#### Establish agent real-time connection — outbound WebSocket to `/api/websocket/connect`
- Invocation: Called by the UI when an authenticated agent opens a real-time-backed view, or automatically on construction when auto-connect is enabled.
- Inputs (carried as query parameters on the connection URL):
  - session credential token (required; pulled from the auth session, not from the caller)
  - optional conversation context identifier (string)
  - optional device label (defaults to a generic web label)
  - optional client version label (defaults provided)
  - Configuration knobs (all optional, with defaults): whether to auto-reconnect (default on), base reconnect delay (default 1s), max reconnect attempts (default 10), heartbeat send interval (default 30s), heartbeat timeout (default 35s), outbound queue max size (default 100), whether to auto-connect on construction (default off).
- Preconditions & Authorization:
  - A session credential token must be present, or the attempt fails before any socket is opened.
  - The token must be well-formed as a three-part dotted credential whose middle part decodes to an object containing at least a user identifier and a role, and whose expiry (if present) is in the future.
  - A short pre-connection probe is made to a server health endpoint (`GET /api/websocket/health`, authenticated with the token, ~5s timeout). If that endpoint responds and explicitly reports the real-time service as disabled, the connection is aborted; if the probe itself fails or times out, the connection still proceeds (probe is advisory only).
- Behavior (observable order):
  1. Run the credential checks and the optional health probe.
  2. If the token will expire within a very short window (about 30s or less), a refresh is attempted first and the connection is aborted if the refresh fails. If the token expires within a larger window (about 2 minutes), a refresh is attempted opportunistically but the connection proceeds with the existing token even if the refresh fails.
  3. Build the connection URL and open the socket; transition through connecting → connected.
  4. On successful open, reset the reconnect counter, clear the last-error state, begin heartbeats, and flush any queued outbound messages in the order they were queued.
- Success Output: An observable connection-state value transitions to "connected" and a connected flag becomes true. The first server-originated acknowledgement notification is treated as a connection acknowledgement and consumed silently.
- Side Effects: Begins a periodic heartbeat send; begins a heartbeat-timeout watchdog; on (re)connection, re-announces membership for any conversations the client had previously joined.
- Error Conditions:
  - Missing token → connection aborted, state becomes "error", a descriptive error is surfaced.
  - Malformed token (wrong number of parts, undecodable payload, missing user/role fields) → aborted with a descriptive error.
  - Expired token → aborted (with refresh attempt as above).
  - Server reports real-time disabled via the probe → aborted with a "service disabled" error.
  - Socket constructor throws → state becomes "error" and a reconnect cycle is scheduled.
- Invariants & Guarantees:
  - Calling connect while already connecting or connected is a no-op.
  - Outbound messages sent while not connected are queued (FIFO) up to the queue cap; when the cap is exceeded the oldest queued message is dropped to make room.

#### Send a real-time message / control frame (client behavior)
- Invocation: Used by higher-level client logic (join/leave a conversation, send chat content, typing start/stop, presence update, heartbeat ping, subscribe-to-list).
- Inputs: A message object with a string type and optional payload, conversation identifier, user identifier, and timestamp. The client stamps each outbound frame with a generated message id and a timestamp.
- Preconditions: Socket open. If not open, the frame is queued instead of sent.
- Behavior: Serializes and transmits the frame; on a transmit error the frame is re-queued.
- Observable result: the frame is transmitted immediately, or queued if the socket is not open.
- Observable outbound frame types include: conversation join, conversation leave, send-message, typing-start, typing-stop, presence-update, heartbeat ping, and subscribe-to-conversation-list.
- Invariants: On reconnection, previously-joined conversations are re-joined automatically; queued frames are flushed on open.

#### Receive & dispatch a real-time notification — inbound socket frame handler
- Invocation: Triggered by any inbound socket frame.
- Inputs: A JSON frame with a string type and optional payload/conversation-id/user-id/timestamp fields.
- Behavior:
  - Heartbeat frames update the last-heartbeat timestamp and fire a heartbeat callback; acknowledgement frames are consumed silently; all other frames are surfaced to the registered message handler.
  - Unparseable frames are ignored (logged only).
  - Higher-level managers then branch on the frame type to update local state (see Real-time / Event Behavior).
- Side Effects: Updates a "last received message" observable; increments a received-message counter; may update typing indicators, presence maps, conversation activity counters, team member counts, and conversation/message lists depending on type.
- Invariants: Frame type matching is intended to be case-insensitive on the customer-facing client path (incoming types are normalized to lowercase before comparison).

#### Heartbeat & liveness watchdog (client behavior)
- Invocation: Started automatically on successful connection.
- Behavior: Periodically (default every 30s) sends a heartbeat ping while connected. A separate watchdog (runs at half the heartbeat-timeout cadence) checks elapsed time since the last inbound heartbeat; if it exceeds the configured timeout (default 35s), the connection is treated as lost and a reconnect cycle is initiated.
- Side Effects: Reconnect attempt on timeout.
- Invariants: Heartbeat timers are stopped on disconnect/cleanup; restarting heartbeat first clears any existing timers.

#### Automatic reconnection with backoff (client behavior)
- Invocation: Triggered by abnormal socket closure, certain close codes, heartbeat timeout, or socket construction failure.
- Behavior (close-code-driven outcomes):
  - Normal closure → state becomes "closed", no reconnect.
  - "Endpoint going away" or other unexpected codes → state "error", standard reconnect cycle.
  - Abnormal closure → state "error", reconnect after a short fixed extra delay (~2s).
  - Policy-violation closure and a family of custom authentication-failure close codes (missing token, invalid token format, invalid/expired token, token expired, token expiring soon, invalid user data, invalid role) → treated as authentication failures: a token refresh is attempted; on success the reconnect counter is reset and a fresh connection is retried shortly after; on failure the client stops retrying and surfaces a "please re-login" error.
  - Server-side authentication error code → state "error", reconnect after a longer fixed delay (~5s).
  - Standard reconnect: increments the attempt counter, transitions to "reconnecting", waits an exponentially increasing delay (base delay doubled per attempt, capped at 30s), and before reconnecting may refresh the token if it is near expiry (aborting the cycle if no valid token can be obtained).
- Invariants & Guarantees:
  - Reconnection stops once the max-attempts ceiling is reached or auto-reconnect is disabled; state then settles on "error".
  - Authentication-driven failures reset the attempt counter so they do not consume the network-failure budget.
  - Disconnect/cleanup cancels pending reconnect timers, stops heartbeats, and closes the socket with a normal-closure code.

#### Customer-facing real-time connection — outbound WebSocket to `/api/customer-ws`
- Invocation: Created when a customer-side conversation view opens.
- Inputs (as query parameters): conversation identifier (required) and a session credential. Optional config: auto-reconnect (default on), max reconnect attempts (default 5), base reconnect interval (default 1s).
- Behavior:
  - Opens the socket, transitions connecting → connected, resets the reconnect counter on open.
  - Normalizes the incoming frame's type to lowercase, then handles: wrapper "event" frames (e.g. connection-established and sync-response on reconnect) passed straight through to the consumer; new-message frames (increments a message counter and forwards the normalized frame; duplicate suppression is delegated downstream by message identity rather than by sender); and user-connected / user-disconnected presence frames (forwarded).
  - On non-normal close with auto-reconnect enabled and attempts remaining, reconnects after an exponential delay (base interval doubled per attempt, capped at 10s); when attempts are exhausted, settles on "error".
- Side Effects: Maintains observable connection state, a connected flag, and a message counter; supports an explicit reconnect that resets the attempt counter.
- Outbound use: primarily control frames such as a sync request (to fetch messages missed during a disconnect) and heartbeat/typing frames; ordinary chat content is sent over HTTP rather than this socket.
- Error Conditions: Parse errors and socket errors are surfaced via an error callback and move the state to "error".

#### Conversation list sync (real-time + safety-net polling) — hybrid sync service
- Invocation: Started by the conversation-list view; can also be manually refreshed and reacts to page-visibility changes (refreshes data when the tab becomes visible again).
- Behavior:
  - On start, opens a real-time connection dedicated to list updates and also starts a low-frequency safety-net fetch loop.
  - On connection acknowledgement, sends a subscribe-to-conversation-list frame.
  - When it receives any list-affecting notification (list-changed, conversation-changed, new-message, message-changed), it records the time and triggers an authoritative fetch of the latest list (page 1, page size 50) — i.e. notifications are treated as "something changed, go refetch" signals rather than as the data itself. The fetched list is handed to the consumer callback.
  - The safety-net fetch only actually fetches when the real-time channel has been silent longer than the configured interval (default 5 minutes) or the service is in fallback "polling" mode, and only on conversation-related routes.
  - If real-time fails repeatedly (up to a small attempt cap, default 3, with increasing delay), it falls back to "polling" mode and surfaces a user-facing fallback message; a later successful fetch while in polling mode will attempt to restore the real-time connection.
- Status values exposed: disconnected, connecting, connected, polling, error, plus a last-update timestamp and an error message.
- Side Effects: Periodic authoritative list fetches; observable status and last-update fields; a page-visibility listener that is cleaned up on stop.
- Invariants: Starting the service first tears down any existing connection/timers; stopping clears all timers and listeners.

### Data Concepts (neutral)
- Connection state: one of disconnected, connecting, connected, reconnecting, error, or closed (the closed state distinguishes a deliberate normal close from an error close).
- Outbound message queue entry: a pending frame with a generated id, a retry counter, and a creation timestamp; the queue is bounded and drops oldest-first on overflow.
- Conversation membership record (per joined conversation): the conversation identifier, an active flag, a last-activity timestamp, a running received-message count, and the set of user identifiers currently typing in that conversation. Held only for the lifetime of the connection.
- Presence record (per user): a user identifier, an online flag, a last-seen timestamp, and an optional "currently viewing" conversation identifier. Held for the current browser session.
- Channel name: a logical subscription target derived from a notification, in conceptual forms such as the global conversation-list channel, a per-conversation channel, a per-conversation messages channel, a notifications channel, an activity channel, a tags channel, a presence channel, and a per-user presence channel.
- Normalized conversation summary (produced by the background processor for the list view): a conversation identifier, the associated end-user identity (id, display name, originating platform, platform-specific user id, created timestamp), the assigned team id/team, a status (defaulting to an "open"-equivalent), platform, last-message timestamp and last-message preview, an unread count, and created/updated timestamps. Names are normalized and missing fields are defaulted. (These are display-shaped fields; no server storage names are implied.)
- List diff: the computed set of added, updated, removed, and moved items between the previous and next list, keyed by item identity, used to drive smooth list transitions. Updates that arrive mid-animation are queued and applied sequentially.
- Local search index: a browser-local full-text index over messages keyed by message identity, weighting message body highest, then sender name, then attachment names, then message kind; supports plain, fuzzy, field-scoped, boolean, and wildcard queries, with graceful fallback to a plain query if an advanced query fails. Rebuilt wholesale on add/update/remove of a single message.

### State & Lifecycle
- Agent connection lifecycle: disconnected → connecting → connected → (on loss) reconnecting → connected, or → error (terminal until a new connect is requested) / closed (deliberate normal close). Authentication-class failures can jump straight to error with a re-login requirement, bypassing further retries.
- Customer connection lifecycle: disconnected → connecting → connected → (on non-normal close) reconnecting → connected, or → error when retries are exhausted.
- List sync lifecycle: disconnected → connecting → connected; degrades to polling on repeated real-time failure; polling can recover back toward connected after a successful fetch; error is surfaced on fetch failure.
- Reconnect budgets are terminal once exhausted (network-class failures) but are reset by successful authentication refreshes.

### Real-time / Event Behavior
The client classifies inbound notification types and either consumes them or fans them out to interested UI areas (channels). System/control types — heartbeat, ping, acknowledgement, subscribe/unsubscribe acknowledgements, and error — are not fanned out.

Application notification types and their observable handling / target channels:
- Conversation-list-changed → refreshes the list channel.
- Conversation-changed → list channel plus the specific conversation's channel.
- New-message → list channel, the conversation's channel, and the conversation's messages channel; increments activity/message counters for joined conversations and forwards the message to the message callback.
- Message-changed and message-read → the conversation's channel and messages channel.
- Conversation-closed, conversation-assigned, conversation-unassigned, conversation-transferred (covering removal-from-old-team, addition-to-new-team, and team-changed sub-cases), and conversation-status-changed → list channel plus the specific conversation's channel.
- Notification → notifications channel.
- Activity → activity channel.
- User-presence → presence channel plus the per-user presence channel; updates the browser-local presence map and the derived online-users list.
- Typing → the conversation's channel; updates the per-conversation typing-user set; emits typing-start / typing-stop callbacks.
- Customer-tags-changed → tags channel (drives auto-refresh of tag-management views).
- Team-member-added, team-member-removed, team-changed → forwarded with their payloads (team id, team name, member count, acting user, affected agent) to drive live member-count updates.
- Unknown application types → no routing (logged as unhandled).

Connection-state transitions themselves are broadcast to subscribers: on entering "connected" the client re-joins previously joined conversations; on entering "disconnected" or "error" it clears transient state (typing indicators are emptied and all tracked users are marked offline), with these clears deferred to the next reactivity tick to avoid update cascades.

## Frontend Routing, Guards, API Layer & i18n

### Purpose
This area governs how the browser single-page application decides which screens a visitor may view, how it talks to the backend over HTTP (authentication, automatic token renewal, retry, multi-team scoping, file transfer), and how it presents its user interface in multiple languages. It is the client-side enforcement and wire-contract layer: route protection and redirects are advisory/UX-level (the backend remains the real authority), while the HTTP client encodes the exact request/response contracts the backend must honor.

### Operations

#### Navigate to a protected screen — client-side route change
- Invocation: Triggered by any in-app navigation or initial page load to a path the SPA recognizes.
- Inputs: The target path; each known path carries metadata flags — "requires authentication" (default true unless explicitly disabled), "guest only" (login-style pages), "requires administrator", an optional human-readable page title, and an optional layout hint.
- Preconditions & Authorization: Evaluated against the current session state held in the browser (a stored access credential plus a cached identity record and a session lifecycle state of "pending", "authenticated", or "unauthenticated").
- Behavior (observable order):
  1. The browser tab title is updated immediately to the screen's title (suffixed with the product name) or to the product name alone if no title is set.
  2. If the target path equals the current path, navigation is allowed without further checks.
  3. A short-lived browser-local authentication snapshot (validity window of a few seconds) may be consulted: if fresh and the route is public, navigation is allowed instantly; if fresh and the user was recently authenticated and the route requires auth, navigation is allowed instantly.
  4. Otherwise the combined guard runs: if the session lifecycle is still "pending", it waits for session initialization to finish, then re-reads the finalized authenticated/not-authenticated state.
  5. Guest-only pages: an authenticated visitor is redirected to the main dashboard; an unauthenticated visitor is allowed.
  6. Auth-required pages: an unauthenticated visitor is redirected to the login screen; an authenticated visitor is allowed.
  7. All other pages are allowed.
  8. If the guard throws unexpectedly, navigation is allowed anyway (fail-open at the UX layer) and the snapshot records a not-authenticated state.
- Success Output: The target screen renders, or a redirect to login / dashboard occurs.
- Side Effects: Updates the document title; refreshes the browser-local auth snapshot; may trigger one-time session initialization (which can call the "current identity" endpoint).
- Error Conditions: Guard failure does not block navigation. A path the SPA does not recognize renders a dedicated "page not found" screen.
- Invariants & Guarantees: Route protection is purely client-side and best-effort; it prevents accidental viewing but is not a security boundary. The auth snapshot is invalidated immediately whenever a global "authentication state changed" signal fires (e.g., on logout). Same-path navigation is short-circuited to avoid redirect loops.

#### Known navigable destinations and their access tiers
- Root path redirects to the dashboard.
- Login screen: public, guest-only, blank layout.
- Dashboard, personal profile, conversation list, single-conversation detail (by conversation identifier), real-time-channel performance monitoring, tag management, notification center, data-export area (defaults to an export sub-screen), and the reporting area (sub-screens: reporting dashboard, report templates, report generation, and a report viewer by report identifier; an export sub-path redirects into the data-export area) — all require authentication only.
- Team management, channel management, activity log, system settings (sub-screens: general, platform-integration screens for each messaging platform, advanced, and a maintenance/health screen; bare paths redirect to sensible defaults), a real-time-channel administration/migration screen, an internal component test screen, and the automatic-reply configuration screen — all require authentication AND administrator role.
- Any unmatched path: a "not found" screen (no auth requirement).
- Note: administrator-required routes are flagged in metadata, but the combined guard only enforces authentication and guest-only rules; finer-grained administrator gating is applied by individual screens and by the backend, not by the central navigation guard.

#### Authenticated HTTP request (shared client) — METHOD /api/<path>
- Invocation: Every backend call issued by the SPA passes through one shared HTTP client whose base address is "/api" during local development (proxied) and a fully-qualified backend address in production.
- Inputs: HTTP verb, relative endpoint, optional JSON body, and per-call options: a retry counter, an "is-retry" flag, and a "redirect to login on unauthorized" flag (default enabled).
- Preconditions & Authorization: If an access credential is held, it is attached as a bearer authorization header. If a team-context identifier is held, it is attached as a dedicated team-context request header. Content type defaults to JSON.
- Behavior (observable order):
  1. The request is sent with the assembled headers.
  2. The response body is parsed as JSON; if parsing fails, a generic "server response format error" message is substituted.
  3. On success, the parsed envelope is returned to the caller unchanged.
  4. On an unauthorized response (status indicating expired/invalid credentials) when redirect-on-unauthorized is enabled, not already a retry, and a renewal credential is held: the client attempts a one-time credential renewal and, if successful, transparently re-issues the original request once with the new credential.
  5. On an unauthorized response when no renewal credential is available (and redirect enabled): the client performs a guarded redirect to the login screen.
  6. On a server-side failure status, the client retries up to a fixed maximum (a few attempts) using increasing back-off delays.
  7. On a network/transport failure, the client retries up to the same maximum with back-off, then returns a "network connection error" envelope with a zero status.
- Success Output: A response envelope carrying a success flag and, on success, a data payload (and optionally a message and pagination block).
- Error Conditions: Failure envelopes carry a success flag of false, a human-readable message (server-provided message preferred, otherwise a status-derived localized default), and the numeric status. Status-to-message mapping covers bad request, unauthorized, forbidden, not found, too-many-requests, and several server-error families.
- Invariants & Guarantees: Only one credential-renewal operation runs at a time; concurrent calls that hit unauthorized are queued and resolved together once renewal completes. Only one redirect to login can occur even when many calls fail simultaneously (guarded by an in-progress flag that resets on full page reload). Retries apply to server and network errors only, never to client-side (4xx) errors. Successful responses do not themselves extend the session at this layer.

#### Credential renewal — POST /api/auth/refresh
- Invocation: Triggered automatically by the HTTP client when an authenticated call is rejected as unauthorized.
- Inputs: The held renewal credential, sent in the request body.
- Behavior: On success the new access credential (and possibly a new renewal credential) are adopted, persisted to browser storage, and a global "credential renewed" signal is broadcast so other parts of the app can re-sync. Any queued requests waiting on renewal are released with the new credential. On failure, queued requests are rejected and a guarded redirect to login occurs.
- Side Effects: Updates stored credentials; emits a "credential renewed" event; may reconnect the real-time channel elsewhere in the app.
- Invariants: Idempotent in effect — concurrent renewal requests collapse into a single in-flight operation.

#### File download (shared client) — GET /api/<path>
- Invocation: Caller requests a binary resource (e.g., a generated report or attachment).
- Inputs: Endpoint; optional is-retry flag.
- Behavior: Sends an authenticated GET. On unauthorized, attempts one-time credential renewal and re-tries; if still unauthorized, performs guarded redirect to login. On other failures, raises an error carrying a server-provided or status-derived message.
- Success Output: A binary payload plus a derived file name (parsed from the response's content-disposition header, supporting both standard and UTF-8 encoded forms; defaults to a generic name) and a content type (defaulting to a generic binary type).

#### File upload (shared client) — POST /api/<path> (multipart)
- Invocation: Caller uploads one or more files.
- Inputs: A multipart form payload; optional is-retry flag.
- Behavior: Sends the form without forcing a content type (so the browser sets multipart boundaries). Attaches bearer and team-context headers. Applies the same one-time unauthorized renewal/retry and guarded-redirect behavior as JSON requests, but does not apply the multi-attempt server/network retry loop.
- Error Conditions: Returns a failure envelope on non-OK responses; returns a "file upload network error" envelope on transport failure.

#### Team-context selection (client behavior)
- Invocation: Called when the user switches the active team scope.
- Inputs: A team identifier or a clear-context signal.
- Behavior: Stores the active team identifier in browser storage and attaches it to all subsequent requests as a dedicated team-context header; clearing it removes both. The value is restored from storage on app start.
- Invariants: Affects authorization scope of subsequent backend calls; observable as different result sets per active team.

#### Sign in — POST /api/auth/login
- Inputs: Email and password.
- Behavior: On success, adopts the returned access credential (and optional renewal credential), caches the returned identity record, computes and stores a session expiry, derives multi-team membership data from the credential, sets the initial active team to the identity's primary team, and marks the session "authenticated". If the backend signals a mandatory password change, the client instead returns a "must change password" outcome carrying a short-lived temporary credential and the identity record, without establishing a normal session.
- Success Output: An envelope containing an access credential, an optional renewal credential, and the identity record.
- Error Conditions: A failed sign-in clears any partial auth state, records an error message, and marks the session "unauthenticated".

#### Fetch current identity — GET /api/auth/me
- Behavior: Returns the authenticated identity record. Used during session initialization when no valid cached identity exists. An unauthorized result triggers a silent local logout.

#### Sign out — POST /api/auth/logout
- Behavior: Attempts to notify the backend (best-effort; failure is tolerated), then clears all local auth state and credentials, clears cached conversation/analytics data to prevent cross-user leakage, marks the session "unauthenticated", broadcasts the "authentication state changed" signal, and navigates to login unless already there.

#### Change password — POST /api/auth/change-password
- Inputs: A new password and, optionally, the current password; optionally a short-lived temporary credential (used for mandatory first-login password change).
- Behavior: When a temporary credential is supplied it is used for this single call and the prior credential is restored afterward. This call explicitly disables the auto-redirect-on-unauthorized behavior so a rejected change does not eject the user to login.

#### Session initialization (client startup sequence)
- Behavior (observable order): Marks the session "pending"; if no stored credential exists, marks "unauthenticated"; if a stored credential exists but the session is invalid/expired, performs a silent local logout and marks "unauthenticated"; if a valid cached identity exists, marks "authenticated" without any network call; otherwise calls the current-identity endpoint and, on success, caches the identity and marks "authenticated", on unauthorized performs silent logout, on other failure marks "unauthenticated".
- Invariants: Navigation guards await this before deciding access, preventing a race where the guard would let a holder of an expired credential through.

#### Conversation operations (mirror of backend)
- List conversations (filter form) — GET /api/conversations with optional status, platform, and team filters; returns an adapted conversation list. Raw backend records are normalized client-side (legacy "open"/"closed"/"resolved" statuses map to an active state; nested vs. flat customer/team shapes are reconciled; an unknown customer falls back to a placeholder name and a default messaging platform).
- List conversations (paginated form) — GET /api/conversations with page, page-size, status (active / assigned / pending), platform, team, free-text search, tag-id list, customer-name, and updated-before/after filters; returns items plus pagination metadata, computing pagination locally if the backend returns a bare array.
- Conversation statistics — GET /api/conversations/stats (totals plus active/assigned/pending and unread counts).
- Get one conversation — GET /api/conversations/{id}; rejects an empty identifier locally with an error.
- Get conversation messages — GET /api/conversations/{id}/messages with optional page, page-size, and "since" cursor.
- Send a message — POST /api/conversations/{id}/messages with content, message type (text/image/file, defaulting appropriately), platform, optional sender, reply-target, metadata, and attachment-id list; rejects locally if both content and attachments are empty.
- Send a quick reply — convenience over send, tagging metadata as a quick reply.
- Upload a conversation attachment — POST /api/conversations/{id}/attachments (multipart) with a file and a message type; returns a stored-file reference (url, file name, attachment identifier).
- Mark conversation read — PUT /api/conversations/{id}/read.
- Mark message(s) read — PUT /api/conversations/{id}/messages/read or .../{messageId}/read.
- Recall a message — DELETE /api/conversations/{id}/messages/{messageId} with an optional reason.
- Get / edit a single message — GET and PUT /api/conversations/{id}/messages/{messageId} (edit rejects empty new content locally).
- Search messages in a conversation — GET /api/conversations/{id}/messages/search with a query and optional message-type.
- Assign a conversation to a team — POST /api/conversations/{id}/assign with a team identifier (required) and optional reason; individual-agent assignment is removed and a deprecated alias returns a local rejection.
- Unassign a conversation — POST /api/conversations/{id}/unassign with optional reason.
- Transfer a conversation between teams — POST /api/conversations/{id}/transfer with target team (required), optional source team, and reason; treats success-without-body as valid because the updated state arrives via the real-time channel.
- Set conversation tags (replace) — PUT /api/conversations/{id}/tags.
- Get conversation tags — GET /api/conversations/{id}/tags.
- Add / remove conversation tags — POST and DELETE (with body) /api/conversations/{id}/tags by tag-id list; reject empty inputs locally.
- Bulk conversation operation — POST /api/conversations/bulk with an operation (assign / close / reopen / set priority / add tags / remove tags), a conversation-id list, and operation data; rejects locally if the list is empty or exceeds one hundred entries. Convenience wrappers exist for bulk close, reopen, assign, add-tags, and remove-tags.

#### Messaging and delayed-message operations
- Standard messaging endpoints duplicate the conversation message send/list/search contracts under the same conversation paths.
- Delayed message (queue-based) — POST /api/messages/delayed; recall — POST /api/messages/recall; pending list — GET /api/messages/pending (paged); recall eligibility — GET /api/messages/{id}/can-recall; message details — GET /api/messages/{id}.
- Bulk create / delete messages — POST /api/messages/bulk-create and /api/messages/bulk-delete; both reject locally above a one-hundred-item limit.
- Delayed message (real-time-cancel variant, separate modern client) — POST /api/delayed-messages-v2/send; DELETE /api/delayed-messages-v2/cancel/{id}; GET /api/delayed-messages-v2/status/{id}; GET /api/delayed-messages-v2/pending; GET /api/delayed-messages-v2/health. These convey a scheduled-send time and a cancel-until deadline and support querying remaining time and cancellability.

#### File-management operations
- Upload single / multiple — POST /api/files/upload and /api/files/upload-multiple (multipart).
- List files — GET /api/files (page, page-size, optional platform).
- File details — GET /api/files/{id}.
- Time-limited download link — GET /api/files/{id}/download-url with an expiry-seconds parameter.
- Delete one / many — DELETE /api/files/{id}; POST /api/files/delete-multiple.
- File statistics — GET /api/files/stats over a period.
- Files by conversation / by message — GET /api/conversations/{id}/files and /api/messages/{id}/files.
- Direct-upload signed link — POST /api/files/presigned-url with name, type, and size; confirm completion — POST /api/files/{id}/confirm with size; signed-link service status — GET /api/files/presigned-url/status.
- Search files — GET /api/files/search with a query plus optional platform/type/date-range and pagination.

#### Tag operations
- List available tags — GET /api/customers/tags/available with optional page, page-size, team, search, and include-global flags; throws to caller if the backend reports failure.
- Create / get / update / delete a tag — POST /api/tags, GET and PUT /api/tags/{id}, DELETE /api/tags/{id} (delete is a soft delete).
- Tag usage statistics — GET /api/tags/{id}/stats.
- Bulk tag operation — POST /api/tags/bulk (activate / deactivate / update color).
- Customer tag membership — GET /api/customers/{id}/tags; add — POST; remove — DELETE (with body); set/replace — PUT.
- Tag's customers / conversations — GET /api/tags/{id}/customers and /api/tags/{id}/conversations (each paged).

#### Team and membership operations
- List members — GET /api/teams/members; member detail — GET /api/teams/members/{id}.
- Email existence check — GET /api/teams/members/check-email (returns existence and active/deleted status).
- Add member — POST /api/teams/members (front-end maps to backend fields; falls back login-id to email/display-name when absent; optional team assignment).
- Remove member — DELETE /api/teams/members/{id} (permanent hard delete; no restore).
- Bulk delete / bulk update / batch-edit members — POST /api/teams/members/bulk-delete, /bulk-update, /batch-edit (each capped at fifty per call); batch-edit may return an undo token; undo — POST /api/teams/members/batch-edit/undo.
- Update member role / status — PUT /api/teams/members/{id}/role and /status (role is two-tier: administrator or agent).
- Reset password — POST /api/teams/members/{id}/reset-password; reset with policy — POST /api/teams/members/{id}/reset; retrieve member password — GET /api/teams/members/{id}/password.
- Update member profile — PUT /api/teams/members/{id}.
- Team statistics — GET /api/teams/stats; password migration utility — POST /api/teams/migrate-passwords.
- Teams CRUD — GET /api/teams (optional include-inactive), POST /api/teams (creation also yields a channel QR code and channel link), PUT /api/teams/{id}, DELETE /api/teams/{id}, GET /api/teams/{id}, GET /api/teams/{id}/members, GET /api/teams/{id}/stats.
- Team scan-code (LIFF) — POST and GET /api/teams/{id}/qr-code/liff, statistics GET /api/teams/{id}/qr-code/liff/stats.
- Assignee discovery — GET /api/teams/assignees (filtered client-side to active assignable members).
- Multi-team membership — GET /api/teams/agent-teams/{agentId}; join — POST .../join; join-multiple — POST .../join-multiple; leave — DELETE .../leave/{teamId}; update team role — PUT .../role/{teamId}; set primary team — PUT .../primary/{teamId}; team members with membership detail — GET .../team/{teamId}/members.
- Batch add to a team — POST /api/teams/{teamId}/members/batch and /members/bulk-remove (each capped at fifty).
- Invariants: Many team functions perform local input validation (non-empty identifiers, list-size caps) and return localized failure envelopes without calling the backend; transport errors are converted to localized "network error" envelopes.

#### Channel-integration operations
- List channels — GET /api/channels (optional platform filter); create — POST /api/channels (returns the new integration plus its webhook address); get — GET /api/channels/{id}; update — PUT /api/channels/{id}; deactivate (soft delete) — DELETE /api/channels/{id}; verify configuration — POST /api/channels/{id}/verify; statistics — GET /api/channels/{id}/stats; health — GET /api/channels/{id}/health.
- Data note: sensitive credentials are never returned to the client; configuration, webhook configuration, and statistics arrive as serialized text blocks that the client parses defensively (empty/default on parse failure).

#### System, credentials, and feedback operations
- System info / settings (get and update) — GET /api/system/info, GET and PUT /api/system/settings.
- Integration test — POST /api/system/integrations/{platform}/test; metrics — GET /api/system/metrics; logs — GET /api/system/logs (level/date/limit filters); config export/import — GET /api/system/config/export and POST /api/system/config/import; stats — GET /api/system/stats (optional period); dashboard stats — GET /api/system/stats; events — GET /api/system/events; webhook URL update / test — PUT /api/system/webhooks/{platform} and POST .../test; maintenance mode set/get — POST and GET /api/system/maintenance.
- Full health check — GET /api/health/system, called via a raw authenticated fetch (bypassing the shared client) specifically so a degraded/unavailable status code still surfaces its body rather than being retried and discarded.
- Credentials — store POST /api/credentials, get one GET /api/credentials/{platform}/{type}, get all GET /api/credentials, clear platform DELETE /api/credentials/{platform}.
- Feedback — submit POST /api/feedback, stats GET /api/feedback/stats (optional time range), by conversation GET /api/feedback/conversation/{id}, list GET /api/feedback (paged).

#### Notification operations
- List — GET /api/notifications (page/size, type, priority, read-state, date-range); get one — GET /api/notifications/{id}; create — POST /api/notifications (rejects empty title/content/type locally); bulk create — POST /api/notifications/bulk; mark read — PUT /api/notifications/{id}/read; mark all read — PUT /api/notifications/mark-all-read (optional type); delete — DELETE /api/notifications/{id}; stats — GET /api/notifications/stats; unread count — GET /api/notifications/unread-count; recent — GET /api/notifications/recent; cleanup expired — DELETE /api/notifications/cleanup; channel stats — GET /api/notifications/channels/stats; channel test — POST /api/notifications/channels/{type}/test; convenience creators — POST /api/notifications/new-message, /conversation-assigned, /system; settings get/update — GET and PUT /api/notifications/settings.
- Notification types include new-message, conversation-assigned, conversation-transferred, mention, system, priority-changed, customer-responded, task-reminder, agent-removed-from-team, customer-followed, and new-conversation. Priorities: low, normal, high, urgent.

#### Reporting operations
- Generate — POST /api/reports; list — GET /api/reports; details / status — GET /api/reports/{id}; download — GET /api/reports/{id}/download (binary; also used to read JSON report content for preview); delete — DELETE /api/reports/{id}; statistics — GET /api/reports/stats (time-range); batch operation — POST /api/reports/batch; templates by type — GET /api/reports/templates/{type}; preview — POST /api/reports/preview; module info/health — GET /api/reports/info and /api/reports/health.
- Scheduled reports — create POST /api/reports/scheduled, list GET, update PUT /api/reports/scheduled/{id}, delete DELETE /api/reports/scheduled/{id}.
- Report lifecycle states: pending, generating, completed, failed, expired. Available output formats include a structured-data format and a spreadsheet format (a printable-document format is generated entirely client-side from the structured data). Report types include conversation summary, agent performance, and message statistics.

#### Automatic-reply operations
- Rules — list GET /api/auto-reply/rules (team/page/scope), create POST /api/auto-reply/rules (optional scope), update PUT /api/auto-reply/rules/{id}, delete (soft) DELETE /api/auto-reply/rules/{id}.
- Schedules — list GET /api/auto-reply/schedules, bulk upsert POST /api/auto-reply/schedules.
- Logs — GET /api/auto-reply/logs (team/page/rule/platform/date filters), including a same-day total.
- Concepts: a rule has a trigger type (welcome, keyword, off-hours, fallback), a priority, an active flag, a per-rule opt-in for falling back to a quota-consuming push delivery when the reply delivery fails, plus conditions (exact / contains / regex / message-type, with case-sensitivity and any/all match mode) and ordered actions (reply text / image / rich message).

#### Activity-log operations
- List — GET /api/activities (filters validated and sanitized client-side: page bounded to a max, page-size bounded, identifiers/action/resource-type pattern-checked, dates parse-checked).
- Per-user stats — GET /api/activities/users/{id}/stats (days clamped 1–365); overview — GET /api/activities/overview (days clamped, administrator-only); cleanup — DELETE /api/activities/cleanup (retention clamped to a safe minimum, administrator-only).
- Export — GET /api/activities/export via raw authenticated fetch returning a spreadsheet blob; on failure returns deliberately generic, sanitized error text (e.g., access-denied / not-found / failed) to avoid information disclosure.
- Restore a reversible activity — POST /api/activities/{id}/restore with a force flag via raw authenticated fetch; rejects non-positive identifiers locally; returns a structured outcome including status, an optional machine-readable code, restore-linkage identifiers, an optional retry-after delay, and a possible mid-change set; transport failure yields a zero-status network-error outcome. Activities expose a flag distinguishing reversible from irreversible operations.

#### Data export operations
- Export messages — GET /api/messages/export via raw authenticated fetch returning a blob (printable-document requests fetch structured data and render the document client-side). Inputs are validated/sanitized client-side: format whitelist, identifier patterns, date parse-checks, numeric customer id, and a record limit capped at a maximum.
- Export record count — GET /api/messages/export/count for a pre-export size estimate.
- Export filter option lists — customer options (sourced from the customer listing endpoint) and agent options (GET /api/messages/export/agents).

#### Resilient request handling for the delayed-message-cancel feature (client behavior)
- Invocation: Used by the real-time-cancel delayed-message feature.
- Behavior: Provides explicit request timeout/abort (returning a timeout outcome), increasing-back-off retries on network/timeout, one-time unauthorized credential renewal, and normalization of standard and paginated response envelopes. On renewal failure it clears local credentials and storage but does not itself force a redirect.

#### Language switching — client UI action
- Invocation: User selects a language.
- Inputs: A locale code from the supported set (a traditional-Chinese, a simplified-Chinese, and an English option).
- Behavior: Rejects unavailable locales; on success updates the active locale reactively, persists the chosen locale to browser storage, and updates the document's language attribute. Date/time/relative-time formatting helpers format according to the active locale.
- Success Output: Returns a boolean indicating whether the switch succeeded.
- Invariants: Default locale is traditional Chinese. On startup the locale is resolved in order: a previously persisted choice, then an inference from the browser language (Chinese regional variants distinguishing traditional from simplified, English mapping to English), otherwise the default. Missing translations fall back to the default locale silently.

### Data Concepts (neutral)
- **Browser-held session state**: an access credential, an optional renewal credential, a cached identity record (with a role of administrator or agent and a primary team), a computed session-expiry timestamp, an active team-context identifier, and a session lifecycle state (pending / authenticated / unauthenticated). Persisted in browser storage; cleared (along with cached conversation and analytics data) on sign-out.
- **Response envelope**: a success flag plus, on success, a data payload and optional message and pagination metadata; on failure, a human-readable message and a numeric status.
- **Conversation (client view)**: identifier, customer summary (name with placeholder fallback, messaging platform with default, platform user reference, optional avatar), assigned team summary, lifecycle status, last-message summary, unread count, and timestamps. Legacy statuses collapse into an active state.
- **Channel integration (client view)**: identifier, owning team, platform, active/verified flags, error tracking, timestamps, and serialized configuration/webhook/statistics text — never the secret credentials.
- **Tag, notification, report, automatic-reply rule, activity entry, file, team, and membership** concepts each carry the fields described in their operation blocks; soft-deletion applies to tags, automatic-reply rules, and channels (deactivation), while team-member removal is a permanent hard delete with no restore.
- **Localization bundles**: three language packs keyed by locale code, plus a default locale and an available-locale list with display names.

### State & Lifecycle
- **Session lifecycle**: pending → authenticated (valid credential and identity) or pending → unauthenticated (no/expired credential or failed identity fetch). Sign-in transitions to authenticated (or yields a separate "must change password" outcome that does not establish a session). Sign-out and any unauthorized identity check transition to unauthenticated and trigger redirect to login.
- **Navigation gate**: the guard awaits the pending→resolved transition before deciding; guest-only screens reject authenticated users (redirect to dashboard); auth-required screens reject unauthenticated users (redirect to login).
- **Report lifecycle**: pending → generating → completed / failed, with a terminal expired state.

### Real-time / Event Behavior
- This layer emits two browser-global signals consumed by other parts of the app: a "credential renewed" signal (after a successful automatic credential renewal, carrying the new credentials) and an "authentication state changed" signal (on sign-out), the latter invalidating the navigation guard's browser-local auth snapshot.
- Several operations explicitly rely on the separate real-time channel for state propagation rather than the HTTP response: conversation transfer treats a success-without-body as valid because the updated conversation is expected to arrive over the real-time channel; credential renewal triggers a real-time-channel reconnection elsewhere.


## 8.5 Frontend-to-Backend Traceability Matrix

### Purpose

This matrix is a requirement review artifact: each row links a user-visible requirement to the corresponding frontend behavior and backend endpoint contracts already defined in this document.

### 需求對照（前端需求 → 後端 API）

| 項目 | 前端需求（8.x / UI 行為） | 後端 API（2/3/6.x + 5.x） |
|---|---|---|
| 用戶的角色和權限 | 8.4 路由/權限中介層與 8.2/8.4 的管理頁權限宣告 | 1.3 權限模型（`exact/minimum role`、`team access`、`team operation`）；`/api/auth/me` 回傳角色與團隊 |
| 用戶的增刪改查 | 8.2 個人設定頁；8.2 團隊管理中的成員維護流程 | `/api/auth/profile`, `/api/auth/me`（本人）；`/api/teams/members` 相關作業（新增/更新/刪除） |
| 團隊的權限 | 8.2 團隊管理頁權限 gate；8.4 team context 與 team scoped 呼叫 | 1.3 權限規則（含 team-operation、team-role）；`/api/teams/*` 團隊資源路由 |
| 團隊的增刪改查 | 8.2 Team Management 畫面（建立/編輯/刪除團隊） | `GET /api/teams`, `POST /api/teams`, `PUT /api/teams/{id}`, `DELETE /api/teams/{id}`, `GET /api/teams/{id}` |
| 對話功能 | 8.2 Conversation List/Detail；8.1 對話列出、查詢、訊息傳送、回覆、附件上傳、已讀 | 2.1 `GET /api/conversations`, `GET /api/conversations/{id}`, `GET /api/conversations/{id}/messages`, `POST /api/conversations/{id}/messages`, `POST /api/conversations/{id}/attachments`, `PUT /api/conversations/{id}/read` |
| Websocket 維持連線功能 | 8.3 建立/心跳/重連；WebSocket 連線生命週期 | 5.1 `GET /api/websocket/connect`（agent）/`/api/customer-ws`（customer） |
| 即時更新功能 | 8.3 事件接收/分發、conversation list sync | 5.2 conversation room/fan-out 事件與重連同步、5.1 連線路由 |
| 標籤管理功能 | 8.2 `/customers/tags`（標籤 CRUD 與管理） | 2.6 `GET /api/tags`, `POST /api/tags`, `PUT /api/tags/{id}`, `DELETE /api/tags/{id}`, `POST /api/tags/bulk`, `GET /api/tags/{id}/stats` |
| 標籤的增刪改查 | 8.2 標籤管理 UI（新增/編輯/刪除/批次） | 2.6 同上 |
| 給對話打標籤 | 8.1/8.2 對話標籤操作 | 2.1 與 2.6：`GET /api/conversations/{id}/tags`, `POST /api/conversations/{id}/tags`, `DELETE /api/conversations/{id}/tags`, `PUT /api/conversations/{id}/tags` |
| 自動回复功能 | 8.2 Auto-Reply 管理頁（規則/排程） | 2.5 `GET/POST /api/auto-reply/rules`, `PUT/DELETE /api/auto-reply/rules/{id}`, `GET/POST /api/auto-reply/schedules` |
| 自動回覆管理與規格 | 8.2 Auto-Reply 規則、排程、執行日誌頁 | 2.5 規格完整性；`GET /api/auto-reply/logs`；健康檢查 |
| 對話指派功能（指派、取消指派、轉指派） | 8.1 對話作業：指派/取消指派/轉指派 | 2.1 `POST /api/conversations/{id}/assign`, `POST /api/conversations/{id}/unassign`, `POST /api/conversations/{id}/transfer` |
| 資料管理（匯出對話記錄） | 8.2 `/data/export`、匯出流程與檔案產製 | 2.2 `GET /api/messages/export`, `GET /api/messages/export/count`, `/api/messages/export/customers`, `/api/messages/export/agents` |
| 活動記錄功能（Activities log） | 8.2 `/activities` 時間軸、篩選、匯出、還原 | 3.5 `GET /api/activities`, `GET /api/activities/{id}`, `POST /api/activities/{id}/restore`, `GET /api/activities/export` |
| API 健康度監控 | 8.2 / 8.3 健康監控入口與狀態展示 | 6.3 `GET /api/monitoring/health`, `GET /api/monitoring/metrics`, `GET /api/system/health`, `GET /api/health/system` |

### 核對結論

- 以上 16 項需求在文件中皆有可追溯到位的前後端行為定義，可直接用作需求核對清單。
- 實作前建議以本表逐列建立測試案例：前端行為可驗收、後端 endpoint 是否可打通、即時事件是否可覆核，避免兩側只完成一邊。


---

# 9. Web Installer

## Web Installer — Backend / Provisioning

### Purpose
This area is a self-service provisioning system that lets an operator stand up a complete, isolated tenant of the multi-channel support product on the operator's own cloud account. From a single configuration submission it sequentially creates all required cloud resources (a relational database, two key-value stores, a file/file store, a message queue, a backend service, a frontend site, an optional custom domain) and bootstraps an initial administrator login. It exposes the cloud provider's credentials/identity-verification flows, accepts and validates intake configuration, runs the provisioning pipeline asynchronously, reports granular progress, and automatically tears down any partially-created resources if provisioning fails. It is designed to be safely re-run for the same tenant name (resource reuse instead of duplication).

### Operations

#### Service descriptor — GET /
- Invocation: unauthenticated client request.
- Inputs: none.
- Behavior: returns a static descriptor of the service.
- Success Output (200): object with service name, version string, a human description, a fixed status string, a map of available endpoint paths grouped by area (health, auth, oauth, deployment), a documentation URL, and a support contact.

#### Liveness check — GET /health
- Invocation: unauthenticated client request.
- Inputs: none.
- Behavior: always returns immediately.
- Success Output (200): object with a fixed "ok" status, a service identifier, a version string, and a current epoch-millisecond timestamp.

#### Begin cloud authorization (interactive grant) — GET /oauth/authorize
- Invocation: unauthenticated client request, used to start an interactive consent flow with the cloud provider.
- Inputs: optional query parameter giving the post-consent return URL (defaults to a localhost callback URL when omitted).
- Preconditions: the service must be configured with a client identifier for the provider; otherwise the call fails.
- Behavior: generates a random anti-forgery value and a random verifier, derives a challenge from the verifier, and assembles a provider consent URL that requests a fixed scope set covering read access to the account plus write access to: backend services, the database product, the key-value product, the file-store product, the queue product, and the static-hosting product. Note (observable): the generated state/verifier are not persisted server-side in the observed system; the verifier is returned to the caller for it to echo back later.
- Success Output (200): object containing the fully-built consent URL, the anti-forgery value, and the verifier.
- Error Conditions: missing client identifier configuration -> 500 with error "OAuth not configured"; any other failure -> 500 with error "Failed to initiate OAuth".

#### Complete cloud authorization (exchange grant) — POST /oauth/callback
- Invocation: unauthenticated client request after the user returns from the provider consent screen.
- Inputs (JSON body): an authorization grant code (required); optionally the anti-forgery value, the verifier, and the return URL used during the authorize step.
- Behavior: exchanges the grant code with the provider for an access credential (sending the verifier when present), then fetches the authorizing user's identity and the list of accounts that credential can act on.
- Success Output (200): object with a success flag, the obtained access credential, its lifetime in seconds, the authorizing user's identifier and email, and the list of accessible accounts (each with an identifier and display name).
- Error Conditions: missing grant code -> 400 with error "Authorization code required"; failed credential exchange -> 500 with an error message that includes the provider's failure text; failure to fetch identity -> 500 with error "Failed to get user info"; any other failure -> 500 with the thrown message.

#### Verify a long-lived access credential — POST /auth/token
- Invocation: unauthenticated client request; alternative to the interactive grant flow. Lets a client supply a pre-issued cloud access credential plus a target account identifier and confirm both are usable.
- Inputs (JSON body): an access credential (required) and a target account identifier (required).
- Behavior: validates the credential against the provider's credential-verification endpoint, then attempts to read the target account.
- Success Output (200): object with a success flag, the confirmed account identifier, and the account display name.
- Error Conditions: either input missing -> 400 with error "API Token and Account ID are required"; credential rejected by provider -> 401 with error "Invalid API Token"; credential not in an active state -> 401 with error "API Token is not active"; account not readable with this credential -> 403 with error stating the credential needs account-read permission; account lookup returns a non-success body -> 500 with error "Failed to retrieve account information"; any other failure -> 500 with the thrown message.

#### Start a provisioning run — POST /deployment/start
- Invocation: authenticated client request (the request itself carries the cloud access credential and account identifier obtained from one of the auth flows).
- Inputs (JSON body):
  - tenant/project name — required string; must be 3–50 characters, lowercase letters, digits, and hyphens only.
  - administrator email — required string; must match a basic email shape.
  - account identifier — required string; must be exactly 32 characters.
  - cloud access credential — required string.
  - administrator password — optional string; if omitted, a random one is generated during provisioning. If supplied it must be at least 8 characters (enforced during the admin-creation step, not at intake).
  - custom domain — optional string; if present must match a hostname pattern.
  - (Configuration model also carries optional fields for explicit backend URL, frontend URL, public file-store URL, messaging-integration credentials/identifiers, and a log-level selector; these influence generated URLs and the deployed configuration but are not all validated at intake.)
- Preconditions & Authorization: caller must possess a valid cloud access credential with the write scopes for all resource families; ownership is implicitly scoped to the named tenant. Each tenant name maps to a single dedicated orchestration instance, so concurrent starts for the same name share one run.
- Behavior (observable, ordered): (1) intake validation runs first and rejects bad input before any work; (2) a new provisioning run is created with a freshly generated unique run identifier and an initial "pending" state; (3) the run is launched asynchronously and the call returns immediately without waiting for completion; (4) the tenant is registered/updated in a global directory of provisioning runs.
- Success Output (200): object with a success flag, the generated run identifier, and a message "Deployment started".
- Side Effects: persists initial run state for the tenant; adds or refreshes an entry in the global run directory carrying tenant name, run identifier, administrator email, account identifier, optional custom domain, and creation/update timestamps; begins creating real cloud resources in the background.
- Error Conditions: any validation failure -> 400 with a specific field error message (e.g., project-name rule message, email rule message, "Invalid cloud account ID", "OAuth token required", "Invalid custom domain format"); unexpected failure launching the run -> 500 with the thrown message or "Failed to start deployment".
- Invariants & Guarantees: idempotent-by-name at the directory level (same tenant name updates the existing directory entry rather than duplicating). The provisioning pipeline itself is designed to be re-runnable: each resource step detects an "already exists / already taken" condition and reuses the existing resource instead of failing.

#### Query provisioning status — GET /deployment/:projectName/status
- Invocation: authenticated client request; typically polled.
- Inputs: tenant name in the path.
- Behavior: returns the current run state for that tenant.
- Success Output (200): a sanitized run-state object containing: run identifier, overall status, current step identifier, current-step percentage (0–100), overall percentage (0–100), a list of created-resource references, an ordered log list, timestamps (created/updated, and completed when finished), an error object if failed, and a credentials object. Sensitive intake values (the access credential and chosen password inside the original config) are never returned. The administrator credentials object is included only once status is "completed".
- Error Conditions: no run found for that tenant -> 404 with error "Deployment not found"; unexpected failure -> 500 with the thrown message or "Failed to get status".

#### Stream provisioning events — server-sent event stream
- Invocation: a real-time subscription to the same per-tenant run; emits server-sent events.
- Behavior: on connect, immediately emits one event named "status" carrying the same sanitized run-state payload as the status query (or an error payload if no run exists), then emits periodic "heartbeat" events carrying a current timestamp until the subscriber disconnects. In a test mode the stream sends the initial status event and then closes.
- Success Output: an event stream with content type for server-sent events.

#### Cancel a provisioning run — POST /deployment/:projectName/cancel
- Invocation: authenticated client request.
- Inputs: tenant name in the path.
- Preconditions: an active run must exist for that tenant.
- Behavior: moves the run into a "rolling back" state, tears down every resource recorded so far, then sets the run to "failed".
- Success Output (200): object with a success flag and message "Deployment cancelled".
- Side Effects: deletes the created cloud resources for that tenant (best-effort); persists the failed end state.
- Error Conditions: no active run -> 404 with error "No active deployment"; unexpected failure -> 500 with the thrown message or "Failed to cancel deployment".

#### List all provisioning runs — GET /deployments
- Invocation: authenticated administrative client request.
- Inputs: none.
- Behavior: reads the global run directory and, for each entry, fetches that tenant's live status, merging directory metadata with current status.
- Success Output (200): object with a list of run summaries and a count. Each summary carries: tenant name, run identifier, status (or "unknown" when live status is unavailable), current step, overall percentage, administrator email, account identifier, optional custom domain, creation/update/completion timestamps, resolved frontend/backend URLs when available, and an error object when failed.
- Error Conditions: directory read failure -> 500 with error "Failed to read deployment index"; unexpected failure -> 500 with the thrown message or "Failed to list deployments".

#### Unknown route — any unmatched path
- Behavior: returns 404 with an error "Not Found", a message that the endpoint does not exist, and the requested path.

#### Global error handling
- Behavior: any unhandled error returns 500. In production mode the body is a generic "Internal Server Error" message; in non-production mode it includes the error message and stack for debugging.

### Data Concepts (neutral)

- Tenant configuration (intake): tenant name, administrator email, optional administrator password, optional custom domain, cloud access credential, account identifier, plus optional explicit backend/frontend/file-store URLs, optional messaging-integration secrets/identifiers, and a log-level preference. The access credential and password are sensitive and are never echoed in any status/list response.
- Provisioning run state: a per-tenant record holding a unique run identifier, the intake configuration (sensitive parts hidden externally), an overall status, the current step, current-step and overall progress percentages, a collection of references to created cloud resources, an ordered log of timestamped entries (each with a severity of informational / success / warning / error, a message, and the step it occurred in), an optional structured error (code, message, the step it failed at, recoverability flag), an optional administrator-credentials object, and creation/update/completion timestamps.
- Created-resource references: identifiers/names for the relational database, the two key-value stores (one for sessions, one for cache), the object/file store, the message queue (id and name), the backend service (name and public URL), and the static-hosting project (id, name, and public URL).
- Administrator credentials: a derived login name (the local part of the email before the at-sign), the password (chosen or generated), and the email. Surfaced to the caller only after successful completion.
- Global run directory: a single shared list of run entries keyed by tenant name, each carrying tenant name, run identifier, administrator email, account identifier, optional custom domain, and timestamps; ordered most-recently-updated first; one entry per tenant name (reachability refresh overwrites/merges).
- Step catalog: an ordered, weighted set of provisioning steps, each with a human description, a contribution weight toward overall progress (weights total 100), a per-step timeout, and retry behavior (whether retryable and a maximum retry count). Persisted credentials/passwords are stored only in a one-way, non-reversible protected form; the plaintext is never retained or returned.

### State & Lifecycle

Provisioning-run status transitions:
- pending -> in_progress (when the asynchronous pipeline begins).
- in_progress -> completed (terminal success; sets a completion timestamp; administrator credentials become visible).
- in_progress -> rolling_back -> failed (terminal failure path; triggered by any pipeline step error after retries are exhausted, or by an explicit cancel request).
- A cancel request can move an active run to rolling_back and then failed.

Step ordering (each visible step must succeed, with retry behavior for retryable steps, before the next begins): initialize; create relational database; create session key-value store; create cache key-value store; create object/file store; create message queue; prepare persistent data structures; generate service configuration; deploy backend service; prepare/build frontend assets; deploy static-hosting site; configure custom domain (only when a custom domain was provided); create administrator account. After these, two non-fatal finalization steps run: a health verification of the deployed backend (retried several times with delay; failure only logs a warning and does not fail the run) and a completion marker.

Resource-step semantics:
- Each create step is tolerant of pre-existing resources: if creation reports an "already exists"/"already taken" condition, the step looks up and reuses the existing resource and continues. This makes the overall run safe to re-execute for the same tenant.
- The migration step refuses to proceed if the relational database reference is missing.
- The configuration/asset steps derive backend and frontend URLs with a precedence: explicitly-supplied URL, else a custom-domain-derived URL, else the actual provisioned resource URL, else a conventional default URL built from the tenant name. The backend's websocket URL is derived from the backend URL by switching to the secure websocket scheme.
- Backend deployment treats first-time vs. repeat deployment differently with respect to stateful-component migrations: on a fresh deploy it includes them; on a redeploy it omits them; and if a precondition conflict about those migrations is reported, it retries the deploy without them. After deploy it attempts to resolve the real public backend hostname (falling back to a conventional hostname) and enables public access.
- Static-hosting deployment creates or reuses the hosting project, uploads the prepared frontend asset set (warns if there are no assets), and sets hosting environment values used for dynamic security-header generation (the backend URL always; an file-store URL and a comma-joined set of allowed custom-domain origins when those were configured).
- Custom-domain configuration attaches the domain to the hosting project; if the provider indicates further DNS setup is needed, this is logged as a warning and is non-fatal (the run continues).
- Administrator creation derives the login name from the email, generates a password if none was supplied, hashes the password, and creates the account; if an account with that email already exists, it treats the account as reusable and returns credentials using the supplied/intended password instead of failing.

Terminal states: completed and failed. Once completed, the completion timestamp and administrator credentials are exposed.

### Real-time / Event Behavior

- Per-tenant event stream: on subscription, emits a single "status" event with the current sanitized run-state snapshot, then "heartbeat" events at a fixed cadence carrying a timestamp, until the subscriber disconnects (heartbeats stop on cancel). In test mode only the initial status event is sent and the stream closes.
- Progress observability: each step boundary updates the current step, resets current-step progress to zero at start and to full on success, recalculates overall progress as the cumulative weight of completed steps (capped at 100), and appends log entries (start, success, retry warnings, errors). These are observable via the status query and the event stream.
- Logging audience: log entries, current step, and progress are visible to any authorized caller polling status or listing runs; sensitive intake values and the administrator password are excluded from those views until successful completion (and even then only the credentials object, never the original access credential).
- Rollback behavior on failure or cancel: all recorded resources are torn down in reverse order of creation (custom domain, then static-hosting project, backend service, message queue, file store, cache key-value store, session key-value store, relational database); each teardown is best-effort and continues even if an individual teardown fails, with warnings logged. Custom-domain teardown is implicit (removed together with the hosting project).

## Web Installer — Frontend / Setup Wizard

### Purpose
This area is the browser-based, self-service setup wizard that lets a non-technical operator deploy a fresh instance of the multi-channel customer-support product into their own cloud hosting account. It guides the user through connecting their hosting account, naming and configuring the new instance (including optional messaging-channel credentials), reviewing a cost and resource summary, launching provisioning, watching real-time progress, and finally retrieving the generated administrator login. It is a single-page client that talks to a separate installer backend over HTTP; this section describes only the client-observable behavior (the wire calls it issues, the screens/states it shows, the validation it enforces, and the local persistence it performs).

### Operations

The wizard is organized as a sequence of screens reachable by client-side navigation. Each screen and each backend call it makes is described below.

#### Connect Hosting Account (credential entry) — client screen + POST /auth/token
- Invocation: User opens the entry/landing screen and reveals the connection form, then submits hosting-account credentials.
- Inputs (form fields, all required to enable submit):
  - API access token (string, entered masked).
  - Account identifier (string).
  - Administrator email (string, email type).
- Preconditions & Authorization: None; this is the unauthenticated entry point. The submit control is disabled while any of the three fields is empty and while a verification request is in flight.
- Behavior: On submit, the client sends the access token and account identifier to the backend verification endpoint. The body carries the token and account identifier (the admin email is not sent in this call). While waiting, the form shows a "Verifying…" busy state and disables its inputs and buttons. On success, the client persists session values (see Data Concepts) and navigates to the configuration screen. A "Cancel" control hides the form and returns to the marketing/landing content without clearing fields.
- Success Output (wire): `success` (boolean), `accountId` (string, the canonical account identifier returned by the backend), `accountName` (human-readable account label).
- Side Effects: Persists into per-tab session storage: the access token (under a generic auth-token key), the backend-returned account identifier, the account label, and the user-entered admin email. No real-time events.
- Error Conditions: If the backend responds non-2xx, the client shows an inline error message taken from the backend error text (falling back to the HTTP status text, then a generic "Verification failed. Check your token and account ID." message). The user remains on the form.
- Invariants & Guarantees: The persisted account identifier is the value returned by the backend, not necessarily the one typed. Session values live only for the current browser tab/session.

#### Begin Hosted Authorization Flow — GET /oauth/authorize
- Invocation: An alternative authentication path: client requests an authorization handoff URL from the backend.
- Inputs: A redirect-back URL (query parameter), the address the provider should return the user to after consent.
- Behavior: Client requests an authorization start descriptor and is expected to redirect the browser to the returned external authorization URL. The descriptor's anti-forgery token and proof key are intended to be stored locally for later verification (see callback operation).
- Success Output (wire): `authorizationUrl` (string, external consent URL), `state` (anti-forgery token), `codeVerifier` (proof key for the exchange).
- Error Conditions: Non-2xx surfaces as a thrown error with backend-provided or status-derived message.
- Side Effects: Intended to persist the anti-forgery token, proof key, and redirect URL into session storage for the callback step.

#### Complete Hosted Authorization — client callback screen + POST /oauth/callback
- Invocation: The external provider redirects the browser back to a dedicated callback screen with query parameters; the screen runs automatically on load.
- Inputs (from redirect query): authorization code, returned anti-forgery token; and optionally an error indicator and error description.
- Preconditions & Authorization: Requires previously stored proof key, redirect URL, and the original anti-forgery token in session storage.
- Behavior (observable sequence): On load the screen shows a "Connecting…" busy state. It first checks for a provider-supplied error and aborts if present. It requires both an authorization code and a returned anti-forgery token, else aborts. It compares the returned anti-forgery token against the stored one and aborts on mismatch (treated as a possible cross-site request forgery). It requires the stored proof key and redirect URL, else aborts. It then exchanges the code at the backend. On success it persists the session token and user email, selects the first listed account (storing its identifier and label), clears the temporary authorization artifacts from session storage, shows a brief success state, then after a short delay navigates to the configuration screen.
- Success Output (wire, from exchange): `success` (boolean), `accessToken` (string), token lifetime in seconds, a user object (`id`, `email`), and a list of accounts (each `id`, `name`, optional `type`).
- Side Effects: Persists session token, user email, selected account identifier, and selected account label; removes the temporary anti-forgery token, proof key, and redirect URL from session storage (also removed on the error path).
- Error Conditions: Any of the abort cases above, or a non-2xx exchange, switches the screen to an error state displaying the relevant message and offering a "Back to Home" navigation. The provider error path uses the error description (or error code) as the message.
- Invariants & Guarantees: Anti-forgery token must match exactly. Account auto-selection always picks the first account in the returned list.

#### Configuration Wizard (multi-step form) — client screens
- Invocation: Reached only after authentication; the route is guarded.
- Preconditions & Authorization: A navigation guard blocks entry unless both a session auth token and an account identifier are present in session storage; otherwise it redirects to the entry/landing screen. On load, the screen also re-checks these two values and redirects to landing if missing. It pre-fills the admin email field from the stored user email and displays the stored account label (or identifier) and user email as read-only context.
- Behavior: A three-step linear stepper labeled, in order, "Basic", "LINE OA", and "Review". A visual step indicator marks past steps as completed (checkmark) and the current step as active. "Next" advances only if the current step passes validation; "Back" moves to the previous step without validation. The final step shows a submit/"Start Deployment" control instead of "Next". Submitting re-validates every step except the review step; the first step that fails becomes the active step and submission aborts.
- Step 1 — Basic configuration inputs and validation:
  - Instance name (required). Must match lowercase letters, digits, and hyphens only; minimum 3 characters; maximum 50 characters. The field enforces a 50-character max length and shows a live character counter that turns to a warning tone past 40 characters and a danger tone past 47. A live "valid" affirmation appears once the value is at least 3 chars and matches the allowed pattern.
  - Admin email (required). Must match a basic email shape (text, "@", text, ".", text).
  - Admin password (required). Minimum 8 characters; entered masked.
  - Confirm password (required). Must equal the password field.
  - An optional collapsible "what you'll need" reference panel and an optional collapsible "resource name preview" panel are available. The preview panel, shown once an instance name is typed, displays the prospective derived names for the various cloud resources (a worker, a database, two key-value stores, a file bucket, a queue, and a static-hosting project), each formed by appending a fixed suffix to the instance name. This preview is purely informational.
- Step 2 — Messaging channel (LINE) configuration inputs and validation (only enforced when the "skip" option is unchecked):
  - Channel bot identifier (required when not skipping). Must start with "@" followed by lowercase letters and digits.
  - Channel access token (required when not skipping). Masked by default with a show/hide toggle.
  - Channel secret (required when not skipping). Masked with show/hide toggle. Must be exactly 32 characters.
  - Optional rich-page application identifier. If provided, must be at least 10 characters; shows a live character counter flagged danger when under 10.
  - A "Skip channel configuration (configure later)" checkbox. Checking it disables channel integration, clears all four channel fields, clears their validation errors, and exempts the step from validation. Unchecking re-enables channel integration. Collapsible inline help panels explain where to obtain each value; these are non-functional aids.
- Step 3 — Review: Read-only summary showing the instance name, admin email, the password rendered as a row of bullet characters of equal length to the password, and a channel-integration status of either "Configured" or "Will configure later" (per the skip choice). It also shows a static estimated monthly cost breakdown and a static list of resources that will be created (named by suffixing the instance name). No inputs.
- Success Output: On valid submit, the wizard initiates deployment (see next operation) and navigates to the progress screen keyed by the instance name.
- Error Conditions: Field-level validation messages render inline beneath each field; invalid steps block forward navigation. If the deployment-initiation call throws, a blocking alert shows the error message and the submit control is re-enabled, keeping the user on the review step.
- Invariants & Guarantees: Validation clears and recomputes the full error set per step evaluation. Channel fields are sent only when not skipped; otherwise they are omitted. Optional fields that are blank are omitted from the request rather than sent empty.

#### Start Deployment — POST /deployment/start
- Invocation: Triggered by submitting the configuration wizard.
- Inputs (request body): instance name; admin email; optional admin password (omitted if blank, in which case the backend is expected to generate one); optional custom domain; account identifier; auth token; optional backend service URL, frontend URL, and file-bucket public URL; optional channel access token, channel secret, channel bot identifier, and rich-page application identifier (all omitted when channel config is skipped); optional secondary-platform page access token and app secret; and an optional log-verbosity level (one of debug/info/warn/error/silent, defaulting to a normal level).
- Preconditions & Authorization: Must be authenticated (auth token and account identifier present). All required configuration fields validated client-side first.
- Behavior: The client resets its deployment tracking state, records the instance name, sets local status to "in progress", stamps a start time, sends the request, records the returned deployment identifier, appends a local "Deployment started" log line, and immediately begins polling status.
- Success Output (wire): `success` (boolean), `deploymentId` (string), and a human-readable `message`.
- Side Effects: Begins a recurring background status poll (see polling operation); local deployment tracking state is initialized.
- Error Conditions: A failed call sets local status to "failed", records the error message, appends an error log line, and re-throws so the caller (wizard) can alert the user.
- Invariants & Guarantees: Only one active deployment is tracked at a time; starting resets prior tracking state.

#### Poll Deployment Status — GET /deployment/{instanceName}/status
- Invocation: Started automatically after deployment begins; also invoked on demand when the progress or success screens load without active local deployment tracking. Repeats on a fixed three-second interval while status is "in progress"; stops automatically when status becomes terminal.
- Inputs: instance name (path).
- Behavior: Each poll updates local tracking from the response: current deployment identifier, status, current step, current-step progress percentage, overall progress percentage, provisioned-resource details, accumulated log entries (merged with de-duplication by message text, capped at a maximum retained count, ordered by timestamp), any error message, and the generated admin credentials when present. Reaching a terminal status ("completed", "failed", or "cancelled") stamps a completion time (once), stops polling, and on success appends a "completed successfully" log line.
- Success Output (wire): `deploymentId`, `status` (one of idle/in_progress/completed/failed/cancelled), `currentStep` (a step identifier or none), current-step progress (number), overall progress (number), provisioned-resource details object, an array of log entries (each with timestamp, severity level of info/success/warning/error, message text, optional step), optional error message, and optional admin credentials (login name, password, email).
- Error Conditions: A failed poll is swallowed (logged to console only) and the interval continues; it does not surface to the user.
- Invariants & Guarantees: Polling is idempotent and self-terminating at terminal states. Log de-duplication is by exact message text. Overall progress is clamped to 0–100 for display.

#### Cancel Deployment — POST /deployment/{instanceName}/cancel
- Invocation: User clicks "Cancel Deployment" on the progress screen while a deployment is in progress; a confirmation prompt must be accepted first.
- Inputs: instance name (path).
- Preconditions & Authorization: Requires an active tracked deployment (an instance name present); otherwise the client throws "No active deployment to cancel".
- Behavior: Sends the cancel request, sets local status to "cancelled", stops polling, and appends a "cancelled by user" warning log line. On the screen, success shows an informational alert stating resources will be cleaned up automatically and navigates back to the entry/landing screen.
- Success Output (wire): `success` (boolean) and a `message`.
- Error Conditions: A failed cancel records and logs the error and re-throws; the screen shows a failure alert and re-enables the cancel control.
- Invariants & Guarantees: Cancellation is treated as terminal locally and stops further polling.

#### Deployment Progress, Success, and Failure Screens — client screens
- Invocation: Reached after starting a deployment, or directly via deep link (the progress screen will fetch current status if it has no local deployment tracking, redirecting to entry/landing if that fetch fails).
- Behavior (Progress screen): Shows a header reflecting in-progress / completed / failed; an overall progress bar driven by overall progress percentage with the current step's description; a fixed ordered checklist of fifteen named provisioning stages where stages before the current one render as completed, the current one shows a spinner, and later ones render as pending (all stages render completed once status is "completed"); a live deployment log console; a provisioned-resource panel (links to the backend service URL and frontend URL, and a custom-domain value, each shown only when present); an error detail panel when an error exists; an elapsed-time readout; and contextual actions: "Cancel" while in progress, "View Credentials & Continue" when completed (navigates to success screen), and "View Error Details" plus "Try Again" when failed.
  - Authorization: The progress and configuration routes are guarded as authenticated; the success and error screens are not guarded.
- Behavior (Success screen): On load, if credentials are not already held it fetches status once. It then validates that both credentials and a frontend URL are available; if either is missing it shows a recoverable "could not load deployment data" error state with "Retry" and "Back to Home". When data is present it shows a success animation, the admin-credentials panel, a static quick-start guide, a deployed-resources table (showing backend URL, frontend URL, custom domain, database identifier, and file-bucket name when each is present), a deployment-time/steps/success-rate stat block, static "what's next" links, and actions to return home or launch the deployed dashboard. Returning home resets local deployment tracking.
- Behavior (Failure screen): On load, if there is neither a stored error nor a failed status it redirects to entry/landing. Otherwise it shows the error message (defaulting to "An unknown error occurred during deployment"), optional deployment context (instance name, the failed step rendered as a human-readable title, elapsed time), a static automatic-rollback reassurance, a static common-issues list, the deployment log console when logs exist, and actions: "Try Again" (resets state and returns to configuration), "Get Support" (opens external support links in new tabs), and "Back to Home" (resets state and returns to entry/landing).
- Side Effects: Navigation between screens; resetting local deployment tracking on certain navigations; opening external links in new tabs.

#### Admin Credentials Panel (within success screen) — client component
- Invocation: Rendered when generated admin credentials are available.
- Behavior: Displays login name, password (masked with a show/hide toggle), and email, each in a read-only field with a per-field "Copy" button. Copying writes the value to the system clipboard and briefly shows a "Copied" confirmation for about two seconds; a clipboard failure shows an alert advising manual copy. A "Download as Text File" action generates and downloads a plain-text file containing the three credentials and a generation timestamp.
- Invariants & Guarantees: Fields are read-only. The panel prominently warns that credentials are shown only once.

#### Backend Health Probe — GET /health
- Invocation: Available client helper (not part of the primary flow).
- Success Output (wire): a status string, a service name, a version string, and a timestamp.

### Data Concepts (neutral)
- Session context (per-tab, ephemeral): the hosting-account auth token, the canonical account identifier, a human-readable account label, and the operator's email. Temporary authorization artifacts (an anti-forgery token, a one-time proof key, and a redirect-back URL) exist only during the hosted authorization handoff and are deleted immediately after exchange or on error. All session context is lost when the tab/session ends.
- Deployment configuration (assembled in the wizard, sent at start): an instance name; admin email and optional admin password; optional custom domain; optional explicit service/frontend/file-bucket URLs; optional messaging-channel credentials (bot identifier, access token, secret, rich-page application identifier); optional secondary-platform credentials; and a log-verbosity level. Blank optional values are omitted from the request.
- Deployment tracking record (client-side, runtime-local): instance name, deployment identifier, status, current stage, current-stage and overall progress percentages, error message, a provisioned-resources detail bag, an ordered de-duplicated log buffer (capped), generated admin credentials, and start/completion timestamps.
- Provisioned-resource details (read-only, from backend): identifiers/labels for a database, two key-value stores, a file bucket, a queue, a worker service plus its URL, a static-hosting project plus its URL, and an optional custom domain. Displayed selectively as available.
- Log entry: a timestamp, a severity level (info, success, warning, or error), message text, and an optional associated stage.
- Admin credentials: a login name, a password, and an email, shown once and downloadable.

### State & Lifecycle
- Deployment status is a small state machine observable by the client: idle → in_progress → one of {completed, failed, cancelled}. The terminal states stop polling. Only the backend advances in_progress to completed/failed; the user may force in_progress to cancelled via the cancel action; the wizard sets failed locally if the start call itself fails.
- The wizard step state advances 0→1→2 linearly; forward moves require per-step validation, backward moves are unrestricted, and final submission re-validates all non-review steps and jumps focus to the first invalid one.
- Provisioning is presented as a fixed ordered sequence of fifteen named stages (initialization; database creation; two key-value store creations; file-bucket creation; queue creation; persistent-data preparation; configuration generation; backend service deployment; frontend build; static-hosting deployment; domain configuration; admin-account creation; health verification; completion). Stage ordering drives the completed/current/pending rendering.
- Reaching a terminal state stamps a one-time completion time used for elapsed-duration display.

### Real-time / Event Behavior
- There is no push/socket channel in this wizard; "real-time" progress is achieved by the client polling the status endpoint every three seconds while in progress and rendering the latest snapshot. Polling is the only mechanism by which progress, logs, resource details, errors, and final credentials reach the UI. The log console auto-scrolls to the newest entry whenever the entry count changes and de-duplicates incoming log messages so repeated server messages are not shown twice.


---

## Appendix A — Clean-Room Integrity Audit Ledger

During production, an independent contamination auditor re-examined every section for leakage of the *how* (internal algorithms, mechanisms, storage layout, or original identifiers). The items below were detected and have each been **neutralized in the body** of this specification: the externally observable guarantee was retained, while the disclosed internal mechanism was removed.

This ledger is provenance only. It deliberately names each leak by **category** rather than restating the mechanism, so the ledger itself introduces no contamination. The clean-room consequence of every row is identical: a re-implementer is free to choose any mechanism that satisfies the observable guarantee stated in the corresponding section.

| # | Section | Severity | Leak category (neutral) | Status |
|---|---------|----------|--------------------------|--------|
| 1 | Customers | low | Internal storage layout of soft-delete state | Neutralized to observable guarantee |
| 2 | Customers | low | Internal serialized-size constraint of stored metadata | Neutralized to observable guarantee |
| 3 | Inbound Webhook Ingestion & Platform Parsing | medium | Internal concurrency-control mechanism and its timing constants | Neutralized to observable guarantee |
| 4 | Channel Integrations | medium | Internal credential-encryption construction | Neutralized to observable guarantee |
| 5 | Channel Integrations | low | Internal credential storage-encoding variants and migration handling | Neutralized to observable guarantee |
| 6 | Activity Log & Reversible Actions | low | Internal data-structure encoding of the restore state | Neutralized to observable guarantee |
| 7 | Activity Log & Reversible Actions | low | Internal value-comparison method used for drift detection | Neutralized to observable guarantee |
| 8 | Rate Limiting & Mutual-Exclusion Guarantees | low | Trusted request-throttling computation | Neutralized to observable guarantee |
| 9 | Rate Limiting & Mutual-Exclusion Guarantees | low | Internal address-bucketing computation | Neutralized to observable guarantee |
| 10 | Rate Limiting & Mutual-Exclusion Guarantees | low | Trusted request-throttling computation (restated) | Neutralized to observable guarantee |
| 11 | Web Installer — Backend / Provisioning | medium | Internal password-protection mechanism | Neutralized to observable guarantee |

**Summary:** 11 items detected · 11 neutralized · 0 remaining. Severity mix: 8 low, 3 medium, 0 high. No original source identifier, code fragment, internal algorithm, or storage-schema name remains in this document.

*End of specification.*

# MCSS Build Progress

Resume here each session. Spec: `Rust_CRD.md`. Plan: `docs/plans/2026-06-11-mcss-implementation.md`.

**Note:** the session log below is current through 2026-07-07 (last commit
`b350999`), but it is a summary, not an exhaustive record. Per-feature detail
from 2026-06-27 onward lives in dated spec + plan pairs under
`docs/superpowers/specs/` and `docs/superpowers/plans/` (one pair per feature);
check there and `git log` alongside this log.

## Status

- [x] Design doc written (`docs/superpowers/specs/2026-06-11-mcss-architecture-design.md`)
- [x] Phase 1: core scaffold + pipeline + data model + auth + authz (34 tests green, clippy clean)
- [x] Phase 2: org — tags, customers, teams, agents, activity log (192 tests green)
- [x] Phase 3: conversations, sessions, messaging, customer-facing (316 tests green)
- [x] Phase 4: realtime — WS gateway, rooms/broadcast, user sessions, customer channels, latest-message cache, collaboration (403 tests green)
- [x] Phase 5: channels/webhooks/auto-reply/delayed/files (499 tests green)
  - [x] §4.1 channel integrations + crypto (d82c8b6)
  - [x] §4.2 webhook ingestion LINE/FB (d82c8b6)
  - [x] §2.5 auto-reply engine + management (375943f)
  - [x] §2.4 delayed messages HTTP surface (2659fc7): both route families on the shared scheduler
  - [x] §4.3 LIFF mini-page/admin/join (8a7a2d5)
  - [x] §4.4 files & attachments (31f1d39): uploads, signed proxies, direct-upload flow
  - [x] §6.5 background queue (b5d14b8): jobs, retries, DLQ, monitoring
- [x] Phase 6: ops/analytics/reports/notifications/monitoring/settings (542 tests green) — BACKEND COMPLETE
  - [x] §6.4 notifications + reminders + alerting (90454aa)
  - [x] §6.3 monitoring & health: sweeps, breaker, dashboards
  - [x] §6.1 analytics: insights/comparison/dashboards/security
  - [x] §6.2 reports: pipeline/downloads/batch/scheduling
  - [x] §6.6 system settings & admin (dbaa7f2): ~50 endpoints
  - [x] §6.7 rate-limit contract + keyed lease locks
- [x] Phase 7: frontend SPA — API layer, stores, realtime, all screens, wizard
  - [x] foundation: API client w/ refresh single-flight, session lifecycle,
        router guards, login + dashboard shell, i18n (frontend/)
  - [x] §8.1 state model: Store<T> w/ optimistic+rollback, conversations,
        messages, teams, tags, notifications, and supporting containers wired
        with cache freshness / optimistic rollback coverage (CRD 5846-6126)
  - [x] §8.2 views: ALL destination screens implemented — conversations
        list/detail, notifications, tags, teams, settings, profile, reports,
        activity, auto-reply, channels
  - [x] §8.3 realtime client core: connect/auth/reconnect/event routing,
        reconnect re-subscribe, and reconnect message resync; presence coverage
        remains a deeper follow-up.
  - [x] §8.4 routing/API/i18n: endpoint contract registry, shared client
        team-context header, and topbar team context switcher are wired.
  - [x] §8.5 traceability matrix check (6689-6723): frontend endpoint
        registry test covers every matrix area.
- [x] Phase 8: web installer — provisioning service (§9.1) + setup wizard (§9.2);
      Cloudflare API-token verification and core resource provisioning are wired,
      OAuth grant exchange is wired, and Workers/Pages deployment remains
      external.

## Current Integration Status

Track B platform work has moved beyond the original 2026-06-12
external-stub boundary:

- [x] LINE outbound: `OutboundGateway` sends real Push API requests when
      `LINE_CHANNEL_ACCESS_TOKEN` is configured; dev/test without a token keeps
      the documented no-network `stub-line-*` success.
- [x] LINE media: inbound downloadable message content is fetched through the
      LINE content API and served through the authenticated media proxy; the
      frontend renders image, sticker, video, audio, file, and location bubbles.
- [x] Outbound attachments: composer upload/drag/paste sends image, video,
      audio, and file attachments; LINE receives native media where supported
      and file links otherwise.
- [x] Facebook Messenger: Send API dispatch is wired through
      `FACEBOOK_PAGE_ACCESS_TOKEN`; webhook handling covers messages, echo,
      postback, delivery, and read receipts.
- [x] Instagram Messaging: dispatch uses `INSTAGRAM_ACCESS_TOKEN` with Facebook
      page-token fallback; webhook handling covers messages, echo, postback,
      seen, reactions, and story events.
- [x] Customer profile enrichment: LINE/Facebook/Instagram profile name and
      avatar lookup backfills placeholder customers when tokens are available.
- [x] Shopee foundation + first messaging slice: signed Open Platform v2 URLs,
      OAuth token exchange, encrypted per-shop token storage,
      refresh-before-expiry, callback wiring, signature-gated Webchat push
      ingestion, and SellerChat text outbound using shop-scoped tokens.

Remaining external/infrastructure boundaries:

- [x] Shopee inbound richer media/chat surface: Webchat media/card payloads
      preserve image preview media, sticker data, and product/order-style
      metadata for downstream rendering.
- [x] Shopee outbound native media delivery: image attachments are routed as
      SellerChat image payloads, while video/audio/file attachments keep the
      documented link-style fallback because public Shopee Chat API material
      only confirms text/image outbound support.
- [x] Installer Cloudflare API-token verification and core resource
      provisioning: `/auth/token` verifies the Bearer token/account through
      Cloudflare API, and deployment creates D1, KV, R2, and Queue resources.
- [x] Installer OAuth grant exchange: `/oauth/authorize` uses Cloudflare OAuth
      endpoints, `/oauth/callback` exchanges authorization codes for access
      tokens, and the setup wizard consumes the returned token.
- [x] Installer Workers/Pages deployment bootstrap: backend provisioning uploads
      a Cloudflare Worker script and frontend provisioning creates a Pages
      project.
- [x] Installer Worker bindings: backend Worker settings bind the provisioned
      D1, KV, R2, and Queue resources.
- [x] Installer provisioning management endpoints: project-name status polling,
      active-run cancellation, and deployment index listing are wired.
- [x] Installer optional Worker route provisioning: custom-domain runs create
      Cloudflare Worker routes for the deployed backend script.
- [x] Installer production artifact upload and Pages deployment assets: Pages
      project provisioning can submit manifest-backed deployment requests with
      build metadata, or execute server-side Wrangler direct uploads from a
      configured artifact root for binary asset deployment.
- [x] Multi-instance customer-channel realtime fan-out: customer conversation
      events relay across backend instances through Postgres event/ack tables.
- [x] Multi-instance routed broadcaster fan-out: queued broadcaster events relay
      across backend instances through Postgres event/ack tables.
- [x] Broader realtime scale-out hardening: injected room broadcasts and
      first-online / last-offline presence transitions reuse the Postgres-backed
      routed broadcaster fan-out across backend instances.
- [x] Configured alert webhook sink: monitoring/test alerts POST JSON to the
      admin-configured webhook URL and record per-channel attempt outcomes.
- [x] Slack-specific alert dispatch: chat-channel alerts send Slack Incoming
      Webhook `text` payloads instead of generic alert JSON.
- [x] Live email alert dispatch: monitoring alerts can send SMTP `email`
      channel attempts from the admin-configured email settings and record
      success/failure outcomes.
- [x] LINE follow default welcome push: fallback welcome messages are delivered
      through the outbound LINE gateway and persist platform message outcomes.
- [x] LIFF welcome endpoint live push: verified LIFF users receive the fixed
      welcome text through the outbound LINE gateway.
- [x] Env-driven security-alert sinks: selected email API, chat webhook, and
      generic webhook destinations POST live payloads and report failures.

## Current Routing Decision

Conversation assignment is team-only. The product intentionally does not assign
conversations to individual agents/operators, including the current signed-in
user. The authoritative conversation assignee is `teamId` / `assignedTeam`.

**Update 2026-07-05:** conversation *access* is no longer team-scoped. Per a
policy change (commit `8987341`), every authenticated agent can see and act on
every conversation, including other teams' and the unassigned pool; the 我的團隊
inbox tab is now just a filter, not an access boundary. This reverses the
team-scoped visibility/`can_act_on` hardening from `b37a17a`. Message/file
bulk-export authorization is a separate code path and is unaffected — it
remains team-scoped.

Abandoned scope:

- Individual-agent conversation assignment.
- "Assign to me" behavior for conversations.
- Round-robin, load-balanced, skill-based, or other automatic staff assignment.

## Conversation Routing TODO

- [x] Frontend: remove or rename the Inbox composer chip formerly labeled
      `指派給我`; it opens the existing team assignment flow and no longer
      implies assignment to the current user.
- [x] Frontend: audit visible routing copy in Inbox, ConversationDetail,
      notifications, and empty/error states so every assignment label refers to
      a team, not an individual staff member.
- [x] Frontend tests: add coverage for `AssignDialog` to verify assign and
      transfer require/select a team and never submit an agent/user id.
- [x] Frontend/API tests: add a regression test that any deprecated
      individual-agent assignment helper or alias rejects locally without
      calling the backend.
- [x] Backend tests: add a schema/response assertion so assignment responses do
      not grow `agentId` / `assigneeId` fields, while keeping the existing
      assign/unassign/transfer/bulk-assign coverage.

Future documentation rule: when user-facing help text is added, describe
conversation routing as "指派至團隊 / 轉接團隊 / 取消指派" only.

## Session log

- 2026-06-11: Read CRD §0/§7/§1.1; wrote design doc + Phase 1 plan; started Phase 1.
- 2026-06-11 (cont.): Phase 1 committed (74599da). Phase 2 via subagents: tags+customers
  (653905f), teams+agents (1e6f4be), activity log + restore (ca76b79). 192 tests, clippy clean.
  Realtime broadcasts are TODO(realtime) markers pending Phase 4.
- 2026-06-11 (cont. 2): Phase 3 via subagents: conversations+sessions (2041334),
  messaging+customer-facing (d34bf26). 316 tests. Delayed-message dispatcher loop in main.rs.
- 2026-06-11 (cont. 3): Phase 4 underway: WS gateway + hub (191ea2c); conversation rooms,
  routed broadcast delivery & user realtime sessions (CRD §5.2, §5.3) — room WS auth modes
  (token / challenge+signature / simplified), reconnection sync, broadcaster queue endpoints
  under /api/realtime/broadcaster, user session surface under /api/realtime/session with
  persisted user state (migration 0007). 369 tests. Remaining: §5.4 customer-side channels
  (/api/realtime/typing, /broadcast, /conversation/:id/status, /online-status, config/stats/
  monitoring endpoints at CRD 3847-4080).
- 2026-06-12: Phase 4 complete: gateway (191ea2c), rooms+user sessions (8e6668e),
  customer channels+module+collaboration (6b0929d). 403 tests, clippy clean.
- 2026-06-12 (cont.): channels+webhooks committed (d82c8b6), auto-reply (375943f).
  462+ tests green. Inline implementation (user prefers no subagents).
- 2026-06-12 (cont. 2): §2.4 delayed messages done (2659fc7). 471 tests green, clippy clean.
  Next: §4.3 LIFF (CRD 2862-2996), §4.4 files (CRD 2996-3221), §6.5 queue (CRD 5106-5247),
  then Phase 6 (CRD 4201-5578), Phase 7 frontend (5844-6723), Phase 8 installer (6724-6979).
- 2026-06-12 (cont. 3): §4.3 LIFF done (8a7a2d5). 479 tests green, clippy clean.
  PAUSED here (context limit). NEXT STEP: §4.4 File & Attachment Management
  (CRD lines 2996-3221, ~20 ops: authenticated upload/list/download + the
  signature-protected public proxy — note §7.1 line 5706 signed-proxy rule;
  partial pieces exist in conversations/messaging attachments + config.upload_dir).
  Then §6.5 background queue (CRD 5106-5247), then Phase 6 (CRD 4201-5578).
  Pattern reminders: domain/<area>/{mod,handlers,store}.rs; envelope::*;
  AppError; require_auth/require_admin; tests per op in backend/tests/<area>.rs;
  cargo test + clippy clean before commit; user prefers inline work (no subagents).
- 2026-06-12 (cont. 4): §4.4 files (31f1d39), §6.5 queue (b5d14b8). Phase 5 COMPLETE.
  Phase 6 next: §6.4 notifications (CRD 4881-5106), §6.3 monitoring (4697-4881),
  §6.1 analytics (4201-4505), §6.2 reports (4505-4697), §6.6 settings (5247-5488),
  §6.7 rate-limit guarantees (5488-5578).
- 2026-06-12 (cont. 5): §6.4 notifications (90454aa), §6.3 monitoring (df98c1f),
  §6.7 rate-limit contract + lease locks (e5474c5). 518 tests, clippy clean.
  PAUSED (context limit). NEXT: §6.1 analytics (CRD 4201-4505), §6.2 reports
  (CRD 4505-4697, tables already in 0001), §6.6 settings (CRD 5247-5488),
  then Phase 7 frontend (5844-6723), Phase 8 installer (6724-6979).
- 2026-06-12 (cont. 6): §6.1 analytics (1d0b78a), §6.2 reports (6ac745f),
  §6.6 system/admin (dbaa7f2). 542 tests, clippy clean. ENTIRE BACKEND (Phases 1-6)
  COMPLETE: all 7 CRD backend sections implemented with per-operation tests.
  NEXT: Phase 7 frontend SPA (CRD 5844-6723: state model 5846, views 6127,
  realtime client 6332, routing/API/i18n 6465, traceability 6689). Suggest
  Vite+React in frontend/. Then Phase 8 installer (6724-6979).
- 2026-06-12 (cont. 7): Phase 7 started — frontend foundation committed.
  npm run build green. Resume with §8.1 state model + §8.2 conversation views.
- 2026-06-12 (cont. 8): §8.1 store layer + conversations screen committed.
- 2026-06-12 (cont. 9): §8.3 realtime client committed.
- 2026-06-12 (cont. 10): §8.2 conversation detail committed.
- 2026-06-12 (cont. 11): notifications center (732fe48), shell nav + tags
  screen committed. Remaining §8.2: profile, reports/export, admin screens
  (teams/channels/settings/auto-reply/activity). Then §8.5 + Phase 8.
- 2026-06-12 (cont. 12): teams screen (6394a26), settings screen committed.
  Remaining §8.2 screens: profile, reports/export, channels, activity,
  auto-reply. Then §8.5 traceability + Phase 8 installer (CRD 6724-6979).
- 2026-06-12 (cont. 13): profile (9f37b60) + reports screens committed.
  Remaining §8.2: channels admin, activity log, auto-reply config screens.
  Then §8.5 traceability + Phase 8 installer.
- 2026-06-12 (cont. 14): activity (b33bf8a), auto-reply + channels (6d574b4).
  Phase 7 substantially complete: API layer, stores, realtime client, shell,
  and every §8.2 destination screen. Remaining: §8.5 traceability sweep,
  deeper §8.1 containers (teams/tags stores), §8.3 sync-after-reconnect.
  Then Phase 8 installer (CRD 6724-6979).
- 2026-06-12 (cont. 15): installer binary committed. All 9 CRD phases now
  have running implementations. Remaining polish: §9.2 wizard UI, §8.5
  traceability sweep, TODO(cloud)/TODO(channels) live integrations. The
  channel TODO portion was later superseded by the 2026-06-21 through
  2026-06-25 Track B work summarized above.
- 2026-06-12 (FINAL): setup wizard (b1f9252). §8.5 traceability sweep PASSED:
  all 17 frontend API call paths map to registered backend routes (348 total).
  ALL CRD SECTIONS (§1-§9) IMPLEMENTED. At that checkpoint, open items were
  external-integration stubs: TODO(channels) live LINE/Facebook APIs,
  TODO(cloud) provider calls, TODO(scale-out) multi-instance realtime. The
  channel status has since advanced to real LINE/Facebook/Instagram gateways
  when credentials are configured.
- 2026-06-12 (verification): hooks-order fix (3edaccc), CI (f24ae31), frontend
  vitest 7 tests (3e438ba), installer tests + .env.example (34419cc).
  FINAL FULL PASS: backend+installer 545 tests green, clippy 0 warnings,
  frontend 7 tests + build green. Project complete for the original CRD; later
  Track B platform integration work reduced the channel stub boundary.
- 2026-06-12 (deploy): docker compose stack RUNTIME-VERIFIED (05829de) —
  fixed volume ownership, healthcheck without wget, nginx upstream ordering,
  lockfile platform flags. Backend healthy via nginx proxy, SPA serves.
  Project fully delivered: code + 552 tests + smoke + containers + docs.
- 2026-06-25: Documentation refresh — README and this progress log updated to
  reflect the current platform state: real credentialed LINE/Facebook/Instagram
  gateways, LINE media proxy and attachment send, profile avatar/name
  enrichment, and Shopee auth/signing/token foundation. Remaining work is
  Shopee full messaging, cloud provisioning, multi-instance realtime fan-out,
  and optional external notification/welcome-reply polish.
- 2026-06-26: Customer-channel realtime scale-out implemented and verified:
  customer conversation events now relay across backend instances via
  Postgres-backed fan-out events and per-instance acks. Remaining scale-out work
  is limited to broader room/presence surfaces if required by deployment.
- 2026-06-26: Frontend realtime reconnect hardening: vitest coverage now guards
  deferred subscribe flushing, reconnect re-subscription, and reconnect events;
  Inbox and ConversationDetail reload message history after reconnect to resync
  any events missed while offline.
- 2026-06-26: Shopee messaging slice implemented: `/api/webhooks/shopee` accepts
  signature-gated Webchat push events, normalizes text/media payloads into the
  shared customer/conversation/message ingestion pipeline, dedupes redelivery by
  platform message id, and routes outbound text through SellerChat
  `/api/v2/sellerchat/send_message` with `shop_id:buyer_id` recipients.
- 2026-06-26: Alert webhook sink implemented: `/api/alert-config/channels/webhook`
  configuration is now used by monitoring/test alerts, which best-effort POST
  a JSON payload to the configured URL and persist success/failure in
  `channelAttempts`.
- 2026-06-26: Slack alert dispatch hardened: configured `alert.slack` now
  receives Slack Incoming Webhook `text` payloads for chat-channel alerts, with
  delivery success/failure still recorded in `channelAttempts`.
- 2026-06-27 – 2026-07-05: frontend/ops round tracked as spec + plan pairs under
  `docs/superpowers/`: platform-credentials UI, team-manager role, assign/
  transfer UX, teams add-member and team-manager access, plus the conversations
  full-access policy change recorded above under "Current Routing Decision".
- 2026-07-06: UX/a11y round — shared loading/error states + keyboard a11y
  (cf2a06f), dark-mode-safe colors + zh-TW i18n sweep (9c20c1b), inbox drafts,
  unread badges, sound/desktop alerts, reconnect banner, notification
  deep-links, inline team-assign in the thread header.
- 2026-07-06: Frontend live-test QA (`docs/qa/2026-07-frontend-live-test.md`,
  8b0bdc2): 11 browser-driven flows PASS, 3 findings raised and all fixed the
  same day — F1 seed `conversations.updated_at` (c5dea43), F2 team names in the
  `/me` teams list (b97ad9c), F3 customer-panel team label refresh after
  assign/transfer (c2cc6cc) — plus a follow-on F4, teams in the login response
  so a fresh session has team context (bc9932d).
- 2026-07-06/07: Meta FB/IG LINE-parity GAP closure (spec
  `docs/superpowers/specs/2026-07-06-meta-fb-ig-parity-gap-closure-design.md`;
  no separate plan file — implemented directly in four commits). G1 native
  outbound media for Messenger/Instagram, G3 actionable 24h-window error, G4
  token-expiry detection into `channel_integrations.last_error` (f8cd472);
  G2 FB/IG inbound media through the authenticated media proxy (be2c82c);
  G6 Instagram credential verify, G7 single `META_GRAPH_URL` Graph version
  (01a9391); G5 `HUMAN_AGENT` tag reserved behind `META_HUMAN_AGENT_TAG`,
  default off (b350999). **G8 (shared "Meta core" abstraction) was scoped as
  optional and is not implemented** — the current code is already close to DRY.
  Spec §8 listed four review questions, resolved as follows: full G1–G7 scope;
  G2 as a **view-time proxy** of the Meta CDN URL (not the spec's recommended
  mirror-at-ingest) — mirroring to survive Meta URL expiry is a follow-up;
  G5 off by default; G3 backend-only, so the 24h-window error text currently
  reaches the frontend only in the realtime delivery payload's `error` field
  with no dedicated UI surface. Those two are the open follow-ups.

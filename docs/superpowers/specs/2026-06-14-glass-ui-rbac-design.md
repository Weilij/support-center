# Design: Apple-Glass UI + Three-Position Access Control

Date: 2026-06-14
Status: Approved (brainstorming)

## Summary

Two independently-shippable workstreams over the existing MCSS frontend:

- **B. Access control** — replace the binary `admin`/`agent` frontend gating with
  **three positions** (系統管理員 / 主管・分析師 / 客服) and a fixed position→feature
  mapping. Analytics is granted to supervisors and above; the System area is
  reserved for system admins. A system admin can change any member's position
  from the frontend.
- **A. Apple-Glass visual refresh** — a global glassmorphism theme layer
  (design tokens + restyled shared components) applied across all pages.

Decisions locked during brainstorming:

- Enforcement is **frontend gating, primarily**. The backend stores a `position`
  string and returns it, but does **not** make permission decisions from it.
  (This matches the status quo: analytics/monitoring/system routes are already
  `require_auth`-only; the "admin only" restriction lives entirely in the
  frontend `adminOnly` route flag today.)
- **Three positions**, **fixed (code-defined) mapping**, **global theme layer**.
- **Only `system_admin`** may change a member's position.
- **自動回覆 (auto-reply) is available to 客服** (daily area), unlike the rest of
  営運管理.

Out of scope: real backend RBAC enforcement, per-user/per-module configurable
permission matrix, dark theme.

---

## Part 1 — Position model (minimal backend change)

### Data

Migration `backend/migrations/0012_agent_position.sql`:

```sql
ALTER TABLE agents ADD COLUMN position TEXT;
```

Nullable. No backfill required — the frontend derives a fallback when null.

### Positions

| `position` value | Display (zh-TW) | Default source |
|---|---|---|
| `system_admin` | 系統管理員 | role = `admin` (when position is null) |
| `supervisor`   | 主管／分析師 | assigned by a system admin |
| `agent`        | 客服 | role = `agent` (when position is null) |

### Backend surface (no permission logic added)

1. `agent_view` (used by `GET /api/auth/me` and login) returns `position`
   (the raw column value; may be null).
2. `operator_view` (used by `GET /api/agents`) returns `position`.
3. `PUT /api/agents/{agentId}` accepts an optional `position` field and persists
   it. This endpoint is already admin-gated (`is_admin()`); position changes
   therefore require backend `role = admin`. Valid values:
   `system_admin | supervisor | agent`; reject others with 400.
4. `AgentRow` (auth) and `OperatorRow` (agents) structs gain
   `position: Option<String>`.

The backend never branches on `position` for authorization — all feature gating
is frontend-side (Part 3).

### Note on consistency

A member promoted to `supervisor` keeps backend `role = agent`, so backend
endpoints that still call `is_admin()` internally (e.g. team CRUD, agent
management mutations) will reject their writes with 403. This is the accepted
limitation of "frontend gating, primarily": supervisors get the analytics +
ops **views**, but write-actions that the backend admin-gates remain admin-only
until a future real-RBAC effort. The position→feature map (Part 2) is about
navigation/visibility, not backend write-enforcement.

---

## Part 2 — Position ↔ feature mapping (fixed, frontend)

Four areas; every nav item and route is tagged with exactly one `area`.

| area | features | 客服 `agent` | 主管 `supervisor` | 系統 `system_admin` |
|---|---|:--:|:--:|:--:|
| `daily` | 儀表板, 對話, 客戶, 訊息搜尋, 提醒, 通知, 標籤, **自動回覆** | ✅ | ✅ | ✅ |
| `ops` | 團隊, 客服人員, 工作階段 | — | ✅ | ✅ |
| `analytics` | 數據分析, 報表, 活動日誌 | — | ✅ | ✅ |
| `system` | 監控, 告警, 維護, 系統設定, LIFF | — | — | ✅ |

`AREA_ACCESS` (the authoritative mapping):

```
agent        → [daily]
supervisor   → [daily, ops, analytics]
system_admin → [daily, ops, analytics, system]
```

Auto-reply is intentionally `daily` (all positions) per the user's
instruction, even though the rest of 営運管理 is `ops`.

---

## Part 3 — Frontend permissions module

### New: `src/auth/permissions.ts`

- `export type Position = 'system_admin' | 'supervisor' | 'agent'`
- `export type Area = 'daily' | 'ops' | 'analytics' | 'system'`
- `positionOf(identity): Position` — returns `identity.position` when it is a
  valid Position; otherwise falls back: `role === 'admin' → system_admin`, else
  `agent`.
- `AREA_ACCESS: Record<Position, Area[]>` as above.
- `can(position: Position, area: Area): boolean`.
- `POSITION_LABELS: Record<Position, string>` (zh-TW) for UI.

### `src/auth/session.ts`

- `Identity` interface gains `position?: string`.
- Add `position()` accessor returning `positionOf(identity)`.
- Keep `isAdmin()` (still used where backend-admin truly matters, e.g. the
  position-edit control gates on `position() === 'system_admin'`).

### `src/Shell.tsx`

- `NavItem` replaces `admin?: boolean` with `area: Area`.
- Nav groups map to areas; filter items with
  `can(session.position(), item.area)`. Hide a whole group when it has no
  visible items.
- Group titles unchanged (日常 / 管理 / 分析 / 系統), but 自動回覆 moves into the
  日常 group (it is `daily`).

### `src/router.tsx`

- `RouteMeta` replaces `adminOnly?: boolean` with `area?: Area` (default
  `daily`).
- `Guard` checks `can(session.position(), meta.area)`; on failure redirect to
  `/dashboard` (consistent with existing snapshot/guard behaviour).
- Each route declares its `area` per the Part 2 table.

### Page-level gates

Replace `session.isAdmin()` early-returns with position checks:

- `Teams`, `Sessions` (ops) → allow `supervisor`+ (`can(pos,'ops')`).
- `AutoReply` (daily) → allow everyone (remove its admin gate).
- `Activity` (analytics) → allow `supervisor`+.
- `LiffSettings`, `Settings`, `SystemMonitoring`, `AlertConfig`,
  `SystemMaintenance` (system) → `system_admin` only.

### Position management UI (system_admin only)

In **`src/pages/Agents.tsx`** (客服人員管理):

- Add a **職位** column rendering a `<select>` of the three positions, bound to
  each agent's `position` (fallback-derived when null).
- On change, `PUT /api/agents/{agentId}` with `{ position }`; optimistic update +
  Toast on success/failure.
- The column + select render only when `session.position() === 'system_admin'`;
  other viewers see a read-only label.
- Below the roster, a read-only **職位權限對照表** rendering the Part 2 matrix so
  the admin sees what each position can access.

### Store

- `src/stores/agents.ts`: `Agent` gains `position?: string`; add
  `setAgentPosition(agentId, position)` calling the PUT.

### Tests

- `src/__tests__/permissions.test.ts`: `positionOf` fallback logic (null →
  derived; valid value passthrough; unknown value → fallback) and `can` for all
  9 position×area combinations.

---

## Part 4 — Apple-Glass global theme layer

### New: `src/styles/theme.css` (imported once in `src/main.tsx`)

Design tokens (CSS custom properties on `:root`):

- App background: soft multi-stop gradient (e.g. `#eef2ff → #f5f3ff → #ecfeff`).
- `--glass-bg: rgba(255,255,255,0.55)`
- `--glass-bg-strong: rgba(255,255,255,0.72)`
- `--glass-blur: 20px`
- `--glass-border: rgba(255,255,255,0.6)`
- `--radius: 14px`
- `--shadow: 0 8px 32px rgba(31,38,135,0.12)`
- accent (`--accent: #3B82F6`), text (`--text: #1f2937`, `--muted: #6b7280`)

Base element styles (so even inline-styled pages inherit the aesthetic):

- `body`: fixed gradient background, base font, `--text` color.
- `button`: glass surface, rounded, subtle border, hover lift; primary variant
  via `.btn-primary` accent fill.
- `input, select, textarea`: translucent surface, `--radius`, focus ring.
- Custom scrollbar styling.
- A reusable `.glass` utility class (`background: var(--glass-bg);
  backdrop-filter: blur(var(--glass-blur)); border; border-radius; box-shadow`).

### Restyle shared components to glass

Update these to consume the tokens / `.glass`:

- `Shell.tsx` nav → sticky translucent glass bar with blur.
- `components/Modal.tsx` (Modal/Drawer/ConfirmDialog) → glass surfaces; keep the
  dimmed backdrop.
- `components/DataTable.tsx` → glass container, translucent header row.
- `components/ui.tsx` → `StatCard`, `Badge`, `StatusPill`, `FilterBar` glassified;
  `Toast` already dark — give it a frosted dark variant.
- `components/Form.tsx` controls inherit the global input styling.

Because most pages render through these shared components plus the global base
styles, the refresh propagates broadly in one pass. A small number of pages with
hardcoded white `background` inline styles (e.g. card wrappers) will be adjusted
to use `.glass`/tokens where they form visible chrome.

Light glass only (frosted cards over a soft gradient) — closest to Apple Glass.

---

## Verification

- `npm run build` (`tsc -b && vite build`) clean.
- `vitest run` green, including the new `permissions.test.ts`.
- Dev server smoke: log in as the seeded admin (`admin@example.com`/`admin123`),
  confirm the System group is visible; set a second user to `supervisor` and
  confirm System is hidden but Analytics/Ops show; set to `agent` and confirm
  only the daily area (incl. auto-reply) shows.
- Visual: screenshot a couple of pages to confirm the glass treatment.

## File-change inventory

Backend:
- `backend/migrations/0012_agent_position.sql` (new)
- `backend/src/domain/auth/handlers.rs` (`agent_view` + `position`)
- `backend/src/domain/auth/store.rs` or agents store (`AgentRow.position`)
- `backend/src/domain/agents/store.rs` (`OperatorRow.position`, `operator_view`)
- `backend/src/domain/agents/handlers.rs` (`update_agent` accepts `position`)

Frontend:
- `src/auth/permissions.ts` (new), `src/__tests__/permissions.test.ts` (new)
- `src/auth/session.ts`, `src/Shell.tsx`, `src/router.tsx`
- `src/pages/Agents.tsx`, `src/stores/agents.ts`
- `src/pages/{Teams,Sessions,AutoReply,Activity,LiffSettings,Settings,SystemMonitoring,AlertConfig,SystemMaintenance}.tsx` (gate swaps)
- `src/styles/theme.css` (new), `src/main.tsx` (import)
- `src/components/{Modal,DataTable,ui,Form}.tsx`, shared-component glass restyle
```

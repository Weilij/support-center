# Glass UI + Three-Position Access Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a three-position (系統管理員/主管・分析師/客服) frontend access-control layer with system-admin position editing, and apply an Apple-Glass global theme across the MCSS frontend.

**Architecture:** Backend gains one nullable `position` column on `agents`, returned by `/api/auth/me` and `/api/agents` and writable via the already-admin-gated `PUT /api/agents/{id}` — it stores data only, makes no authorization decisions. The frontend owns all gating through a `permissions` module mapping positions to feature areas (`daily`/`ops`/`analytics`/`system`), wired into the nav (`Shell`), routing (`router`), and page guards. A global `theme.css` plus glass-restyled shared components delivers the visual refresh.

**Tech Stack:** Rust/axum + sqlx + PostgreSQL (backend); React 18 + react-router 6 + Vite + Vitest, inline styles + new global CSS (frontend).

**Conventions:** Run all frontend commands from `frontend/`. Verify each frontend task with `node_modules/.bin/tsc --noEmit` and `npx vitest run`. Commits are optional and batched per the existing workflow on this branch (the working tree already holds prior uncommitted feature work) — a maintainer may squash at the end; commit steps below are provided but may be deferred.

---

## Workstream B — Access control

### Task 1: Backend `position` field

**Files:**
- Create: `backend/migrations/0012_agent_position.sql`
- Modify: `backend/src/domain/auth/handlers.rs` (`agent_view`, ~line 41)
- Modify: `backend/src/domain/auth/store.rs` (`AgentRow` struct)
- Modify: `backend/src/domain/agents/store.rs` (`OperatorRow` struct + `operator_view`, ~line 52)
- Modify: `backend/src/domain/agents/handlers.rs` (`update_agent`)

- [ ] **Step 1: Add the migration**

Create `backend/migrations/0012_agent_position.sql`:

```sql
-- Advisory position used by the frontend for feature gating. The backend stores
-- and returns it but makes no authorization decisions from it.
ALTER TABLE agents ADD COLUMN position TEXT;
```

- [ ] **Step 2: Add `position` to `AgentRow`**

In `backend/src/domain/auth/store.rs`, add to the `AgentRow` struct (the one returned by `find_agent_by_id`):

```rust
    pub position: Option<String>,
```

If `AgentRow` is built via `SELECT *` / `query_as`, the new column is picked up automatically; otherwise add `position` to its explicit column list.

- [ ] **Step 3: Return `position` from `agent_view`**

In `backend/src/domain/auth/handlers.rs`, inside `agent_view` (~line 41), add to the `json!` object:

```rust
        "position": agent.position,
```

- [ ] **Step 4: Add `position` to `OperatorRow` + `operator_view`**

In `backend/src/domain/agents/store.rs`, add `pub position: Option<String>,` to `OperatorRow`, and in `operator_view` (~line 52) add:

```rust
        "position": o.position,
```

Ensure the operator `SELECT` includes `position` (if it uses an explicit column list rather than `*`).

- [ ] **Step 5: Accept `position` in `update_agent`**

In `backend/src/domain/agents/handlers.rs`, in `update_agent` (PUT `/api/agents/{agentId}`, already admin-gated), parse an optional `position` from the body, validate against the allowed set, and persist:

```rust
    if let Some(position) = body.get("position").and_then(|v| v.as_str()) {
        if !["system_admin", "supervisor", "agent"].contains(&position) {
            return Err(AppError::BadRequest(
                "position must be one of: system_admin, supervisor, agent".into(),
            ));
        }
        sqlx::query("UPDATE agents SET position = $1, updated_at = $2 WHERE id = $3")
            .bind(position)
            .bind(crate::db::now_iso())
            .bind(&agent_id)
            .execute(&state.db)
            .await?;
    }
```

(Adapt `agent_id`/`state`/binding style to the surrounding handler; if `update_agent` already builds a dynamic UPDATE, add `position` to that builder instead of a second query.)

- [ ] **Step 6: Build and smoke-test the backend**

Run from `backend/`: `cargo build` then restart the running server.
Expected: compiles; migration `0012` applies on startup (idempotent).
Verify the field round-trips:

```bash
curl -s -X POST localhost:3000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@example.com","password":"admin123"}' | grep -o '"position":[^,}]*'
```

Expected: response includes a `position` key (value may be `null` for now).

- [ ] **Step 7: Commit (optional)**

```bash
git add backend/migrations/0012_agent_position.sql backend/src/domain/auth backend/src/domain/agents
git commit -m "feat(backend): advisory agent.position field for frontend gating"
```

---

### Task 2: Frontend `permissions` module (TDD)

**Files:**
- Create: `frontend/src/auth/permissions.ts`
- Test: `frontend/src/__tests__/permissions.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/__tests__/permissions.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { positionOf, can, AREA_ACCESS } from '../auth/permissions'

describe('positionOf', () => {
  it('passes through a valid explicit position', () => {
    expect(positionOf({ position: 'supervisor', role: 'agent' })).toBe('supervisor')
  })
  it('falls back to system_admin for admin role when position is null', () => {
    expect(positionOf({ role: 'admin' })).toBe('system_admin')
  })
  it('falls back to agent otherwise', () => {
    expect(positionOf({ role: 'agent' })).toBe('agent')
    expect(positionOf({})).toBe('agent')
  })
  it('ignores an unknown position value and falls back', () => {
    expect(positionOf({ position: 'wizard', role: 'admin' })).toBe('system_admin')
  })
})

describe('can', () => {
  it('agent sees only daily', () => {
    expect(can('agent', 'daily')).toBe(true)
    expect(can('agent', 'ops')).toBe(false)
    expect(can('agent', 'analytics')).toBe(false)
    expect(can('agent', 'system')).toBe(false)
  })
  it('supervisor sees daily, ops, analytics but not system', () => {
    expect(can('supervisor', 'analytics')).toBe(true)
    expect(can('supervisor', 'ops')).toBe(true)
    expect(can('supervisor', 'system')).toBe(false)
  })
  it('system_admin sees everything', () => {
    expect(['daily', 'ops', 'analytics', 'system'].every((a) => can('system_admin', a as never))).toBe(true)
  })
  it('AREA_ACCESS is the source of truth', () => {
    expect(AREA_ACCESS.agent).toEqual(['daily'])
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run from `frontend/`: `npx vitest run src/__tests__/permissions.test.ts`
Expected: FAIL — cannot resolve `../auth/permissions`.

- [ ] **Step 3: Implement the module**

Create `frontend/src/auth/permissions.ts`:

```ts
// Frontend access-control (spec 2026-06-14). Three positions map to feature
// areas; the backend stores `position` but does not enforce it — all gating is
// here.

export type Position = 'system_admin' | 'supervisor' | 'agent'
export type Area = 'daily' | 'ops' | 'analytics' | 'system'

export const AREA_ACCESS: Record<Position, Area[]> = {
  agent: ['daily'],
  supervisor: ['daily', 'ops', 'analytics'],
  system_admin: ['daily', 'ops', 'analytics', 'system'],
}

export const POSITION_LABELS: Record<Position, string> = {
  system_admin: '系統管理員',
  supervisor: '主管／分析師',
  agent: '客服',
}

const POSITIONS: Position[] = ['system_admin', 'supervisor', 'agent']

/// Resolve a user's position: an explicit, valid `position` wins; otherwise
/// derive from the backend role (admin → system_admin, else agent).
export function positionOf(identity: { position?: string; role?: string } | null | undefined): Position {
  const p = identity?.position
  if (p && (POSITIONS as string[]).includes(p)) return p as Position
  return identity?.role === 'admin' ? 'system_admin' : 'agent'
}

export function can(position: Position, area: Area): boolean {
  return AREA_ACCESS[position].includes(area)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/__tests__/permissions.test.ts`
Expected: PASS (all cases).

- [ ] **Step 5: Commit (optional)**

```bash
git add frontend/src/auth/permissions.ts frontend/src/__tests__/permissions.test.ts
git commit -m "feat(frontend): position→area permissions module"
```

---

### Task 3: `session.ts` position accessor

**Files:**
- Modify: `frontend/src/auth/session.ts` (`Identity` interface ~line 11; accessors ~line 64)

- [ ] **Step 1: Extend the Identity type**

In `frontend/src/auth/session.ts`, add `position?: string` to the `Identity` interface (alongside `role?: string`).

- [ ] **Step 2: Add the `position()` accessor**

Add the import at the top:

```ts
import { positionOf, type Position } from './permissions'
```

In the exported `session` object (next to `isAdmin`), add:

```ts
  position: (): Position => positionOf(identity),
```

- [ ] **Step 3: Verify typecheck**

Run: `node_modules/.bin/tsc --noEmit`
Expected: no output (clean).

---

### Task 4: `Shell.tsx` nav by area

**Files:**
- Modify: `frontend/src/Shell.tsx`

- [ ] **Step 1: Switch NavItem from `admin` to `area`**

In `frontend/src/Shell.tsx`, change the `NavItem` interface and `NAV_GROUPS` so each item carries an `area` (import `type Area` and `can` from `./auth/permissions`). Auto-reply moves into the 日常 group. Use:

```ts
import { can, type Area } from './auth/permissions'

interface NavItem {
  to: string
  label: string
  area: Area
  badge?: 'unread'
}

const NAV_GROUPS: { title: string; items: NavItem[] }[] = [
  {
    title: '日常',
    items: [
      { to: '/dashboard', label: '儀表板', area: 'daily' },
      { to: '/conversations', label: '對話', area: 'daily' },
      { to: '/customers', label: '客戶', area: 'daily' },
      { to: '/messages/search', label: '訊息搜尋', area: 'daily' },
      { to: '/reminders', label: '提醒', area: 'daily' },
      { to: '/auto-reply', label: '自動回覆', area: 'daily' },
      { to: '/tags', label: '標籤', area: 'daily' },
      { to: '/notifications', label: '通知', area: 'daily', badge: 'unread' },
    ],
  },
  {
    title: '營運管理',
    items: [
      { to: '/agents', label: '客服人員', area: 'ops' },
      { to: '/teams', label: '團隊', area: 'ops' },
      { to: '/sessions', label: '工作階段', area: 'ops' },
    ],
  },
  {
    title: '分析',
    items: [
      { to: '/analytics', label: '數據分析', area: 'analytics' },
      { to: '/reports', label: '報表', area: 'analytics' },
      { to: '/activity', label: '活動日誌', area: 'analytics' },
    ],
  },
  {
    title: '系統',
    items: [
      { to: '/system/monitoring', label: '監控', area: 'system' },
      { to: '/system/alerts', label: '告警', area: 'system' },
      { to: '/system/maintenance', label: '維護', area: 'system' },
      { to: '/liff', label: 'LIFF', area: 'system' },
      { to: '/settings', label: '設定', area: 'system' },
    ],
  },
]
```

- [ ] **Step 2: Filter by `can(position, area)`**

Replace the `const isAdmin = session.isAdmin()` usage and the per-group `group.items.filter((i) => !i.admin || isAdmin)` with:

```ts
  const pos = session.position()
  // ...
  const visible = group.items.filter((i) => can(pos, i.area))
```

- [ ] **Step 3: Verify typecheck + tests**

Run: `node_modules/.bin/tsc --noEmit` then `npx vitest run`
Expected: clean; tests pass.

---

### Task 5: `router.tsx` area guard

**Files:**
- Modify: `frontend/src/router.tsx`

- [ ] **Step 1: Replace `adminOnly` with `area` in RouteMeta**

In `frontend/src/router.tsx`, import `can, type Area` from `./auth/permissions`. Change `RouteMeta` so it has `area?: Area` (default `'daily'`) instead of `adminOnly?: boolean`.

- [ ] **Step 2: Enforce in Guard**

In the `Guard` component, after the existing auth checks, add the area check:

```ts
  if (!can(session.position(), meta.area ?? 'daily')) {
    return <Navigate to="/dashboard" replace />
  }
```

- [ ] **Step 3: Tag each route's area**

Update every `page({...}, <X/>)` meta to set `area` instead of `adminOnly`:

- `daily`: `/dashboard`, `/conversations`, `/conversations/:id`, `/customers`, `/messages/search`, `/reminders`, `/notifications`, `/tags`, `/profile`, `/auto-reply`
- `ops`: `/agents`, `/teams`, `/sessions`
- `analytics`: `/analytics`, `/reports`, `/export`, `/activity`
- `system`: `/system/monitoring`, `/system/alerts`, `/system/maintenance`, `/liff`, `/settings`

Leave public routes (`/login`, `/install`, `*`) with `requiresAuth: false` and no area gating.

- [ ] **Step 4: Verify typecheck + build**

Run: `node_modules/.bin/tsc --noEmit` then `npx vite build`
Expected: clean; build succeeds.

---

### Task 6: Page-level gate swaps

**Files:**
- Modify: `frontend/src/pages/AutoReply.tsx`, `Teams.tsx`, `Sessions.tsx`, `Activity.tsx`, `LiffSettings.tsx`, `Settings.tsx`, `SystemMonitoring.tsx`, `AlertConfig.tsx`, `SystemMaintenance.tsx`

- [ ] **Step 1: Add a shared gate helper usage**

For each page that currently early-returns on `!session.isAdmin()`, replace the check with the area appropriate to that page. Pattern (example for an `ops` page like `Teams.tsx`):

```ts
import { can } from '../auth/permissions'
// ...
if (!can(session.position(), 'ops')) {
  return <main style={{ margin: '10vh auto', maxWidth: 480 }}><p>權限不足</p></main>
}
```

Apply per page:
- `AutoReply.tsx` → remove the gate entirely (area `daily`, everyone allowed).
- `Teams.tsx`, `Sessions.tsx` → `can(pos, 'ops')`.
- `Activity.tsx` → `can(pos, 'analytics')`.
- `LiffSettings.tsx`, `Settings.tsx`, `SystemMonitoring.tsx`, `AlertConfig.tsx`, `SystemMaintenance.tsx` → `can(pos, 'system')`.

Keep each gate AFTER all hooks (Rules of Hooks), matching the existing placement.

- [ ] **Step 2: Verify typecheck + tests**

Run: `node_modules/.bin/tsc --noEmit` then `npx vitest run`
Expected: clean; tests pass.

- [ ] **Step 3: Commit (optional)**

```bash
git add frontend/src/auth/session.ts frontend/src/Shell.tsx frontend/src/router.tsx frontend/src/pages
git commit -m "feat(frontend): gate nav/routes/pages by position area"
```

---

### Task 7: Agents page position editing + matrix

**Files:**
- Modify: `frontend/src/stores/agents.ts` (`Agent` interface; add `setAgentPosition`)
- Modify: `frontend/src/pages/Agents.tsx`

- [ ] **Step 1: Extend the agents store**

In `frontend/src/stores/agents.ts`, add `position?: string` to the `Agent` interface, and add (import `put` from `../api/client`):

```ts
/// System-admin-only: persist a member's position.
export async function setAgentPosition(
  agentId: string,
  position: string,
): Promise<{ ok: boolean; message?: string }> {
  const resp = await put(`/api/agents/${agentId}`, { position })
  return { ok: resp.success, message: resp.message }
}
```

- [ ] **Step 2: Add a position column (system_admin only) + matrix to Agents.tsx**

In `frontend/src/pages/Agents.tsx`:

Imports:

```ts
import { session } from '../auth/session'
import { positionOf, POSITION_LABELS, AREA_ACCESS, type Position } from '../auth/permissions'
import { setAgentPosition } from '../stores/agents'
```

Add local state and handler inside the component:

```ts
  const canEditPosition = session.position() === 'system_admin'

  const changePosition = async (agentId: string, position: string) => {
    const res = await setAgentPosition(agentId, position)
    setToast(res.ok ? '職位已更新' : res.message ?? '更新失敗')
    if (res.ok) setAgents((as) => as.map((a) => (a.id === agentId ? { ...a, position } : a)))
  }
```

Add a column to `columns` (after 角色):

```ts
    {
      key: 'position',
      header: '職位',
      width: 150,
      render: (a) =>
        canEditPosition ? (
          <select
            value={positionOf(a as { position?: string; role?: string })}
            onChange={(e) => void changePosition(a.id, e.target.value)}
            style={{ padding: '3px 6px', borderRadius: 6, border: '1px solid #ccc' }}
          >
            {(Object.keys(POSITION_LABELS) as Position[]).map((p) => (
              <option key={p} value={p}>
                {POSITION_LABELS[p]}
              </option>
            ))}
          </select>
        ) : (
          POSITION_LABELS[positionOf(a as { position?: string; role?: string })]
        ),
    },
```

Below the roster table (before the closing `</main>`), add the read-only matrix:

```tsx
      <section style={{ marginTop: 24 }}>
        <h3>職位權限對照表</h3>
        <table style={{ borderCollapse: 'collapse', fontSize: 14 }}>
          <thead>
            <tr>
              <th style={{ textAlign: 'left', padding: '6px 12px' }}>區域</th>
              {(Object.keys(POSITION_LABELS) as Position[]).map((p) => (
                <th key={p} style={{ padding: '6px 12px' }}>{POSITION_LABELS[p]}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {(['daily', 'ops', 'analytics', 'system'] as const).map((area) => (
              <tr key={area} style={{ borderTop: '1px solid #eee' }}>
                <td style={{ padding: '6px 12px' }}>{AREA_LABELS[area]}</td>
                {(Object.keys(POSITION_LABELS) as Position[]).map((p) => (
                  <td key={p} style={{ textAlign: 'center', padding: '6px 12px' }}>
                    {AREA_ACCESS[p].includes(area) ? '✅' : '—'}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </section>
```

Add the `AREA_LABELS` constant near the top of the file:

```ts
const AREA_LABELS: Record<string, string> = {
  daily: '日常',
  ops: '營運管理',
  analytics: '分析',
  system: '系統',
}
```

- [ ] **Step 3: Verify typecheck + tests + build**

Run: `node_modules/.bin/tsc --noEmit` then `npx vitest run` then `npx vite build`
Expected: clean; 14 tests pass; build succeeds.

- [ ] **Step 4: Manual round-trip check**

Start backend + dev server. Log in as `admin@example.com`/`admin123`, open 客服人員管理, change a member's 職位 to 主管／分析師, reload — the select retains the new value (persisted).

- [ ] **Step 5: Commit (optional)**

```bash
git add frontend/src/stores/agents.ts frontend/src/pages/Agents.tsx
git commit -m "feat(frontend): system-admin position editing + permission matrix"
```

---

## Workstream A — Apple-Glass theme

### Task 8: Global theme stylesheet

**Files:**
- Create: `frontend/src/styles/theme.css`
- Modify: `frontend/src/main.tsx` (add import)

- [ ] **Step 1: Create the theme stylesheet**

Create `frontend/src/styles/theme.css`:

```css
/* Apple-Glass theme layer (spec 2026-06-14). Tokens + base element styles so
   even inline-styled pages inherit the aesthetic. */
:root {
  --accent: #3b82f6;
  --text: #1f2937;
  --muted: #6b7280;
  --radius: 14px;
  --glass-bg: rgba(255, 255, 255, 0.55);
  --glass-bg-strong: rgba(255, 255, 255, 0.72);
  --glass-border: rgba(255, 255, 255, 0.6);
  --glass-blur: 20px;
  --shadow: 0 8px 32px rgba(31, 38, 135, 0.12);
}

html, body, #root { min-height: 100%; }

body {
  margin: 0;
  color: var(--text);
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
  background: linear-gradient(135deg, #eef2ff 0%, #f5f3ff 45%, #ecfeff 100%) fixed;
}

.glass {
  background: var(--glass-bg);
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
  border: 1px solid var(--glass-border);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
}

button {
  font: inherit;
  color: var(--text);
  background: var(--glass-bg-strong);
  border: 1px solid var(--glass-border);
  border-radius: 10px;
  padding: 6px 14px;
  cursor: pointer;
  transition: transform 0.08s ease, box-shadow 0.15s ease;
}
button:hover:not(:disabled) { box-shadow: var(--shadow); transform: translateY(-1px); }
button:disabled { opacity: 0.5; cursor: default; }
.btn-primary { background: var(--accent); color: #fff; border-color: transparent; }

input, select, textarea {
  font: inherit;
  color: var(--text);
  background: rgba(255, 255, 255, 0.7);
  border: 1px solid rgba(0, 0, 0, 0.1);
  border-radius: 10px;
  padding: 7px 10px;
}
input:focus, select:focus, textarea:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.18);
}

::-webkit-scrollbar { width: 10px; height: 10px; }
::-webkit-scrollbar-thumb { background: rgba(120, 120, 140, 0.35); border-radius: 8px; }
```

- [ ] **Step 2: Import it once**

In `frontend/src/main.tsx`, add near the top (after other imports):

```ts
import './styles/theme.css'
```

- [ ] **Step 3: Verify build + visual smoke**

Run: `npx vite build` (expected: succeeds). With the dev server running, load `http://localhost:5173/` — the app sits on a soft gradient; buttons/inputs are rounded and frosted.

---

### Task 9: Glass-restyle shared components

**Files:**
- Modify: `frontend/src/Shell.tsx`, `frontend/src/components/Modal.tsx`, `frontend/src/components/DataTable.tsx`, `frontend/src/components/ui.tsx`

- [ ] **Step 1: Frost the nav bar (`Shell.tsx`)**

On the `<nav>` element, change its inline style background/border to the glass treatment and make it sticky:

```ts
        style={{
          display: 'flex', gap: 20, alignItems: 'center', flexWrap: 'wrap',
          padding: '10px 16px',
          position: 'sticky', top: 0, zIndex: 100,
          background: 'var(--glass-bg)',
          backdropFilter: 'blur(var(--glass-blur))',
          WebkitBackdropFilter: 'blur(var(--glass-blur))',
          borderBottom: '1px solid var(--glass-border)',
          boxShadow: 'var(--shadow)',
        }}
```

- [ ] **Step 2: Frost overlays (`Modal.tsx`)**

In `Modal` and `Drawer`, change the white panel `background: 'white'` to `background: 'var(--glass-bg-strong)'` and add:

```ts
          backdropFilter: 'blur(var(--glass-blur))',
          WebkitBackdropFilter: 'blur(var(--glass-blur))',
          border: '1px solid var(--glass-border)',
```

Keep the existing dimmed backdrop and `borderRadius`/`boxShadow`.

- [ ] **Step 3: Frost the table container (`DataTable.tsx`)**

Wrap the table container `<div style={{ overflowX: 'auto' }}>` with glass styling:

```ts
    <div style={{ overflowX: 'auto', background: 'var(--glass-bg)', backdropFilter: 'blur(var(--glass-blur))', WebkitBackdropFilter: 'blur(var(--glass-blur))', border: '1px solid var(--glass-border)', borderRadius: 'var(--radius)', boxShadow: 'var(--shadow)' }}>
```

Change the header cell `borderBottom` color to `rgba(0,0,0,0.08)` and the body cell border to `rgba(0,0,0,0.05)` so they read on glass.

- [ ] **Step 4: Frost cards & badges (`ui.tsx`)**

In `StatCard`, change its container to use glass tokens:

```ts
        background: 'var(--glass-bg)',
        backdropFilter: 'blur(var(--glass-blur))',
        WebkitBackdropFilter: 'blur(var(--glass-blur))',
        border: '1px solid var(--glass-border)',
        borderRadius: 'var(--radius)',
        boxShadow: 'var(--shadow)',
        padding: 16, minWidth: 140,
```

In `FilterBar`, add a subtle glass panel wrapper (translucent background, `--radius`, padding). Leave `StatusPill`/`Badge` colors as-is (they are accent chips). For `Toast`, change its background to `rgba(17,17,17,0.78)` with `backdropFilter: 'blur(12px)'` for a frosted-dark look.

- [ ] **Step 5: Verify typecheck + tests + build**

Run: `node_modules/.bin/tsc --noEmit` then `npx vitest run` then `npx vite build`
Expected: clean; tests pass; build succeeds.

- [ ] **Step 6: Commit (optional)**

```bash
git add frontend/src/styles/theme.css frontend/src/main.tsx frontend/src/Shell.tsx frontend/src/components
git commit -m "feat(frontend): apple-glass global theme + glass shared components"
```

---

### Task 10: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Full build + test**

Run from `frontend/`: `npm run build` then `npx vitest run`
Expected: `tsc -b && vite build` succeeds; 14 tests pass.

- [ ] **Step 2: Position-gating manual matrix**

With backend + dev server running, log in as admin (`system_admin`): confirm the 系統 nav group shows. In 客服人員管理, create/seed a second user (or reuse) and set them to `supervisor`; log in as them: confirm 分析 + 營運管理 show but 系統 is hidden, and visiting `/system/monitoring` redirects to `/dashboard`. Set to `agent`: confirm only 日常 (incl. 自動回覆) shows.

- [ ] **Step 3: Visual confirmation**

Screenshot Dashboard, Conversations, and Agents pages; confirm the glass cards/nav render over the gradient.

---

## Self-Review

**Spec coverage:**
- Part 1 (backend position) → Task 1. ✅
- Part 2 (area mapping) → Task 2 (`AREA_ACCESS`), enforced in Tasks 4–6. ✅
- Part 3 (permissions module, session, Shell, router, page gates, Agents editing + matrix, store, test) → Tasks 2,3,4,5,6,7. ✅
- Part 4 (theme.css + base styles; glass components) → Tasks 8,9. ✅
- "Only system_admin edits position" → Task 7 `canEditPosition` gate. ✅
- "auto-reply available to 客服" → Task 4 (nav `daily`), Task 5 (route `daily`), Task 6 (gate removed). ✅
- Verification → Task 10. ✅

**Placeholder scan:** no TBD/TODO; all code steps contain concrete code. ✅

**Type consistency:** `Position`/`Area` types, `AREA_ACCESS`, `positionOf`, `can`, `POSITION_LABELS` used consistently across Tasks 2–7; `setAgentPosition` defined in Task 7 Step 1 and used in Step 2; `position` field added to backend (Task 1) and consumed by `positionOf` (Task 2). ✅
```

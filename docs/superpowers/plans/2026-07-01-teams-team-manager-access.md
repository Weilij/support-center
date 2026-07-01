# Team-Manager Access to the Teams Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an in-team lead/supervisor (even if their global position is `agent`) reach the Teams page from the 日常 nav group and manage their own team, while create/delete-team and the status toggle stay admin-only and hidden for non-admins.

**Architecture:** One additive backend change exposes the caller's in-team roles on `/api/auth/me` (merged in the `me` handler; `agent_view` untouched). The frontend gains `session.isTeamManager()`, moves the 團隊 nav item into the 日常 group behind a `show()` predicate, relaxes the Teams page gate to match, and conditionally hides admin-only controls. **No backend authorization changes** — existing `require_admin` / `require_team_rank` gates are unchanged.

**Tech Stack:** Rust (axum, sqlx, serde_json) backend; React 18 + TypeScript + Vite + vitest + @testing-library/react frontend.

**Verification gate (this repo's CI):**
- Backend: `cd backend && cargo clippy --all-targets -- -D warnings && cargo test`
- Frontend: `cd frontend && npm run build && npx vitest run`; `package-lock.json` in sync.

---

## File Structure

- `backend/src/domain/auth/handlers.rs` — **modify**: add a pure helper `membership_teams_json` + its unit test; wire it into the `me` handler to merge a `teams` array into the `/me` response. `agent_view` is NOT touched.
- `frontend/src/auth/session.ts` — **modify**: `TeamOption` gains `role?`; `readTeams` captures `roleInTeam`; add `session.isTeamManager()`.
- `frontend/src/auth/session.test.ts` — **create**: `isTeamManager()` truth-table tests.
- `frontend/src/components/AppShell.tsx` — **modify**: `NavItem` gains `show?`; export `NavGroups`; move the 團隊 item into the 日常 group with a `show` predicate; the nav filter honors `show`.
- `frontend/src/components/AppShell.test.tsx` — **create**: 團隊 nav item visible for an in-team manager, hidden for a plain member.
- `frontend/src/pages/Teams.tsx` — **modify**: relax the page gate; hide the create-team form, the 刪除團隊… button, and the status-toggle button for non-admins.
- `frontend/src/pages/Teams.test.tsx` — **modify**: convert the session mock to `vi.fn`-based (with `isAdmin`/`isTeamManager`); add a non-admin-manager test asserting admin-only controls are hidden while manager controls remain.

---

## Task 1: Backend — expose in-team roles on `/api/auth/me`

**Files:**
- Modify: `backend/src/domain/auth/handlers.rs` (`me` handler ~line 573; add helper + test module)

Context: `me` currently returns `Ok(envelope::ok(agent_view(&agent)))`. `agent_view` is shared (impact: LOW, 2 callers) so we do NOT change it. `AuthUser.teams: Vec<TeamMembership>` (fields `team_id: i64`, `role: String`, `is_primary: bool` — all `pub`, from `backend/src/state.rs`) is already populated on the request extension from the JWT; no DB call needed. `json!` and `Value` are already imported in this file (`agent_view` returns `Value`).

- [ ] **Step 1: Write the failing test**

Add this test module at the bottom of `backend/src/domain/auth/handlers.rs`:
```rust
#[cfg(test)]
mod me_teams_tests {
    use super::membership_teams_json;
    use crate::state::TeamMembership;
    use serde_json::json;

    #[test]
    fn serializes_memberships_with_in_team_role() {
        let teams = vec![
            TeamMembership { team_id: 7, role: "supervisor".into(), is_primary: true },
            TeamMembership { team_id: 9, role: "member".into(), is_primary: false },
        ];
        assert_eq!(
            membership_teams_json(&teams),
            vec![
                json!({ "teamId": 7, "roleInTeam": "supervisor", "isPrimary": true }),
                json!({ "teamId": 9, "roleInTeam": "member", "isPrimary": false }),
            ],
        );
    }

    #[test]
    fn empty_memberships_serialize_to_empty_vec() {
        assert!(membership_teams_json(&[]).is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd backend && cargo test me_teams_tests 2>&1 | tail -15`
Expected: FAIL — compile error `cannot find function membership_teams_json in this scope`.

- [ ] **Step 3: Add the helper**

Add this free function just above the `me` handler in `backend/src/domain/auth/handlers.rs`:
```rust
/// Serialize the caller's team memberships (in-team role included) for the `/me`
/// response. Kept pure (no DB) so it is unit-testable; `me` reads `AuthUser.teams`.
fn membership_teams_json(teams: &[crate::state::TeamMembership]) -> Vec<Value> {
    teams
        .iter()
        .map(|t| {
            json!({
                "teamId": t.team_id,
                "roleInTeam": t.role,
                "isPrimary": t.is_primary,
            })
        })
        .collect()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd backend && cargo test me_teams_tests 2>&1 | tail -15`
Expected: PASS (2 tests).

- [ ] **Step 5: Wire the helper into the `me` handler**

In `backend/src/domain/auth/handlers.rs`, change the `me` handler body from:
```rust
    Ok(envelope::ok(agent_view(&agent)))
```
to:
```rust
    let mut view = agent_view(&agent);
    if let Value::Object(ref mut map) = view {
        map.insert("teams".into(), json!(membership_teams_json(&user.teams)));
    }
    Ok(envelope::ok(view))
```
(The `me` signature already has `Extension(user): Extension<AuthUser>` and `agent` from `find_agent_by_id`. The `teams` key is additive; existing consumers ignore it.)

- [ ] **Step 6: Build + clippy + full backend tests**

Run:
- `cd backend && cargo clippy --all-targets -- -D warnings 2>&1 | tail -8` → no warnings.
- `cd backend && cargo test 2>&1 | tail -15` → green.

- [ ] **Step 7: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add backend/src/domain/auth/handlers.rs
git commit -m "feat(auth): /me exposes the caller's in-team roles (teams[].roleInTeam)"
```

---

## Task 2: Frontend session — `TeamOption.role` + `isTeamManager()`

**Files:**
- Modify: `frontend/src/auth/session.ts`
- Create: `frontend/src/auth/session.test.ts`

Context: `readTeams(who)` (in `session.ts`) maps `who.teams` items to `TeamOption { id, name, isPrimary }` — it drops the in-team role. `storeLogin(sessionId, who)` sets `identity = who` and calls `setInitialTeamContext(who)` → `teamOptions = readTeams(who)`, so a test can inject identity + teams via `storeLogin`. `session.isAdmin()` already exists (`identity?.role === 'admin'`). The `TeamOption` interface is near the top of `session.ts` (`{ id: string; name: string; isPrimary: boolean }`). The `session` export object ends near line 183.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/auth/session.test.ts`:
```ts
import { beforeEach, describe, expect, it } from 'vitest'
import { session } from './session'

describe('session.isTeamManager', () => {
  beforeEach(() => {
    session.clear()
  })

  it('is true for a global agent who is a team supervisor', () => {
    session.storeLogin('s1', {
      id: 'u1',
      role: 'agent',
      teams: [{ teamId: 1, roleInTeam: 'supervisor', isPrimary: true }],
    })
    expect(session.isTeamManager()).toBe(true)
  })

  it('is true for a global agent who is a team lead', () => {
    session.storeLogin('s1', {
      id: 'u1',
      role: 'agent',
      teams: [{ teamId: 1, roleInTeam: 'lead', isPrimary: true }],
    })
    expect(session.isTeamManager()).toBe(true)
  })

  it('is false for a global agent who is only a plain member', () => {
    session.storeLogin('s1', {
      id: 'u1',
      role: 'agent',
      teams: [{ teamId: 1, roleInTeam: 'member', isPrimary: true }],
    })
    expect(session.isTeamManager()).toBe(false)
  })

  it('is true for a global admin with no teams', () => {
    session.storeLogin('s1', { id: 'u1', role: 'admin', teams: [] })
    expect(session.isTeamManager()).toBe(true)
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/auth/session.test.ts 2>&1 | tail -15`
Expected: FAIL — `session.isTeamManager is not a function`.

- [ ] **Step 3: Capture the in-team role in `TeamOption` + `readTeams`**

In `frontend/src/auth/session.ts`:
- Extend the `TeamOption` interface:
```ts
export interface TeamOption {
  id: string
  name: string
  isPrimary: boolean
  role?: string // in-team role: member/lead/supervisor (absent on the primary-only fallback)
}
```
- In `readTeams`, add `role` to the pushed option (inside the `for (const item of raw)` loop, where the object with `id`/`name`/`isPrimary` is built):
```ts
    options.push({
      id,
      name: String(item.name ?? item.teamName ?? `Team ${id}`),
      isPrimary: item.isPrimary === true || item.primary === true,
      role: typeof item.roleInTeam === 'string' ? item.roleInTeam : undefined,
    })
```
(Leave the primary-only fallback entry — built from `who.teamId` — without a `role`; it stays `undefined`.)

- [ ] **Step 4: Add `isTeamManager()` to the `session` export**

In the `session` export object in `frontend/src/auth/session.ts`, add this method next to `isAdmin` / `position`:
```ts
  isTeamManager: () =>
    identity?.role === 'admin' ||
    teamOptions.some((t) => t.role === 'lead' || t.role === 'supervisor'),
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/auth/session.test.ts 2>&1 | tail -12`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/auth/session.ts frontend/src/auth/session.test.ts
git commit -m "feat(session): capture in-team role + add session.isTeamManager()"
```

---

## Task 3: Frontend nav — move 團隊 to 日常 behind a `show` predicate

**Files:**
- Modify: `frontend/src/components/AppShell.tsx`
- Create: `frontend/src/components/AppShell.test.tsx`

Context: `NavItem` is `{ to, label, area, badge? }`. `NAV_GROUPS` (module const, ~line 53) has the 日常 group first and 營運管理 group containing `{ to: '/teams', label: '團隊', area: 'ops' }`. `NavGroups` is a local (non-exported) function (~line 122) whose render filters `group.items.filter((i) => can(pos, i.area))` (~line 136) and renders `<Link>` from react-router-dom. `session` is already imported in this file.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/AppShell.test.tsx`:
```tsx
import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

// Use the REAL permissions (can) so ops-area gating is realistic; only stub the
// session's isTeamManager per test. Stub stores/teams so importing AppShell has no
// side effects.
const sessionMock = vi.hoisted(() => ({
  position: vi.fn(() => 'agent'),
  isTeamManager: vi.fn(() => false),
}))
vi.mock('../auth/session', () => ({ session: sessionMock }))
// NavGroups never reads teamsStore (only the AppShell default component does), so a
// stub that just satisfies the module import is enough.
vi.mock('../stores/teams', () => ({ loadTeams: vi.fn(), teamsStore: {} }))

import { NavGroups } from './AppShell'

function renderNav() {
  return render(
    <MemoryRouter>
      <NavGroups pathname="/" pos="agent" unread={0} />
    </MemoryRouter>,
  )
}

describe('團隊 nav visibility', () => {
  beforeEach(() => vi.clearAllMocks())

  it('shows 團隊 for a global agent who is an in-team manager', () => {
    sessionMock.isTeamManager.mockReturnValue(true)
    renderNav()
    expect(screen.getByText('團隊')).toBeTruthy()
  })

  it('hides 團隊 for a plain member (agent, not a manager)', () => {
    sessionMock.isTeamManager.mockReturnValue(false)
    renderNav()
    expect(screen.queryByText('團隊')).toBeNull()
  })
})
```
(`AppShell.tsx` imports exactly `{ loadTeams, teamsStore }` from `../stores/teams` — the stub above matches. The two hard assertions that MUST hold: 團隊 present when `isTeamManager()` is true, absent when false, for `pos="agent"`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/AppShell.test.tsx 2>&1 | tail -20`
Expected: FAIL — `NavGroups` is not exported (import error), or 團隊 present in both cases (still area `ops`, no `show`).

- [ ] **Step 3: Add `show?` to `NavItem` and export `NavGroups`**

In `frontend/src/components/AppShell.tsx`:
- Extend the interface:
```ts
interface NavItem {
  to: string
  label: string
  area: Area
  badge?: 'unread'
  show?: () => boolean
}
```
- Export the `NavGroups` function (add `export` to its declaration):
```ts
export function NavGroups({
```

- [ ] **Step 4: Move the 團隊 item into the 日常 group with a `show` predicate**

In `NAV_GROUPS` in `frontend/src/components/AppShell.tsx`:
- Remove `{ to: '/teams', label: '團隊', area: 'ops' }` from the 營運管理 group's `items`.
- Add it to the **日常** group's `items` (append after the existing daily items, before the group closes):
```ts
      { to: '/teams', label: '團隊', area: 'daily', show: () => can(session.position(), 'ops') || session.isTeamManager() },
```
- Update the filter (~line 136) to honor `show`:
```ts
        const visible = group.items.filter((i) => can(pos, i.area) && (i.show ? i.show() : true))
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/components/AppShell.test.tsx 2>&1 | tail -12`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/components/AppShell.tsx frontend/src/components/AppShell.test.tsx
git commit -m "feat(nav): 團隊 moves to 日常, visible to in-team managers (show predicate)"
```

---

## Task 4: Frontend Teams page — relax gate + hide admin-only controls

**Files:**
- Modify: `frontend/src/pages/Teams.tsx`
- Modify: `frontend/src/pages/Teams.test.tsx`

Context (current `Teams.tsx` lines): gate at 203 `if (!can(session.position(), 'ops'))`; create form at ~259 `<form onSubmit={create} ...>` inside a `<Card>`; the 刪除團隊… button at ~307; the status toggle at ~245 (`<button onClick={() => void toggleActive(m)}><StatusPill .../></button>`). `session.isAdmin()` and (after Task 2) `session.isTeamManager()` exist. The existing `Teams.test.tsx` mocks `'../auth/session'` as `{ session: { position: () => 'system_admin' } }` and `'../auth/permissions'` as `{ can: () => true }`, and has 4 passing tests.

- [ ] **Step 1: Update the session mock to be overridable + write the failing test**

In `frontend/src/pages/Teams.test.tsx`, replace the static session mock:
```tsx
vi.mock('../auth/session', () => ({
  session: { position: () => 'system_admin' },
}))
```
with a `vi.fn`-based, hoisted mock that defaults to admin:
```tsx
const sessionMock = vi.hoisted(() => ({
  position: vi.fn(() => 'system_admin'),
  isAdmin: vi.fn(() => true),
  isTeamManager: vi.fn(() => true),
}))
vi.mock('../auth/session', () => ({ session: sessionMock }))
```
In the existing `beforeEach`, reset them to the admin defaults so prior tests are unaffected:
```tsx
    sessionMock.position.mockReturnValue('system_admin')
    sessionMock.isAdmin.mockReturnValue(true)
    sessionMock.isTeamManager.mockReturnValue(true)
```
Then add a new test (at the end of the `describe`) for a non-admin manager:
```tsx
  it('hides admin-only controls for a non-admin team manager', async () => {
    sessionMock.position.mockReturnValue('agent')
    sessionMock.isAdmin.mockReturnValue(false)
    sessionMock.isTeamManager.mockReturnValue(true)

    render(<Teams />)
    fireEvent.click(await screen.findByText('客服一隊'))
    await waitFor(() => expect(apiMock.get).toHaveBeenCalledWith('/api/teams/1/members'))

    // Manager controls remain.
    expect(await screen.findByLabelText('新增成員')).toBeTruthy()
    expect(screen.getByDisplayValue('成員')).toBeTruthy() // role dropdown

    // Admin-only controls are gone.
    expect(screen.queryByRole('button', { name: '建立' })).toBeNull()
    expect(screen.queryByRole('button', { name: '刪除團隊…' })).toBeNull()
    // Status is a read-only pill, not a toggle button.
    expect(screen.queryByRole('button', { name: /啟用|停用/ })).toBeNull()
  })
```
(`screen` is already imported in this file. The 移出團隊/加入團隊/role-dropdown tests already exist and must keep passing with the admin defaults.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/pages/Teams.test.tsx 2>&1 | tail -20`
Expected: FAIL — the create 建立 button, 刪除團隊… button, and the status toggle button are still rendered for the non-admin manager.

- [ ] **Step 3: Relax the page gate**

In `frontend/src/pages/Teams.tsx`, change the gate (line ~203):
```tsx
  if (!(can(session.position(), 'ops') || session.isTeamManager())) {
```
(keep the existing 權限不足 body unchanged.)

- [ ] **Step 4: Compute `isAdmin` and hide admin-only controls**

In `frontend/src/pages/Teams.tsx`, add near the other derived values (just after the gate, before `const memberIds = ...`):
```tsx
  const isAdmin = session.isAdmin()
```
Then:

(a) Wrap the create-team `<Card>`/form so it renders only for admins. The block is:
```tsx
      <Card style={{ marginBottom: 'var(--sp-4)' }}>
        <form onSubmit={create} style={{ display: 'flex', gap: 8 }}>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="新團隊名稱" />
          <button type="submit">建立</button>
        </form>
      </Card>
```
Change it to:
```tsx
      {isAdmin && (
        <Card style={{ marginBottom: 'var(--sp-4)' }}>
          <form onSubmit={create} style={{ display: 'flex', gap: 8 }}>
            <input value={name} onChange={(e) => setName(e.target.value)} placeholder="新團隊名稱" />
            <button type="submit">建立</button>
          </form>
        </Card>
      )}
```

(b) Gate the 刪除團隊… button (in the open-team panel header). The block is:
```tsx
                <button
                  onClick={() => setConfirmDeleteTeam(true)}
                  style={{ color: 'crimson', marginLeft: picked.size > 0 ? 0 : 'auto' }}
                >
                  刪除團隊…
                </button>
```
Wrap it:
```tsx
                {isAdmin && (
                  <button
                    onClick={() => setConfirmDeleteTeam(true)}
                    style={{ color: 'crimson', marginLeft: picked.size > 0 ? 0 : 'auto' }}
                  >
                    刪除團隊…
                  </button>
                )}
```

(c) Make the status column read-only for non-admins (`set_member_status` is `require_admin`). The `isActive` column `render` is:
```tsx
      render: (m) => (
        <button onClick={() => void toggleActive(m)}>
          <StatusPill status={m.isActive ? 'active' : 'inactive'} label={m.isActive ? '啟用' : '停用'} />
        </button>
      ),
```
Change it to:
```tsx
      render: (m) =>
        isAdmin ? (
          <button onClick={() => void toggleActive(m)}>
            <StatusPill status={m.isActive ? 'active' : 'inactive'} label={m.isActive ? '啟用' : '停用'} />
          </button>
        ) : (
          <StatusPill status={m.isActive ? 'active' : 'inactive'} label={m.isActive ? '啟用' : '停用'} />
        ),
```
(`memberColumns` is computed inside the component body, so `isAdmin` is in scope.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/pages/Teams.test.tsx 2>&1 | tail -15`
Expected: PASS (5 tests — the 4 existing admin tests + the new non-admin test).

- [ ] **Step 6: Build + full frontend suite**

Run:
- `cd frontend && npm run build 2>&1 | tail -4` → `tsc -b` clean + vite success.
- `cd frontend && npx vitest run 2>&1 | tail -8` → green.

- [ ] **Step 7: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/pages/Teams.tsx frontend/src/pages/Teams.test.tsx
git commit -m "feat(teams): in-team managers can manage their team; admin-only controls hidden"
```

---

## Final verification

- [ ] `cd backend && cargo clippy --all-targets -- -D warnings && cargo test` — green.
- [ ] `cd frontend && npm run build && npx vitest run` — green; `package-lock.json` in sync.
- [ ] Run `detect_changes({scope: "compare", base_ref: "main"})` (GitNexus) and confirm only the expected symbols/processes are affected; no backend authorization symbol changed.
- [ ] Manual (live):
  - Global-`agent` who is 主管 of a team → 團隊 appears under 日常 → open → sees only their team; can change roles / 移出團隊 / 新增成員 / view QR; does NOT see 建立團隊 / 刪除團隊… / the 啟用-停用 toggle (status shows as a read-only pill).
  - Plain member (in-team `member`) → no 團隊 nav item; direct-navigating to `/teams` shows 權限不足.
  - Admin → unchanged (all visible, all usable).

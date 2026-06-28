# Appoint Team Manager (role options) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an admin appoint a team member as the team manager by fixing the `Teams.tsx` member-role dropdown to the real team roles (`member`/`lead`/`supervisor`).

**Architecture:** Frontend-only one-liner-ish fix: the backend already validates `PUT /api/teams/members/{id}/role` against `TEAM_ROLES = ["member","lead","supervisor"]` (in `teams/handlers/*`); the page currently sends the wrong global roles (`agent`/`admin`). Replace the dropdown options + default.

**Tech Stack:** React 18 + TypeScript + Vite; vitest.

**Verification gate (this repo's CI):** `npm run build` + `npx vitest run` green; `package-lock.json` in sync.

---

## File Structure

- `frontend/src/pages/Teams.tsx` — **modify**: `ROLE_OPTIONS` → team roles; role `<select>` default fallback.
- `frontend/src/pages/Teams.test.tsx` — **create**: role-options + appoint-supervisor test.

---

## Task 1: Team-role options on the Teams page

**Files:**
- Modify: `frontend/src/pages/Teams.tsx`
- Create: `frontend/src/pages/Teams.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/pages/Teams.test.tsx`. Render the page with one team + one member, then change the member's role select to 主管 and assert `put` was called with the supervisor role. Mock `'../api/client'` (`get` resolves the data the page loads, `post`/`put` are `vi.fn().mockResolvedValue({ success: true })`), and mock the admin gate the same way an existing admin-page test does (grep for how `Channels.test.tsx` / another page test mocks `'../auth/permissions'` `can` and `'../auth/session'` — mirror it so the page renders past its gate). Read `Teams.tsx` first to supply the exact props/data shape its `get` calls expect (teams list endpoint + members), and how it picks the selected team (it may auto-select the first team or need a click).

```tsx
import { render, fireEvent, waitFor, within } from '@testing-library/react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import * as api from '../api/client'
import Teams from './Teams'

vi.mock('../api/client', () => ({
  get: vi.fn(),
  post: vi.fn().mockResolvedValue({ success: true }),
  put: vi.fn().mockResolvedValue({ success: true }),
}))
// Mirror the admin-gate mocks used by other admin-page tests (adjust to the real API):
vi.mock('../auth/permissions', () => ({ can: () => true }))
vi.mock('../auth/session', () => ({ session: { position: () => 'system', identity: () => ({ role: 'admin' }) } }))

describe('Teams member role', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    // Make every GET the page issues resolve with sensible data. Adapt the shapes
    // to what Teams.tsx reads (teams array; a team's members with id+role).
    ;(api.get as ReturnType<typeof vi.fn>).mockImplementation((url: string) => {
      if (url.includes('/members')) return Promise.resolve({ success: true, data: [{ id: 'm1', displayName: 'Alice', role: 'member' }] })
      return Promise.resolve({ success: true, data: [{ id: 1, name: 'Team A', memberCount: 1 }] })
    })
  })

  it('offers team roles and appoints a supervisor', async () => {
    const { findByRole, findByText } = render(<Teams />)
    // select the team if needed (adapt: the page may auto-load the first team's members)
    // ...
    const select = await findByRole('combobox') // the member role <select>
    // the three option labels exist
    expect(within(select).getByText('成員')).toBeTruthy()
    expect(within(select).getByText('組長')).toBeTruthy()
    expect(within(select).getByText('主管（團隊管理員）')).toBeTruthy()
    fireEvent.change(select, { target: { value: 'supervisor' } })
    await waitFor(() =>
      expect(api.put).toHaveBeenCalledWith('/api/teams/members/m1/role', { role: 'supervisor' }),
    )
  })
})
```
(This test is a guide — adapt the `get` mock shapes, the team-selection step, and the `findByRole('combobox')` query to the actual `Teams.tsx` structure you read. The two hard assertions that MUST hold: the three labels 成員/組長/主管（團隊管理員） are present, and selecting 主管 calls `put('/api/teams/members/m1/role', { role: 'supervisor' })`.)
Run `cd frontend && npx vitest run src/pages/Teams.test.tsx 2>&1 | tail -15` → FAIL (options are 客服/管理員; selecting sends the wrong value).

- [ ] **Step 2: Fix `ROLE_OPTIONS` + the select default**

In `frontend/src/pages/Teams.tsx`:
- Replace:
```tsx
const ROLE_OPTIONS = [
  { value: 'agent', label: '客服' },
  { value: 'admin', label: '管理員' },
]
```
with:
```tsx
const ROLE_OPTIONS = [
  { value: 'member', label: '成員' },
  { value: 'lead', label: '組長' },
  { value: 'supervisor', label: '主管（團隊管理員）' },
]
```
- In the member-table role column, change the `<select value={m.role ?? 'agent'}` fallback to `'member'`:
```tsx
        <select
          value={m.role ?? 'member'}
          onChange={(e) => void changeRole(m.id, e.target.value)}
          ...
```
No other change (the `changeRole(memberId, role)` → `put('/api/teams/members/{id}/role', { role })` flow already sends whatever option value is selected, now a valid `TEAM_ROLES` value).

- [ ] **Step 3: Test passes**

Run `cd frontend && npx vitest run src/pages/Teams.test.tsx 2>&1 | tail -10` → PASS.

- [ ] **Step 4: Build + full suite**

- `cd frontend && npm run build 2>&1 | tail -4` → `tsc -b` clean + vite success.
- `cd frontend && npx vitest run 2>&1 | tail -6` → green.

- [ ] **Step 5: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/pages/Teams.tsx frontend/src/pages/Teams.test.tsx
git commit -m "fix(teams): member role options use real team roles (member/lead/supervisor)"
```

---

## Final verification

- [ ] `cd frontend && npm run build && npx vitest run` — green.
- [ ] Read-only confirm (no change): `cd backend && grep -rn "TEAM_ROLES.contains" src/domain/teams/handlers` — the `PUT /members/{id}/role` handler (`set_member_role`) accepts `member`/`lead`/`supervisor` and would have rejected `admin`/`agent`, so the new options are valid.
- [ ] Manual: 團隊管理 → a team → change a member to **主管（團隊管理員）** → saved (200); that member now holds `supervisor` rank and can manage their team (existing backend gate).

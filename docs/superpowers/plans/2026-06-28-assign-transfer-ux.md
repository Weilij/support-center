# Assign / Transfer UX Clarification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the confusing 指派 + 轉接 conversation actions into one self-routing "指派團隊" dialog, add a one-click "指給我的團隊" quick action, and make the inbox team tab mean "my team's conversations".

**Architecture:** Frontend only — reuse the existing `assignConversation`/`transferConversation`/`unassignConversation` store helpers (and their backend endpoints). The `AssignDialog` becomes self-determining when no `mode` is passed (keeping the explicit-`mode` path for the legacy `ConversationDetail` page). The inbox `Thread`/`ThreadHeader` expose a single team action; `MessageComposer` gets a quick-claim; `ConversationList` filters the team tab by the agent's teams.

**Tech Stack:** React 18 + TypeScript + Vite; vitest.

**Verification gate (this repo's CI):** every task ends green on `npm run build` + `npx vitest run` (keep `package-lock.json` in sync).

---

## File Structure

- `frontend/src/components/ConversationAssign.tsx` — **modify**: make `mode` optional; when omitted, render a self-routing "指派團隊" dialog (assign / transfer / unassign).
- `frontend/src/components/ConversationAssign.test.tsx` — **create**: routing tests.
- `frontend/src/pages/inbox/ThreadHeader.tsx` — **modify**: one "指派團隊" button (remove the separate transfer trigger).
- `frontend/src/pages/inbox/Thread.tsx` — **modify**: single `assignOpen` state; quick-claim handler.
- `frontend/src/pages/inbox/MessageComposer.tsx` — **modify**: the team button becomes "指給我的團隊" → calls `onQuickAssign`.
- `frontend/src/pages/inbox/ConversationList.tsx` — **modify**: team tab → "我的團隊", filter by the agent's team ids.

---

## Task 1: Unified self-routing "指派團隊" dialog + single header button

**Files:**
- Modify: `frontend/src/components/ConversationAssign.tsx`
- Create: `frontend/src/components/ConversationAssign.test.tsx`
- Modify: `frontend/src/pages/inbox/ThreadHeader.tsx`
- Modify: `frontend/src/pages/inbox/Thread.tsx`

- [ ] **Step 1: Write the failing dialog tests**

Create `frontend/src/components/ConversationAssign.test.tsx`. Mock `'../stores/conversations'` (`assignConversation`/`transferConversation`/`unassignConversation` = `vi.fn().mockResolvedValue(true)`) and `'../stores/teams'` (`teamsStore` with items `[{id:5,name:'A'},{id:7,name:'B'}]`, `loadTeams` a no-op). Render `<AssignDialog>` WITHOUT a `mode` (unified):
```tsx
import { render, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import * as convs from '../stores/conversations'
import { AssignDialog } from './ConversationAssign'

vi.mock('../stores/conversations', () => ({
  assignConversation: vi.fn().mockResolvedValue(true),
  transferConversation: vi.fn().mockResolvedValue(true),
  unassignConversation: vi.fn().mockResolvedValue(true),
}))
vi.mock('../stores/teams', () => ({
  teamsStore: { get: () => ({ items: [{ id: 5, name: 'A' }, { id: 7, name: 'B' }] }), subscribe: () => () => {} },
  loadTeams: vi.fn(),
}))

describe('AssignDialog (unified)', () => {
  beforeEach(() => vi.clearAllMocks())

  it('assigns when there is no current team', async () => {
    const { getByRole, getByLabelText } = render(
      <AssignDialog open conversationId="c1" currentTeamId={null} onClose={() => {}} />,
    )
    fireEvent.change(getByLabelText(/團隊/), { target: { value: '7' } })
    fireEvent.click(getByRole('button', { name: /確認|確定/ }))
    await waitFor(() => expect(convs.assignConversation).toHaveBeenCalledWith('c1', 7, undefined))
    expect(convs.transferConversation).not.toHaveBeenCalled()
  })

  it('transfers when a current team exists', async () => {
    const { getByRole, getByLabelText } = render(
      <AssignDialog open conversationId="c1" currentTeamId={5} onClose={() => {}} />,
    )
    fireEvent.change(getByLabelText(/團隊/), { target: { value: '7' } })
    fireEvent.click(getByRole('button', { name: /確認|確定/ }))
    await waitFor(() => expect(convs.transferConversation).toHaveBeenCalledWith('c1', 7, 5, undefined))
  })

  it('unassigns via the 取消指派 action', async () => {
    const { getByRole } = render(
      <AssignDialog open conversationId="c1" currentTeamId={5} onClose={() => {}} />,
    )
    fireEvent.click(getByRole('button', { name: /取消指派/ }))
    await waitFor(() => expect(convs.unassignConversation).toHaveBeenCalledWith('c1', undefined))
  })
})
```
(If the project's `teamsStore`/`useStore` mock shape differs, mirror an existing test that mocks a store — grep `vi.mock('../stores/` in `*.test.tsx`. Adapt the teams mock so `useStore(teamsStore)` yields `{ items: [...] }`.)
Run `cd frontend && npx vitest run src/components/ConversationAssign.test.tsx 2>&1 | tail -15` → FAIL.

- [ ] **Step 2: Make the dialog self-routing when `mode` is omitted**

In `frontend/src/components/ConversationAssign.tsx`:
- Change the prop type to `mode?: AssignMode` (optional). Keep `AssignMode` exported (the legacy `ConversationDetail` page still passes it).
- Compute an effective behavior: `const unified = mode === undefined`. When `unified`, the dialog self-routes; otherwise keep the exact existing `mode`-based behavior.
- Title: `unified ? '指派團隊' : TITLES[mode]`.
- Team `<Select>`: render it when `unified || mode !== 'unassign'`. Its label = `unified ? '指派團隊' : (mode === 'transfer' ? '轉接至團隊' : '指派團隊')`. Its options exclude the current team when `unified ? currentTeamId != null : mode === 'transfer'`:
```tsx
  const excludeCurrent = unified ? currentTeamId != null : mode === 'transfer'
  const teamOptions = teams
    .filter((t) => !excludeCurrent || t.id !== currentTeamId)
    .map((t) => ({ value: t.id, label: t.name }))
```
- Add a "目前團隊" line above the select when `unified`: show the current team name (look it up in `teams` by `currentTeamId`) or "未指派".
- Replace `submit` so the unified path routes by `currentTeamId`:
```tsx
  const submit = async () => {
    const effectiveUnassign = !unified && mode === 'unassign'
    if (!effectiveUnassign && !teamId) { setError('請選擇團隊'); return }
    setBusy(true)
    let ok = false
    if (effectiveUnassign) {
      ok = await unassignConversation(conversationId, reason || undefined)
    } else if (unified) {
      ok = currentTeamId == null
        ? await assignConversation(conversationId, Number(teamId), reason || undefined)
        : await transferConversation(conversationId, Number(teamId), currentTeamId, reason || undefined)
    } else if (mode === 'assign') {
      ok = await assignConversation(conversationId, Number(teamId), reason || undefined)
    } else {
      ok = await transferConversation(conversationId, Number(teamId), currentTeamId, reason || undefined)
    }
    setBusy(false)
    if (ok) { onDone?.(true); onClose() } else { setError('操作失敗，請重試') }
  }
```
- Add a secondary "取消指派" action shown only when `unified && currentTeamId != null`, calling a small `doUnassign`:
```tsx
  const doUnassign = async () => {
    setBusy(true)
    const ok = await unassignConversation(conversationId, reason || undefined)
    setBusy(false)
    if (ok) { onDone?.(true); onClose() } else { setError('操作失敗，請重試') }
  }
```
Render it in the footer (left-aligned) when `unified && currentTeamId != null`:
```tsx
        {unified && currentTeamId != null && (
          <button onClick={() => void doUnassign()} disabled={busy} style={{ marginRight: 'auto', color: 'crimson' }}>
            取消指派
          </button>
        )}
```
Run `cd frontend && npx vitest run src/components/ConversationAssign.test.tsx 2>&1 | tail -10` → PASS (3/3).

- [ ] **Step 3: One "指派團隊" header button**

In `frontend/src/pages/inbox/ThreadHeader.tsx`, the header currently has two buttons wired to `onAssign` (指派) and `onTransfer` (轉接). Collapse to ONE button labeled "指派團隊" (keep the `users` icon), wired to a single `onAssignTeam` prop. Update the component prop types: remove `onTransfer`, rename `onAssign` → `onAssignTeam` (or keep `onAssign` and delete the transfer button — pick the smaller diff; the button's `aria-label`/`title` becomes "指派團隊"). Remove the now-unused transfer button JSX.

- [ ] **Step 4: Single open-state in `Thread.tsx`**

In `frontend/src/pages/inbox/Thread.tsx`:
- Replace `const [assignMode, setAssignMode] = useState<AssignMode | null>(null)` with `const [assignOpen, setAssignOpen] = useState(false)`.
- The `ThreadHeader` `onAssign`/`onTransfer` props → a single `onAssignTeam={() => setAssignOpen(true)}` (match the prop name you chose in Step 3).
- The dialog render becomes:
```tsx
      {convId && assignOpen && (
        <AssignDialog
          open
          conversationId={convId}
          currentTeamId={currentTeamId}
          onClose={() => setAssignOpen(false)}
          onDone={() => onMetaReload?.()}
        />
      )}
```
Do NOT pass `mode` (unified). If there is no existing `onMetaReload`/meta-refresh callback, drop the `onDone` line (the store helpers update the conversation list optimistically, so `currentTeamId` derived from the store refreshes on its own) — just `onClose`. Leave the `import { AssignDialog, type AssignMode }` as `import { AssignDialog } from '...'` if `AssignMode` is no longer referenced in this file (remove the unused type import to satisfy tsc/lint).

- [ ] **Step 5: Build + suite**

- `cd frontend && npm run build 2>&1 | tail -5` → `tsc -b` clean + vite success.
- `cd frontend && npx vitest run 2>&1 | tail -6` → green (incl. the 3 dialog tests).

- [ ] **Step 6: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/components/ConversationAssign.tsx frontend/src/components/ConversationAssign.test.tsx frontend/src/pages/inbox/ThreadHeader.tsx frontend/src/pages/inbox/Thread.tsx
git commit -m "feat(inbox): unified 指派團隊 dialog (self-routes assign/transfer/unassign), single header button"
```

---

## Task 2: "指給我的團隊" one-click quick action

**Files:**
- Modify: `frontend/src/pages/inbox/MessageComposer.tsx`
- Modify: `frontend/src/pages/inbox/Thread.tsx`

- [ ] **Step 1: Quick-assign handler in `Thread.tsx`**

`session` exposes the agent's teams: `session.currentTeam()` (`{id,name,isPrimary} | null`, id is a string) and `session.teamOptions()`. Add a handler in `Thread.tsx` that assigns the conversation to the agent's current/primary team in one click:
```tsx
  const quickAssignToMyTeam = async () => {
    if (!convId) return
    const myTeam = session.currentTeam() ?? session.teamOptions().find((t) => t.isPrimary) ?? session.teamOptions()[0]
    const teamNum = myTeam ? Number(myTeam.id) : NaN
    if (!Number.isFinite(teamNum)) { setToast?.('你沒有所屬團隊，無法指派'); return }
    const ok = await assignConversation(convId, teamNum)
    setToast?.(ok ? `已指派給「${myTeam!.name}」` : '指派失敗')
  }
```
Ensure `session` (from `'../../auth/session'`) and `assignConversation` (from `'../../stores/conversations'`) are imported in `Thread.tsx`; `setToast` should already exist in this component (it powers upload toasts) — reuse it (adapt the name if different).

- [ ] **Step 2: Wire the composer button to the quick action**

`MessageComposer.tsx` has an `onAssign: () => void` prop and a button currently labeled "指派至團隊" → `onClick={onAssign}`. Rename the prop to `onQuickAssign` (or keep `onAssign` and just change the label + handler target), set the button label to **"指給我的團隊"**, and have `Thread.tsx` pass `onQuickAssign={() => void quickAssignToMyTeam()}` (the composer button no longer opens the dialog — the header's "指派團隊" button covers the full dialog). If the agent has no team, the handler shows a toast (Step 1) — no need to hide the button.

- [ ] **Step 3: Build + suite**

- `cd frontend && npm run build 2>&1 | tail -5` → clean.
- `cd frontend && npx vitest run 2>&1 | tail -6` → green.

- [ ] **Step 4: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/pages/inbox/MessageComposer.tsx frontend/src/pages/inbox/Thread.tsx
git commit -m "feat(inbox): composer 指給我的團隊 one-click quick assign"
```

---

## Task 3: "我的團隊" tab filtered by the agent's teams

**Files:**
- Modify: `frontend/src/pages/inbox/ConversationList.tsx`
- Test: `frontend/src/pages/inbox/ConversationList.test.tsx` (create or extend)

- [ ] **Step 1: Failing test**

Create/extend `frontend/src/pages/inbox/ConversationList.test.tsx`. Mock `'../../auth/session'` so `session.teamOptions()` returns `[{ id: '5', name: 'A', isPrimary: true }]` and `session.isAdmin()` returns `false`. Render `<ConversationList>` with conversations `[{id:'a', teamId:5, customerName:'X'}, {id:'b', teamId:9, customerName:'Y'}, {id:'c', teamId:null, customerName:'Z'}]` (supply the props the component needs — read its signature). Click the "我的團隊" tab and assert only conversation `a` (teamId 5 ∈ my teams) is shown; `b` and `c` are not. Mirror an existing inbox test's render/props setup. Run `cd frontend && npx vitest run src/pages/inbox/ConversationList.test.tsx 2>&1 | tail -12` → FAIL.

- [ ] **Step 2: Relabel + re-filter the tab**

In `frontend/src/pages/inbox/ConversationList.tsx`:
- In the `TABS` array, change `{ key: 'team', label: '團隊' }` → `{ key: 'team', label: '我的團隊' }`.
- Import `session`: `import { session } from '../../auth/session'`.
- Replace `hasTeamAssignment` with a "my team" predicate (keep the old helper name's call site or rename — update the filter at the `tab === 'team'` line):
```tsx
function isMyTeam(c: Conversation): boolean {
  // Admins (or users with no team) keep the broad "has a team" behavior so the
  // tab is never empty for them.
  const myIds = session.teamOptions().map((t) => String(t.id))
  if (session.isAdmin() || myIds.length === 0) {
    const assignedTeam = c['assignedTeam']
    return c.teamId != null || (assignedTeam !== null && assignedTeam !== undefined)
  }
  return c.teamId != null && myIds.includes(String(c.teamId))
}
```
- Update the filter line `if (tab === 'team' && !hasTeamAssignment(conversation)) return false` to call `isMyTeam(conversation)`. Remove the now-unused `hasTeamAssignment` if nothing else references it (grep first).

- [ ] **Step 3: Test passes + build + suite**

- `cd frontend && npx vitest run src/pages/inbox/ConversationList.test.tsx 2>&1 | tail -10` → PASS.
- `cd frontend && npm run build 2>&1 | tail -5` → clean.
- `cd frontend && npx vitest run 2>&1 | tail -6` → green.

- [ ] **Step 4: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/pages/inbox/ConversationList.tsx frontend/src/pages/inbox/ConversationList.test.tsx
git commit -m "feat(inbox): 我的團隊 tab filters by the agent's teams"
```

---

## Final verification (after all tasks)

- [ ] `cd frontend && npm run build && npx vitest run` — green; `npm ci` in sync.
- [ ] Legacy `ConversationDetail` page (which still passes `mode` to `AssignDialog`) is unbroken — grep `mode=` on `AssignDialog` usages and confirm that call still compiles + behaves as before.
- [ ] `detect_changes()` is N/A here (frontend-only, GitNexus indexes are stale for the merged tree). Rely on build + vitest.
- [ ] Manual: in the inbox, the thread header shows ONE "指派團隊" button → dialog shows current team, pick a team to assign/transfer, 取消指派 to clear. The composer "指給我的團隊" one-click assigns to my team (toast). The "我的團隊" tab lists only my team's conversations.

# Add Existing Member to a Team Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an admin add an existing agent to a team from the Teams page (agent picker → `POST /api/teams/{id}/members {agentId}` → reload members).

**Architecture:** Frontend-only, inline in `Teams.tsx` (which already does its own `get`/`post`/`put` + local state, no team store for these). Load the agent roster via `stores/agents.loadAgents`, render a `<select>` of agents not already in the open team, and POST to the existing `add_member` endpoint.

**Tech Stack:** React 18 + TypeScript + Vite; vitest.

**Verification gate (this repo's CI):** `npm run build` + `npx vitest run` green; `package-lock.json` in sync.

---

## File Structure

- `frontend/src/pages/Teams.tsx` — **modify**: agent-roster state + "新增成員" picker in the selected-team panel + add handler.
- `frontend/src/pages/Teams.test.tsx` — **modify**: add-member picker + POST test.

---

## Task 1: "新增成員" picker on the Teams page

**Files:**
- Modify: `frontend/src/pages/Teams.tsx`
- Modify: `frontend/src/pages/Teams.test.tsx`

- [ ] **Step 1: Write the failing test**

Extend `frontend/src/pages/Teams.test.tsx` (created in the team-role task). Mock the agent roster via `'../stores/agents'` and assert that adding an agent POSTs to the team-members endpoint. Add to the existing mocks:
```tsx
vi.mock('../stores/agents', () => ({
  loadAgents: vi.fn().mockResolvedValue({
    items: [
      { id: 'm1', displayName: 'Alice' },
      { id: 'a2', displayName: 'Bob', email: 'bob@x.com' },
    ],
    total: 2, page: 1,
  }),
}))
```
(If `AgentsPage`'s list field is named `agents` rather than `items`, use that key — grep `stores/agents.ts` for the returned shape and match it.)
Then add a test: open the team (whose members are `[{ id: 'm1', displayName: 'Alice', role: 'member' }]`), and assert:
1. the add-member `<select>` (query by an `aria-label="新增成員"` you add in Step 2) offers `a2`/Bob but NOT `m1`/Alice (already a member).
2. selecting `a2` and clicking 加入團隊 calls `post('/api/teams/1/members', { agentId: 'a2' })`.

```tsx
  it('adds an existing agent to the team', async () => {
    const { findByText, findByLabelText, getByRole } = render(<Teams />)
    fireEvent.click(await findByText('客服一隊')) // open team 1 (adapt to the seeded team name)
    const picker = await findByLabelText('新增成員')
    // Alice (m1) is already a member → not an option; Bob (a2) is
    expect(within(picker).queryByText('Alice')).toBeNull()
    expect(within(picker).getByText(/Bob/)).toBeTruthy()
    fireEvent.change(picker, { target: { value: 'a2' } })
    fireEvent.click(getByRole('button', { name: '加入團隊' }))
    await waitFor(() =>
      expect(api.post).toHaveBeenCalledWith('/api/teams/1/members', { agentId: 'a2' }),
    )
  })
```
(Reuse the existing test file's `get` mock that returns team `{id:1,name:'客服一隊'}` and its members `[{id:'m1',...}]`; adapt names to what that test already seeds. Ensure `within`, `findByLabelText`, `waitFor`, `fireEvent` are imported.)
Run `cd frontend && npx vitest run src/pages/Teams.test.tsx 2>&1 | tail -15` → FAIL.

- [ ] **Step 2: Implement the roster + picker in `Teams.tsx`**

In `frontend/src/pages/Teams.tsx`:
- Import the roster loader + type:
```ts
import { loadAgents, type Agent } from '../stores/agents'
```
- Add state near the other `useState`s:
```ts
  const [allAgents, setAllAgents] = useState<Agent[]>([])
  const [addAgentId, setAddAgentId] = useState('')
  const [addBusy, setAddBusy] = useState(false)
```
- Load the roster once, alongside the initial teams load (in the existing mount `useEffect`, or a new one):
```ts
  useEffect(() => {
    void loadAgents(1, 200).then((page) => setAllAgents(page.items ?? []))
  }, [])
```
(Use the actual list field of `AgentsPage` — `page.items` or `page.agents`; match `stores/agents.ts`.)
- Add the add handler (mirror `openTeam`/`changeRole`'s inline `post` + reload style):
```ts
  const addMember = async () => {
    if (selected == null || !addAgentId) return
    setAddBusy(true)
    const resp = await post(`/api/teams/${selected}/members`, { agentId: addAgentId })
    setAddBusy(false)
    if (resp.success) {
      setAddAgentId('')
      setToast('已加入團隊')
      await openTeam(selected)
    } else {
      setError(resp.message ?? '加入失敗')
    }
  }
```
- Compute the candidate list (agents not already members) and render the control inside the selected-team panel, near the member `DataTable` (only when a team is `selected`):
```tsx
  const memberIds = new Set(members.map((m) => m.id))
  const candidateAgents = allAgents.filter((a) => !memberIds.has(a.id))
```
```tsx
        <div style={{ display: 'flex', gap: 8, alignItems: 'center', margin: '8px 0' }}>
          {candidateAgents.length === 0 ? (
            <span style={{ color: 'var(--muted)', fontSize: 13 }}>所有客服都已在此團隊</span>
          ) : (
            <>
              <select
                aria-label="新增成員"
                value={addAgentId}
                onChange={(e) => setAddAgentId(e.target.value)}
                style={{ padding: '4px 8px', borderRadius: 6, border: '1px solid #ccc' }}
              >
                <option value="">選擇要加入的客服…</option>
                {candidateAgents.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.displayName || a.email || a.id}
                  </option>
                ))}
              </select>
              <button type="button" onClick={() => void addMember()} disabled={!addAgentId || addBusy}>
                {addBusy ? '加入中…' : '加入團隊'}
              </button>
            </>
          )}
        </div>
```
Place this block where the selected team's members are rendered (find the JSX region that renders the member `DataTable` when `selected != null`; put the control just above or below it).

- [ ] **Step 3: Test passes**

Run `cd frontend && npx vitest run src/pages/Teams.test.tsx 2>&1 | tail -10` → PASS.

- [ ] **Step 4: Build + full suite**

- `cd frontend && npm run build 2>&1 | tail -4` → `tsc -b` clean + vite success.
- `cd frontend && npx vitest run 2>&1 | tail -6` → green.

- [ ] **Step 5: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/pages/Teams.tsx frontend/src/pages/Teams.test.tsx
git commit -m "feat(teams): add existing agent to a team from the Teams page"
```

---

## Final verification

- [ ] `cd frontend && npm run build && npx vitest run` — green.
- [ ] Read-only confirm (no change): `cd backend && sed -n '104,110p' src/domain/teams/mod.rs` — `POST /api/teams/{id}/members` → `add_member` exists and takes `{ agentId }` (inserts role `member`).
- [ ] Manual: 團隊管理 → open a team → the 新增成員 picker lists agents not in the team → pick one → 加入團隊 → they appear in the member list as 成員; the role dropdown can then promote them to 主管（團隊管理員）.

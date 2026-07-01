# Add Existing Member to a Team — Design Spec

**Date:** 2026-07-01
**Track:** team management UI (frontend only)
**Status:** design approved, pending written-spec review

---

## 0. Context

The team-management page (`frontend/src/pages/Teams.tsx`) lets an admin create a
team, edit a member's role, toggle member active state, bulk-delete members, and
show a join-QR — but there is **no way to add a member to a team**. So an admin who
created accounts on the Agents page cannot bring those agents into a team from the UI.

The backend already supports it: `POST /api/teams/{id}/members` (`add_member`) adds an
**existing** agent to a team (`{ agentId }`; inserts `team_members` with role `member`,
first team → primary). The agent roster is available at `GET /api/agents` (paginated;
the frontend has `loadAgents(page, limit)` in `stores/agents.ts` returning `Agent[]`).

**Decision (user):** add an "add existing agent to a team" control to the Teams page.
Account creation stays on the Agents page (no overlap). **Frontend only.**

---

## 1. Goal & non-goals

**Goal:** In the selected team's member panel, an admin picks an existing agent (one
not already in the team) and adds them; the member list refreshes to show them (as
role 成員).

**Non-goals:**
- **No backend change** (`add_member` + `GET /api/agents` already exist).
- **No account creation here** — that stays on the Agents page.
- No search box (a simple dropdown of agents-minus-current-members; a searchable
  picker is a later enhancement if the roster grows large).
- No role selection at add time — new members join as `member`; promote to
  組長/主管 via the existing role dropdown (the 2026-06-28 team-role change).

---

## 2. Frontend change (`Teams.tsx` + `stores/teams.ts`)

### 2.1 Store helper (`stores/teams.ts`)

Add:
```ts
export async function addTeamMember(teamId: number, agentId: string): Promise<{ ok: boolean; message?: string }>
```
It `post`s `/api/teams/${teamId}/members` with `{ agentId }` and returns `{ ok:
resp.success, message: resp.message }`. (Match the file's existing helper style /
return shape; if `stores/teams.ts` isn't the right home, put it inline in `Teams.tsx`.)

### 2.2 Agent roster for the picker

Load the agent roster once (when the page mounts or the first team opens) via the
existing `loadAgents` with a high limit (e.g. `loadAgents(1, 200)`) — enough for a
support-center roster — and keep it in component state as `allAgents: Agent[]`.

### 2.3 "新增成員" control in the selected-team panel

When a team is open (its member list is shown), render an add-member row:
- A `<select>` of **candidate agents** = `allAgents` filtered to exclude those whose id
  is already in the team's `members` (compare `agent.id` to member `id`). Each option
  shows the agent's display name (+ email if present).
- An **「加入團隊」** button, disabled while no candidate is selected or a request is in
  flight.
- On click: `addTeamMember(selectedTeamId, agentId)` → on `ok`, reload that team's
  members (reuse the existing `openTeam(teamId)` / member-refresh path) and clear the
  selection + show a success toast/message; on failure, show `message` via the page's
  existing error/message state.
- When the candidate list is empty (every agent is already in the team), show a muted
  note "所有客服都已在此團隊" and hide/disable the control.

---

## 3. Error handling

- Add failure (permission, already-member race, unknown agent) → the backend's message
  is shown via the existing `setError`/`setMessage` path; the member list is unchanged.
- The picker only lists valid non-member agents, so the common invalid cases are
  prevented client-side.

---

## 4. Testing (vitest)

- `Teams.test.tsx` (extend the existing file): with a team open whose members are
  `[{id:'m1'}]` and an agent roster `[{id:'m1',...},{id:'a2',displayName:'Bob'}]`, the
  add-member `<select>` offers only `a2` (m1 excluded); selecting `a2` and clicking
  加入團隊 calls `post('/api/teams/<teamId>/members', { agentId: 'a2' })` and triggers a
  member-list reload (assert the `GET .../members` fires again or the store helper is
  called). Mock `'../api/client'` + the admin gate as the existing Teams test does; mock
  `loadAgents` (or the `/api/agents` GET) to return the roster.

---

## 5. Verification

- `cd frontend && npm run build && npx vitest run` — green; `npm ci` in sync.
- Read-only confirm (no change): `POST /api/teams/{id}/members` (`add_member`) accepts
  `{ agentId }` and inserts role `member` — matches the new control.
- Manual: 團隊管理 → open a team → 新增成員 picker lists agents not in the team → pick
  one → 加入團隊 → they appear in the member list as 成員; then the role dropdown can
  promote them to 主管（團隊管理員）.

---

## 6. Resolved decisions

- Add **existing** agents to a team via `POST /api/teams/{id}/members { agentId }`;
  account creation stays on the Agents page.
- Picker = simple `<select>` of agents minus current members (no search for now).
- New members join as `member`; role changes use the existing dropdown.
- Frontend only; backend untouched.

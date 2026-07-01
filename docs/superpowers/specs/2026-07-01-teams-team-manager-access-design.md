# Team-Manager Access to the Teams Page — Design Spec

**Date:** 2026-07-01
**Track:** team management + access control (backend `/me` + frontend nav/page)
**Status:** design approved, pending written-spec review

---

## 0. Context

The Teams page (`frontend/src/pages/Teams.tsx`) sits in the **營運管理 (ops)** nav
group and is gated by `can(session.position(), 'ops')`. Positions map to areas
(`frontend/src/auth/permissions.ts`):

- `agent` → `['daily']`
- `supervisor` → `['daily','ops','analytics']`
- `system_admin` → `['daily','ops','analytics','system']`

So a person whose **global** position is `agent` (客服) cannot see or open the
Teams page **at all** — even if they are the **in-team 主管/組長** (supervisor/lead)
of their own team. That defeats the team-role system: the backend already lets a
team lead/supervisor manage their own team via `require_team_rank`, but the UI is
unreachable for them.

**User decision:** Move 團隊 into the **日常** nav group and let an in-team
**lead/supervisor** (even if global position `agent`) open the Teams page and
manage **their own** team. **Plain members** (in-team role `member`) still do not
see it. **Admin-only** operations stay admin-only and are **hidden** for
non-admins (so nobody sees a button that just 403s).

### Backend permission reality (verified)

| Operation | Endpoint | Gate | Team lead/supervisor? |
|---|---|---|---|
| List teams | `GET /api/teams` | non-admin → only their `primary_team_id` team | ✅ scoped already |
| Team members | `GET /api/teams/{id}/members` | (team-scoped) | ✅ |
| Change in-team role | `PUT /api/teams/agent-teams/{agentId}/role/{teamId}` | `require_team_rank(lead)` | ✅ |
| Remove from team | `DELETE /api/teams/agent-teams/{agentId}/leave/{teamId}` | `require_team_rank(lead)` | ✅ |
| Add member | `POST /api/teams/{id}/members` | `require_team_rank(lead)` | ✅ |
| QR latest / regenerate | `GET .../qr-code/latest` / `POST .../qr-code` | none / `require_team_rank(supervisor)` | ✅ (view) / supervisor only |
| **Create team** | `POST /api/teams` | `require_admin` | ❌ admin only |
| **Delete team** | `DELETE /api/teams/{id}` | `require_admin` | ❌ admin only |
| **Toggle active status** | `PUT /api/teams/members/{id}/status` | `require_admin` | ❌ admin only |

**Blocker:** the frontend cannot currently tell whether the user is a team
lead/supervisor. The in-team role lives only in the JWT access token (HttpOnly —
unreadable from JS). `/api/auth/me` (`agent_view`) and the login payload return
**no per-team role**. So the frontend must be given the caller's in-team roles.

---

## 1. Goal & non-goals

**Goal:** An in-team lead/supervisor (or admin) can reach the Teams page from the
日常 nav group and manage their own team; create/delete-team and status-toggle are
admin-only and hidden for non-admins; plain members and unaffiliated agents do not
see 團隊.

**Non-goals:**
- No change to any backend **authorization** — the existing `require_admin` /
  `require_team_rank` gates stay exactly as they are. We only **expose** the
  caller's in-team roles to the frontend and adjust **frontend visibility**.
- No new team operations.
- No change to `list_teams` scoping (it already returns only the caller's team for
  non-admins).
- We do **not** modify `agent_view` (shared by 2 callers). Teams are merged into
  the `/me` response in the `me` handler only.

---

## 2. Backend change — expose in-team roles on `/me`

**File:** `backend/src/domain/auth/handlers.rs`, handler `me` (~line 573).

`me` currently returns `Ok(envelope::ok(agent_view(&agent)))`. Change it to merge a
`teams` array into that object **without touching `agent_view`** (keeps the other
`agent_view` caller — `profile`-adjacent — unaffected; impact analysis: `agent_view`
upstream = LOW, 2 direct callers):

```rust
pub async fn me(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    let agent = store::find_agent_by_id(&state.db, &user.id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("Account not found".into()))?;
    let mut view = agent_view(&agent);
    let teams: Vec<Value> = user
        .teams
        .iter()
        .map(|t| json!({
            "teamId": t.team_id,
            "roleInTeam": t.role,
            "isPrimary": t.is_primary,
        }))
        .collect();
    if let Value::Object(ref mut map) = view {
        map.insert("teams".into(), json!(teams));
    }
    Ok(envelope::ok(view))
}
```

`AuthUser.teams` (`Vec<TeamMembership>` with `team_id`, `role`, `is_primary`) is
already populated on the request extension from the JWT — no DB call needed. The
added `teams` key is additive; existing `/me` consumers ignore it.

**Verification gate (this repo's CI):** `cargo clippy --all-targets -- -D warnings`
+ `cargo test` green.

---

## 3. Frontend changes

### 3.1 Session exposes in-team role (`frontend/src/auth/session.ts`)

- `TeamOption` gains `role?: string` (in-team role: `member`/`lead`/`supervisor`).
- `readTeams` captures it from each `/me` team item:
  `role: typeof item.roleInTeam === 'string' ? item.roleInTeam : undefined`.
  (The primary-only fallback entry — built from `who.teamId` when `teams` is
  absent — has no role; leave it `undefined`.)
- Add `session.isTeamManager()`:
  ```ts
  isTeamManager: () =>
    identity?.role === 'admin' ||
    teamOptions.some((t) => t.role === 'lead' || t.role === 'supervisor'),
  ```

### 3.2 Nav: move 團隊 into 日常 with a custom predicate (`frontend/src/components/AppShell.tsx`)

- Remove `{ to: '/teams', label: '團隊', area: 'ops' }` from the 營運管理 group.
- Add it to the **日常** group. Because plain agents must NOT see it, give the item
  an optional visibility predicate rather than relying on `area` alone:
  - Extend `NavItem` with an optional `show?: () => boolean`.
  - The filter becomes:
    `group.items.filter((i) => can(pos, i.area) && (i.show ? i.show() : true))`.
  - The teams item:
    `{ to: '/teams', label: '團隊', area: 'daily', show: () => can(session.position(), 'ops') || session.isTeamManager() }`.
  - Using `can(pos,'ops') || isTeamManager()` (not `isTeamManager()` alone) so
    existing ops-holders — global `supervisor`/`system_admin` — do **not** lose the
    entry even if they hold no in-team lead/supervisor role. In-team managers who
    are global `agent` gain it. (`area: 'daily'` is always-true for everyone; the
    `show` predicate is the real gate.)

### 3.3 Teams page gate (`frontend/src/pages/Teams.tsx`)

- Replace `if (!can(session.position(), 'ops'))` with
  `if (!(can(session.position(), 'ops') || session.isTeamManager()))` → matches the
  nav predicate exactly, so non-managers (plain members / unaffiliated agents) get
  權限不足 while ops-holders and in-team managers pass.

### 3.4 Hide admin-only controls for non-admins (`frontend/src/pages/Teams.tsx`)

Introduce `const isAdmin = session.position() === 'system_admin'` (or
`session.identity()?.role === 'admin'` — match the file's existing accessor). Then:

- **Create-team form** (the `<form onSubmit={create}>` card) → render only when `isAdmin`.
- **刪除團隊… button** → render only when `isAdmin`.
- **Status toggle** (啟用/停用 button in the member table `isActive` column) →
  when not admin, render a read-only `StatusPill` (no button), since
  `set_member_status` is `require_admin`.
- **Keep for managers:** role dropdown (`changeRole`), 移出團隊 (`removeFromTeam`),
  新增成員 (`addMember`), QR 顯示 (`showQr`). QR 重新產生 requires supervisor rank;
  leave it visible — a lead who lacks rank sees the surfaced error (acceptable; a
  finer gate is out of scope).

---

## 4. Error handling

- No new failure modes. Any residual action a non-admin manager is not permitted to
  do (e.g. a lead pressing 重新產生 QR without supervisor rank) surfaces the
  backend message via the page's existing `setError`/`setToast` paths.

---

## 5. Testing (vitest + cargo)

- **`session.test.ts`** (or nearest): with `/me` teams `[{teamId, roleInTeam:'supervisor'}]`
  and global role `agent`, `session.isTeamManager()` is `true`; with only
  `roleInTeam:'member'`, it is `false`; with global `admin` and no teams, `true`.
- **`AppShell` nav test**: a global-`agent` who is a team `lead` sees the 團隊 nav
  item (in 日常); a plain `member` does not.
- **`Teams.test.tsx`**: rendering as a non-admin manager (mock
  `session.isTeamManager() → true`, `position() → 'agent'`) hides the create-team
  form, the 刪除團隊… button, and the status toggle button, while still showing the
  role dropdown + 移出團隊 + 新增成員. Existing admin tests keep passing (admin sees
  everything).
- **Backend** (`me` handler): a unit/integration check that `/me` includes
  `teams` with `roleInTeam` for a user with memberships (or, if no existing harness,
  a focused assertion on the merge logic). Keep it minimal; the change is additive.

---

## 6. Verification

- `cd backend && cargo clippy --all-targets -- -D warnings && cargo test` — green.
- `cd frontend && npm run build && npx vitest run` — green; `package-lock.json` in sync.
- Manual:
  - Log in as a global-`agent` who is 主管 of a team → 團隊 appears under 日常 →
    open it → sees only their team; can change roles / 移出團隊 / 新增成員 / view QR;
    does NOT see 建立團隊 / 刪除團隊… / 啟用-停用 toggle.
  - Log in as a plain member (in-team `member`) → no 團隊 nav item; direct-navigating
    to `/teams` shows 權限不足.
  - Log in as admin → unchanged (全部可見、全部可用).

---

## 7. Resolved decisions

- Scope opened to **in-team lead/supervisor** only (option A); plain members
  excluded; admin-only ops hidden for non-admins.
- In-team role reaches the frontend via an **additive `teams` field on `/me`**
  (merged in the `me` handler; `agent_view` untouched).
- Nav gating uses a per-item `show()` predicate (`isTeamManager()`), with the item
  living in the 日常 group.
- **No backend authorization change** — only exposure + frontend visibility.

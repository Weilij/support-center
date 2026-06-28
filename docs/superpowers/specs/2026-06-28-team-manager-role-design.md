# Appoint Team Manager (team-member role options) — Design Spec

**Date:** 2026-06-28
**Track:** team management UI (frontend only)
**Status:** design approved, pending written-spec review

---

## 0. Context

A conversation/team uses a two-level role model:
- **Global agent role** (`agents.role`): `admin` | `agent`.
- **In-team role** (`team_members.role`): the backend `TEAM_ROLES = ["member", "lead", "supervisor"]` (rank: supervisor > lead > member). Team management is gated by `require_team_rank(team, "supervisor")` with an admin bypass — i.e. a **supervisor is the team's manager / "團隊管理員"** and can manage their own team; a system admin can manage any.

The bug: the frontend `Teams.tsx` member-role `<Select>` uses
`ROLE_OPTIONS = [{value:'agent'},{value:'admin'}]` — the **global** roles, not the
team roles. So `PUT /api/teams/members/{id}/role { role }` sends `agent`/`admin`,
which are not in `TEAM_ROLES`; an admin therefore **cannot appoint a member as
supervisor (team manager)** from the UI, and existing `lead`/`supervisor` rows show
no matching label.

**Goal:** let an admin appoint a team member as the team manager by fixing the
role options to the real team roles. Backend already supports it — **frontend only**.

---

## 1. Goal & non-goals

**Goal:** The member-role dropdown offers `member` / `lead` / `supervisor` with clear
labels, so changing a member to **主管(團隊管理員)** sends `role: 'supervisor'` and the
backend makes them the team's manager (who can then manage their own team).

**Non-goals:**
- No backend change (the endpoints + `TEAM_ROLES` + `require_team_rank` already exist).
- No new "supervisor can self-manage the Teams page" access work (that was a separate
  option the user did not choose); only the role-appointment fix.
- No change to the global `admin`/`agent` account roles (Agents page) or team creation
  (both already exist).

---

## 2. Frontend change (`Teams.tsx`)

- Replace `ROLE_OPTIONS` with the team roles:
  ```ts
  const ROLE_OPTIONS = [
    { value: 'member', label: '成員' },
    { value: 'lead', label: '組長' },
    { value: 'supervisor', label: '主管（團隊管理員）' },
  ]
  ```
- The member table's role `<Select>` currently defaults `value={m.role ?? 'agent'}` →
  change the fallback to `'member'` so an unset/legacy role shows as 成員 and the label
  resolves against the new options.
- No other logic changes — `changeRole(memberId, role)` already `PUT /api/teams/members/{id}/role { role }`; it now sends a valid `TEAM_ROLES` value.

---

## 3. Error handling

- Changing a role to an invalid value is no longer possible from the UI (the dropdown
  only offers `TEAM_ROLES`). A backend rejection (e.g. permission) surfaces via the
  existing `setError`/response-message path in `Teams.tsx` (unchanged).

---

## 4. Testing (vitest)

- `Teams.tsx`: the member-role `<Select>` renders the three labels 成員 / 組長 /
  主管（團隊管理員）; selecting 主管 calls `put('/api/teams/members/<id>/role', { role: 'supervisor' })`. (Mock `'../api/client'`; render the page with one team + one member; admin-gated mock.)

---

## 5. Verification

- `cd frontend && npm run build && npx vitest run` — green; `npm ci` in sync.
- Confirm (read-only) the backend `update_member_role`-style handler validates against
  `TEAM_ROLES` (so `supervisor` is accepted and `admin` would have been rejected) — no
  change needed, just confirm the new values are valid.
- Manual: in 團隊管理, open a team, change a member's role to **主管（團隊管理員）** → saved;
  that member can now manage the team (supervisor rank), per the existing backend gate.

---

## 6. Resolved decisions

- The member-role dropdown uses the real `TEAM_ROLES` (`member`/`lead`/`supervisor`),
  labelled 成員 / 組長 / **主管（團隊管理員）**; default fallback `member`.
- Frontend only; backend (roles, gating, endpoints) untouched.
- Out of scope: supervisor access to the Teams page, account creation, team creation
  (all already exist).

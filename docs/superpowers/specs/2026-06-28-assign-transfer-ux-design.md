# Assign / Transfer UX Clarification — Design Spec

**Date:** 2026-06-28
**Track:** inbox conversation routing UX (frontend only)
**Status:** design approved, pending written-spec review

---

## 0. Context

A conversation's ownership is modeled **only** by `conversations.team_id` (+ `status`);
there is **no agent-level assignee column** anywhere. The three backend endpoints all
operate on the team:
- `POST /api/conversations/{id}/assign` — set `team_id` (status → `assigned`).
- `POST /api/conversations/{id}/transfer` — set `team_id` to another team (status → `active`).
- `POST /api/conversations/{id}/unassign` — clear `team_id`.

So **"指派" and "轉接" do the same thing** (set the owning team) — they differ only in
"first assignment" vs "re-route" + the resulting status. The UI exposes both as
separate buttons (`ThreadHeader` 指派 + 轉接) plus a misleading **"指派給我"** button in
`MessageComposer` that actually opens a team-picker (it does NOT assign to the agent),
and a conversation-list **"團隊"** tab (recently renamed from "我的" by a collaborator)
that filters on `hasTeamAssignment` (any team) rather than the agent's own team.

**Decision (user):** keep the team-only model (no agent assignment); collapse the
two operations into one clear team action and fix the misleading pieces. **Frontend
only — no backend change** (reuse the three existing endpoints).

---

## 1. Goal & non-goals

**Goal:** One clear "指派團隊" action that assigns / re-routes / unassigns a conversation's
team; a one-click "指給我的團隊" quick action that honestly assigns to the agent's own
team; and a conversation-list tab that means "my team's conversations".

**Non-goals:**
- **No agent-level assignment** (no new `assignee` column / endpoint) — team-only.
- **No backend change** — the `assign`/`transfer`/`unassign` endpoints + their realtime
  events are reused as-is.
- No change to `status` semantics (assign→`assigned`, transfer→`active` preserved by
  routing to the matching endpoint).

---

## 2. Frontend changes

### 2.1 One unified "指派團隊" action (`ThreadHeader.tsx` + `Thread.tsx`)

- Replace the two header triggers (the 指派 button + the 轉接 button) with a **single
  "指派團隊" button** (the `users` icon). It opens the unified dialog.
- `Thread.tsx`: replace the `assignMode: 'assign' | 'transfer'` state with a single
  boolean (`assignOpen`); `onAssign`/`onTransfer` both collapse into one
  `onAssignTeam` that opens the dialog. The dialog decides assign-vs-transfer itself
  (§2.2), so the page no longer chooses a mode.

### 2.2 Unified dialog (`ConversationAssign.tsx`)

Turn `AssignDialog` from a 3-mode dialog into one self-determining "指派團隊" dialog:
- Title: **"指派團隊"**. Always rendered the same way.
- Shows the **current team** ("目前團隊：<name>" or "目前：未指派") from `currentTeamId`.
- A **team dropdown** (`teamsStore`) excluding the current team; optional **原因** field.
- Primary **「確定」** button, and — only when `currentTeamId` is set — a secondary
  **「取消指派」** action.
- On submit, route to the right existing endpoint by current state:
  - `currentTeamId == null` and a team chosen → `assignConversation(id, team, reason)`.
  - `currentTeamId != null` and a different team chosen → `transferConversation(id, team, currentTeamId, reason)`.
  - 「取消指派」 chosen → `unassignConversation(id, reason)`.
- Keep the `assignConversation`/`transferConversation`/`unassignConversation` store
  helpers unchanged. Drop the external `mode` prop (and the `AssignMode` type export if
  nothing else uses it — grep first; if other callers exist, keep the type but default
  the dialog to the unified behavior).

### 2.3 "指派給我" → "指給我的團隊" quick action (`MessageComposer.tsx`)

The composer's assign button currently opens the team-picker. Change it to a true
one-click quick action **"指給我的團隊"**: on click, resolve the agent's primary team id
from `session.identity()` (the session exposes the user's teams / primary `teamId`) and
call `assignConversation(convId, myPrimaryTeamId)` directly (no dialog), with a toast on
success/failure. If the agent has no primary team, hide/disable the button. (Wiring:
`MessageComposer` gains an `onQuickAssign` callback from `Thread.tsx`, which performs the
assign + toast.)

### 2.4 "我的團隊" tab (`ConversationList.tsx`)

- Relabel the `team` tab from **"團隊"** to **"我的團隊"**.
- Change its predicate from `hasTeamAssignment(c)` (any team) to **`c.team_id` ∈ the
  agent's team ids** — derive the agent's team-id set from `session.identity()`
  (primary `teamId` + any `teams[]`). Admins (or users with no team) see the unchanged
  "has a team" behavior as a sensible fallback so the tab is never empty for them.

---

## 3. Error handling

- Dialog: a failed assign/transfer/unassign surfaces the store helper's error message
  inline (existing behavior); the dialog stays open on failure.
- Quick "指給我的團隊": no primary team → button hidden/disabled (no dead click); a failed
  assign → error toast, conversation unchanged.
- Tab filter: a conversation with no team simply doesn't match "我的團隊" — no error.

---

## 4. Testing (vitest)

- `ConversationAssign`: with `currentTeamId=null`, choosing a team + 確定 calls
  `assignConversation`; with `currentTeamId=5`, choosing team 7 calls
  `transferConversation(id, 7, 5, …)`; 「取消指派」 calls `unassignConversation`.
  (Mock the three store helpers.)
- `ConversationList`: the "我的團隊" tab shows only conversations whose `teamId` is in the
  mocked session team set; "全部" shows all.
- Quick action: clicking "指給我的團隊" with a mocked primary team calls
  `assignConversation(convId, primaryTeamId)`.

---

## 5. Verification

- `cd frontend && npm run build && npx vitest run` — green; `npm ci` in sync.
- Manual: open a conversation → one "指派團隊" button → dialog shows current team, pick a
  team → assigns; reopen → pick a different team → transfers; 取消指派 → unassigns. The
  composer "指給我的團隊" one-click assigns to my team. The "我的團隊" tab lists only my
  team's conversations.

---

## 6. Resolved decisions

- **Team-only**; collapse 指派 + 轉接 into one **"指派團隊"** dialog that self-routes to
  assign / transfer / unassign. Backend untouched.
- **"指派給我" → "指給我的團隊"** one-click quick action (honest label; assigns to the
  agent's primary team).
- **Tab "團隊" → "我的團隊"**, filtered by the agent's own team ids (admin/no-team
  fallback = any team).
- No agent-level assignee. No status-semantics change.

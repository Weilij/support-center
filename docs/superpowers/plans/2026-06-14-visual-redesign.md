# Visual Redesign (Refined Glass + Sidebar) Implementation Plan

> Execute via superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** Redesign the whole frontend to a polished "refined Apple-Glass" look on a sidebar app shell — via a design-system layer (tokens + shell + shared components) plus bespoke redesigns of the key screens, so every page looks consistent and designed.

**Approved direction (brainstorm 2026-06-14):** Option B "精緻玻璃" — left sidebar (frosted, grouped nav gated by position) + top header bar; soft gradient background with frosted glass cards. Depth: design system + key-page polish (remaining pages inherit the shell + components).

**Constraints:** React 18 + inline styles + global `theme.css`. ZERO new dependencies. Do not touch the backend, the three-position permission model (`src/auth/permissions.ts` / `can`), routing logic, or data flows — only presentation. Verify each task with `node_modules/.bin/tsc --noEmit`, `npx vitest run`, `npx vite build` (run from `frontend/`). Work on `main` (no branch). Commit per task.

## Design tokens (target values)

Background gradient `linear-gradient(135deg,#dfe7ff 0%,#eadfff 42%,#d9f5ff 100%)` fixed.
- `--accent:#4f46e5`, `--accent-2:#3b82f6`, `--text:#1f2433`, `--muted:#7a82a0`
- `--surface:rgba(255,255,255,.55)`, `--surface-strong:rgba(255,255,255,.72)`, `--surface-border:rgba(255,255,255,.65)`
- `--blur:22px`, `--radius:18px`, `--radius-sm:11px`, `--shadow:0 8px 32px rgba(60,70,140,.10)`
- spacing scale `--sp-1:4px --sp-2:8px --sp-3:12px --sp-4:16px --sp-5:24px --sp-6:32px`
- status: ok `#16a34a`, warn `#b45309`/bg `rgba(245,158,11,.2)`, danger `#dc2626`

## File structure
- `src/styles/theme.css` — token overhaul + base element styles (rewrite)
- `src/components/AppShell.tsx` — NEW sidebar+header layout (replaces Shell as the page wrapper)
- `src/components/PageHeader.tsx` — NEW (title + subtitle + actions slot)
- `src/components/Card.tsx` — NEW (`Card`, `Panel`, `StatGrid`)
- `src/components/{DataTable,Modal,Form,ui}.tsx` — restyle to tokens
- `src/Shell.tsx` — kept or thinned; router uses AppShell instead
- `src/router.tsx` — `page()` wraps with `<AppShell title=...>`; pass route title
- `src/pages/Login.tsx`, `src/pages/Dashboard.tsx` — bespoke redesign
- all other `src/pages/*.tsx` — wrap content in `PageHeader` + `Card`

---

### Task 1: Design tokens + base styles (`theme.css`)
Rewrite `src/styles/theme.css` with the token set above: `:root` vars, `body` gradient bg + base font/color, `.glass`/`.glass-strong` utilities (bg + backdrop-blur + border + radius + shadow), refined `button` (glass default + `.btn-primary` accent), `input/select/textarea` (translucent, focus ring), custom scrollbar, and base `h1/h2/h3` sizing. Verify `npx vite build`. Commit.

### Task 2: AppShell (sidebar + header) + router integration
Create `src/components/AppShell.tsx`: a flex layout with a frosted floating **sidebar** (brand, grouped nav from the existing NAV_GROUPS data filtered by `can(session.position(), area)`, active item highlight via `useLocation`) and a **header** (page title + greeting/subtitle slot, notifications bell with unread badge from `notificationsStore`, user displayName + avatar, logout). Content area renders `children` on the gradient bg with padding. Accept a `title?` prop for the header.
Update `src/router.tsx` `page()` to wrap authed routes with `<AppShell title={meta.title}>{element}</AppShell>` instead of `<Shell>`. Keep guest routes (login/install) un-shelled. Preserve all gating (Guard unchanged). Active-nav, position gating, and logout behavior must match the old Shell. Verify tsc + vitest + build. Commit. (REVIEW)

### Task 3: Layout components + restyle shared components
Create `src/components/PageHeader.tsx` (`<PageHeader title subtitle actions>`), `src/components/Card.tsx` exporting `Card` (glass panel, optional `title`/`actions`), `Panel`, and `StatGrid` (responsive grid wrapper for StatCards). Restyle `DataTable`, `Modal/Drawer`, `Form` controls, and `ui` (`StatCard`, `Badge`, `StatusPill`, `FilterBar`, `Toast`) to the new tokens (glass surfaces, spacing, radius). Keep all component props/APIs unchanged. Export new components from `src/components/index.ts`. Verify tsc + vitest + build. Commit.

### Task 4: Login redesign
Redesign `src/pages/Login.tsx` into a centered frosted glass card on the gradient bg: brand mark + title, the existing email/password form (keep all auth logic/state untouched), styled inputs/button (`.btn-primary`), error display. Verify tsc + build. Commit.

### Task 5: Dashboard redesign (real data)
Redesign `src/pages/Dashboard.tsx`: a greeting `PageHeader` (uses `session.identity()`), a `StatGrid` of metric `StatCard`s from `/api/system/stats` (對話/訊息/客戶 — keep to fields the API returns; do NOT fabricate trends, omit what isn't available), a "最近對話" `Card` listing the first rows from `loadConversations`/`conversationsStore`, and a "團隊狀態" `Card` from `loadTeams`/`teamsStore` (online counts via memberCount or status if available; otherwise show team list). Wire real stores already in the codebase. Verify tsc + vitest + build. Commit. (REVIEW)

### Task 6: Apply PageHeader + Card to daily-area pages
Wrap content of `Conversations`, `ConversationDetail`, `Customers`, `MessageSearch`, `Reminders`, `Notifications`, `Tags`, `AutoReply` in `PageHeader` (page title) + `Card`/`Panel` where they currently use bare `<main><h1>` + ad-hoc containers. Keep all logic/data/handlers unchanged — only restructure presentation to the new components and tokens. Light polish on `ConversationDetail` message bubbles. Verify tsc + vitest + build. Commit.

### Task 7: Apply PageHeader + Card to ops/analytics/system pages
Same treatment for `Agents`, `Teams`, `Sessions`, `Analytics`, `Reports`, `Activity`, `LiffSettings`, `Settings`, `Profile`, `Channels`, `SystemMonitoring`, `AlertConfig`, `SystemMaintenance`. Preserve each page's permission gate and logic. Verify tsc + vitest + build. Commit.

### Task 8: Final verification
`npm run build` + `npx vitest run`. Restart frontend; screenshot Login, Dashboard, Conversations, Agents to confirm the redesign. Fix any visual breakage. Final holistic review.

## Self-review
- Direction (glass+sidebar) → Tasks 1,2. Design system → Tasks 1,2,3. Key screens → 4,5. Whole-app consistency → 6,7. ✅
- No backend/permission/data changes — presentation only. ✅
- Zero new deps. ✅

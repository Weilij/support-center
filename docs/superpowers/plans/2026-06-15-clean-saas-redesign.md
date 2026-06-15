# Clean Light SaaS Redesign (replace glass) — Implementation Plan

> Execute via subagent-driven development; verify each task with build + tests.

**Goal:** Replace the Apple-Glass theme with the "匯流客服 Omnichannel Desk" clean light SaaS design system from the handoff at `/Users/kkllzz_0/Downloads/design_handoff_customer_service/` (read `README.md` + `assets/styles.css` for pixel-level specs). Apply the design system across all ~20 pages; rebuild the Dashboard to spec; build a new 3-column Inbox merging the conversation list + thread + customer panel.

**Approved scope (brainstorm 2026-06-15):** full-app re-skin to the new design system + bespoke Dashboard + 3-column Inbox.

**Aesthetic (from handoff):** sky-blue accent (`--blue-600 #0284c7`), cool slate neutrals, white cards on `--bg #f6f8fb`, 16/12/9px radii, soft shadows, Noto Sans TC + Roboto Mono. NOT glass.

**Constraints:** React 18 + Vite. Reference the handoff for exact values but implement in our codebase. Do NOT touch backend, auth/permissions, routing logic, or data flows — presentation only. No fabricated data (Dashboard uses real values where the backend provides them; omit/placeholder the rest honestly). Verify each task: `node_modules/.bin/tsc --noEmit`, `npx vitest run`, `npx vite build` (from `frontend/`). Work on `main`. Commit per task.

## Token-migration strategy
The current code references glass tokens (`--glass-bg`, `--surface-strong`, `--surface-border`, `--blur`, `--accent`, `--sp-*`, `--radius`). The new theme defines the handoff tokens (`--blue-*`, `--ink`, `--line`, `--surface`, `--radius-lg/-sm`, `--shadow-sm/-/-lg`, `--font`, `--mono`). To avoid breaking every inline style at once, `theme.css` keeps COMPATIBILITY ALIASES mapping the old names to new values (e.g. `--accent: var(--blue-600)`, `--surface-strong: var(--surface)`, `--surface-border: var(--line)`, `--blur: 0px`, keep `--muted`, `--radius`, `--sp-*`, `--shadow`). Glass-specific effects (`backdrop-filter: blur(0)`) become no-ops; later tasks remove them from the chrome.

## Tasks

### N1 — Design tokens + fonts + component classes (`theme.css`, `index.html`)
Rewrite `src/styles/theme.css`: `:root` = the handoff `:root` (read `assets/styles.css`) PLUS the compatibility aliases above; base `body` (font `--font`, bg `--bg`, color `--ink`); base `button`/`input`/`select`/`textarea` to the handoff `.cs-btn`/inputs look; port the handoff component classes (`.cs-side`, `.cs-nav`, `.cs-topbar`, `.cs-card*`, `.cs-kpi*`, `.cs-chip`, `.cs-tag`, `.cs-status*`, `.cs-bar`, `.cs-av*`, `.cs-conv*`, `.cs-thread*`, `.cs-bubble*`, `.cs-composer*`, `.cs-cust*`, `.cs-kv`, `.cs-mono`, `.cs-divider`, etc.) verbatim-adapted so the new screens can use them. Add Google Fonts `<link>` for Noto Sans TC (400/500/600/700) + Roboto Mono in `index.html`. Verify `vite build`.

### N2 — AppShell: light sidebar + topbar
Rebuild `src/components/AppShell.tsx` to the handoff Sidebar (`.cs-side`, 244px white, brand mark sky-blue gradient + chat icon + 「客服中心」, grouped nav with `.cs-nav`/`.cs-nav--active` (blue-50), unread badge `.cs-nav-badge`, footer user) and Topbar (`.cs-topbar`, 70px, title + subtitle, bell `.cs-icon-btn` + actions). PRESERVE position gating (`can(pos, area)`), active-nav logic, logout, and the responsive mobile drawer. Add an `Icon` component (port `ICONS` map + `Icon` from handoff `components.jsx`) at `src/components/Icon.tsx` and give each nav item a handoff icon name. Router passes `title`/`subtitle` as before. Verify tsc + vitest + build.

### N3 — Shared components + new primitives
Restyle to the new tokens/classes: `Card`/`Panel` (`.cs-card` + `.cs-card-head`/`-title`/`-link`), `PageHeader`, `DataTable`, `Modal/Drawer`, `Form` controls, `StatCard`, `Badge`, `StatusPill` (`.cs-status`), `FilterBar`, `Toast`. Add new components: `Avatar` (initials + hashed color, port from handoff), `ChanGlyph` (channel badge), `Chip`/`Tag` (the handoff tag palette: 訂單/優惠/退換貨/客訴/會員/已結案/運送中), `KpiCard`, `Bar` (progress). Export from `components/index.ts`. Keep all prop APIs. Verify tsc + vitest + build.

### N4 — Dashboard to spec
Rebuild `src/pages/Dashboard.tsx` per handoff §畫面1: KPI row ×4 (`KpiCard`), a 1.55fr/1fr row = 渠道對話分佈卡 + 客服團隊狀態卡, and a full-width 待處理佇列卡 (4 cols). Wire REAL data: `/api/system/stats` (對話/訊息/客戶), agent presence (`/api/agents/status/statistics`), recent/queue from `conversationsStore`, teams from `teamsStore`. For metrics the backend lacks (CSAT, channel %, FRT, 7-day trend) show "—" or a real available substitute — DO NOT fabricate. Keep the exact card/KPI visual structure. Verify tsc + vitest + build.

### N5 — 3-column Inbox (merge Conversations + ConversationDetail)
Create `src/pages/Inbox.tsx` per handoff §畫面2: `.cs-conv-list` (340px: header + search + tabs 全部/未讀/我的/待跟進 + `.cs-conv-item` rows with avatar+ChanGlyph, unread dot, tags) + `.cs-thread` (header, message area with `.cs-bubble`/`--me`, day separator, composer `.cs-composer`) + `.cs-cust` customer panel (profile, contact KV, recent order, 3-col stats). Route `/conversations` → Inbox; `/conversations/:id` → Inbox with that conversation selected. REUSE existing stores/logic: `conversationsStore`/`loadConversations`, message load + optimistic send (`/api/conversations/:id/messages`), realtime `onEvent`/`subscribeConversation`, assign/transfer (`AssignDialog`), files + scheduling, customer detail (`loadCustomerDetail`). Customer-panel order/stats: real where available, omit otherwise. Update router + remove the old Conversations list page from nav (Inbox replaces it). Verify tsc + vitest + build.

### N6 — Re-skin remaining pages
Sweep the other pages (Customers, MessageSearch, Reminders, Notifications, Tags, AutoReply, Agents, Teams, Sessions, Analytics, Reports, Activity, LiffSettings, Settings, Profile, Channels, SystemMonitoring, AlertConfig, SystemMaintenance, Login): remove glass-specific inline styles (backdrop-filter, glass bg), ensure they use the restyled Card/PageHeader/DataTable and new tokens so they match the clean light look. Redesign Login to a clean white card (sky-blue brand). Verify tsc + vitest + build.

### N7 — Final verification
`npm run build` + `vitest`. Restart frontend; screenshot Dashboard, Inbox, Login, Agents to confirm the new design. Final review.

## Self-review
- Direction (clean light SaaS) → N1. Design system global → N1,N2,N3. Hero screens → N4 (Dashboard), N5 (Inbox). Whole-app consistency → N6. ✓
- Presentation only; no backend/permission/data changes. No fabricated metrics. ✓

# Foundation — Design Tokens (Skywork Unified Inbox) — Design Spec

**Date:** 2026-06-20
**Track:** A① (frontend, first sub-project)
**Status:** design approved, pending written-spec review

---

## 0. Program context (why this is first)

The "Skywork Unified Inbox Design System v1.0" reference reshapes the support-center frontend. It is large, so it is decomposed into sequential sub-projects, each with its own spec → plan → implementation:

- **Track A① — Foundation tokens** *(this spec)*: formalize the design system as additive CSS tokens + a dark-mode mechanism. Pure style; underpins everything below.
- **Track A② — Inbox restyle + layout**: match the 3-column reference (conversation list │ chat thread │ customer panel) **and** adopt its layout features (badge × tag, collapsible customer panel).
- **Track A③ — Motion & power-user features**: `/` command menu, keyboard shortcuts (⌘K / ⌘↵ / Tab), list-reorder / micro-interactions — **net-new additions**.
- **Track B — Backend platform adapters** *(separate, later)*: fill in FB / IG / Shopee ingestion + send (LINE already exists). Its own spec.

**Guiding rule for the frontend tracks:** the design image is a **style/layout reference**. We do **not** rewrite or break existing working logic, routes, or data flow — but we **may add** net-new UX features the design introduces that the current app lacks (those live in A②/A③, not here).

---

## 1. Goal & non-goals (this spec)

**Goal:** add the design system as **additive** CSS custom properties in `styles/theme.css`, plus a `[data-theme]` dark-mode mechanism, so all later work draws from one token source and the app supports light/dark.

**Non-goals:**
- No functional/logic refactor; no route, store, or API changes.
- No **renaming or re-valuing** of existing tokens (26 pages depend on them) — strictly additive.
- No page restyle (Track A②) and no `/` command / shortcuts (Track A③).
- No living style-guide page.
- No backend changes.

---

## 2. Current state (verified)

- `styles/theme.css` (imported once in `main.tsx`) already defines: a blue scale `--blue-50…900`, cool-slate neutrals (`--ink/--ink-2/--muted/--muted-2/--line/--line-2/--bg/--surface`), status `--ok/--warn/--busy` (oklch), some channel colors (`--line-green #06c755`, `--fb-blue #0084ff`, `--wa-green`, `--chat-blue`), radius `--radius-sm 9 / --radius 12 / --radius-lg 16`, shadows `--shadow-sm/--shadow/--shadow-lg`, spacing `--sp-1…6`, fonts.
- **No app-wide dark mode** exists — only a `.cs-side--dark` sidebar variant. Dark mode is genuinely new.

---

## 3. Token groups to add (all additive)

### 3a. Platform brand colors (4 platforms)
```css
--brand-line:#06C755;   --brand-line-ink:#fff;
--brand-fb:#1877F2;     --brand-fb-ink:#fff;
--brand-shopee:#EE4D2D; --brand-shopee-ink:#fff;
--brand-ig:#E1306C;     --brand-ig-ink:#fff;            /* solid fallback */
--brand-ig-gradient: linear-gradient(135deg,#833AB4 0%,#FD1D1D 55%,#FCAF45 100%);
```
Brand colors are identity — they stay **fixed in dark mode**. (Existing `--line-green/--fb-blue` are kept as-is; the new `--brand-*` are the canonical names going forward.)

### 3b. Semantic status colors (UNREAD / ONLINE / EVENT / URGENT)
Each has a strong value (dot/text) + a soft tint (badge background):
```css
--status-unread:#0EA5E9;  --status-unread-soft:#E0F2FE;
--status-online:#16A34A;  --status-online-soft:#DCFCE7;
--status-event:#D97706;   --status-event-soft:#FEF3C7;
--status-urgent:#DC2626;  --status-urgent-soft:#FEE2E2;
```

### 3c. Elevation (formal names aliasing the 3 existing shadows)
```css
--elevation-1: var(--shadow-sm);
--elevation-2: var(--shadow);
--elevation-3: var(--shadow-lg);
```

### 3d. Radius scale — **D1: new namespace** (non-breaking)
The spec's `sm=6, lg=12` conflict with existing `--radius-sm=9 / --radius-lg=16`. To stay additive, add the canonical scale under a fresh `--r-*` namespace; leave `--radius-*` untouched (legacy):
```css
--r-xs:4px; --r-sm:6px; --r-md:8px; --r-lg:12px; --r-xl:16px; --r-pill:9999px;
```
New components use `--r-*`; existing pages keep `--radius-*`.

### 3e. Typography scale
```css
--fs-title:18px;   --fw-title:600; --lh-title:1.3;
--fs-body:13px;    --fw-body:400;  --lh-body:1.55;
--fs-caption:12px; --fw-caption:400;--lh-caption:1.4;
--fs-label:11px;   --fw-label:700; --ls-label:.04em;
```
Plus optional helper classes that bundle size+weight+line-height for direct use:
`.ds-title / .ds-body / .ds-caption / .ds-label`.

---

## 4. Dark mode

**Mechanism:** `:root[data-theme="dark"]` overrides the **neutral** tokens only; brand colors fixed.
```css
:root[data-theme="dark"]{
  --bg:#0B1220; --surface:#111A2B; --surface-strong:#16223A;
  --ink:#E8EEF6; --ink-2:#C3CFDD; --muted:#93A2B6; --muted-2:#5F7088;
  --line:#1F2C40; --line-2:#18222F;
  --primary-tint:rgba(56,189,248,.14);
  --elevation-1:0 1px 2px rgba(0,0,0,.5);
  --elevation-2:0 1px 3px rgba(0,0,0,.5),0 6px 18px rgba(0,0,0,.45);
  --elevation-3:0 2px 6px rgba(0,0,0,.5),0 16px 40px rgba(0,0,0,.55);
  /* status-soft tints get darker, lower-alpha variants */
}
```
Exact dark neutral/soft values are tuned during implementation for contrast (WCAG AA on text).

**`theme.ts` (new util):**
- `type Theme = 'light' | 'dark'`
- `resolveInitialTheme()` → `localStorage['theme']` if set, else `matchMedia('(prefers-color-scheme: dark)')`.
- `applyTheme(t)` → sets `document.documentElement.dataset.theme = t` and persists to `localStorage`.
- `toggleTheme()` → flips current and applies.
- `initTheme()` → called from `main.tsx` before render; applies the resolved theme.

**D2: minimal toggle.** One icon button (sun/moon) added to the existing top bar (`AppShell` topbar) calling `toggleTheme()`, so dark mode is verifiable now. No other UI.

---

## 5. Files

- **Edit** `src/styles/theme.css` — append §3 token groups, the `[data-theme="dark"]` block (§4), and the `.ds-*` helper classes. No existing line changed.
- **Add** `src/theme.ts` — the theme util (§4).
- **Edit** `src/main.tsx` — call `initTheme()` before mount.
- **Edit** `src/components/AppShell.tsx` — add the minimal theme-toggle button to the top bar.

---

## 6. Verification

- `npm run build` (tsc -b + vite build) clean.
- `npm test` (vitest) green, **including** a new unit test for `theme.ts`: resolves from `localStorage`, falls back to OS `matchMedia`, `applyTheme` sets `data-theme` + persists, `toggleTheme` flips. (Mock `localStorage` + `matchMedia`.)
- Manual: toggling switches the whole app light↔dark; existing pages render unchanged in light mode (proves the additions are non-breaking).

---

## 7. Open decisions — resolved
- **D1** radius naming → **new `--r-*` namespace** (non-breaking).
- **D2** dark-mode toggle → **minimal top-bar toggle now**.

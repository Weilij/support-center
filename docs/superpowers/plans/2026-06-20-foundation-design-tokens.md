# Foundation Design Tokens Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the Skywork design system as additive CSS tokens plus a `[data-theme]` light/dark mechanism with a minimal top-bar toggle, changing no existing behavior.

**Architecture:** Append new token groups (platform brand, semantic status, elevation, radius `--r-*`, typography) to `styles/theme.css` without touching existing tokens; add a `[data-theme="dark"]` block overriding only neutral tokens (brand colors stay fixed); a small `theme.ts` resolves/persists the theme via `localStorage` + OS preference and applies it through `document.documentElement.dataset.theme`; a `ThemeToggle` button in the existing top bar flips it.

**Tech Stack:** React 18 + TypeScript + Vite, vitest (jsdom), plain CSS custom properties.

**Spec:** `docs/superpowers/specs/2026-06-20-foundation-design-tokens-design.md`

---

## File Structure

- `src/styles/theme.css` — **modify** (append-only): new token groups, `.ds-*` helper classes, and the dark-mode override block. No existing line edited.
- `src/theme.ts` — **create**: theme resolution/persistence/apply util. Single responsibility: theme state.
- `src/__tests__/theme.test.ts` — **create**: unit tests for `theme.ts`.
- `src/main.tsx` — **modify**: call `initTheme()` before render.
- `src/components/Icon.tsx` — **modify**: add `sun` + `moon` glyphs to `ICONS`.
- `src/components/ThemeToggle.tsx` — **create**: the toggle button (uses `theme.ts`). Single responsibility: the toggle control.
- `src/components/AppShell.tsx` — **modify**: render `<ThemeToggle/>` in the top bar.

---

## Task 1: Additive token groups in theme.css

**Files:**
- Modify: `src/styles/theme.css` (append after the existing compatibility `:root` block, around line 70 — do not edit existing lines)

- [ ] **Step 1: Append the new token block**

Add this at the end of `src/styles/theme.css` is acceptable, but preferred placement is immediately after the first `:root{…}` block closes (after line 70). Insert:

```css
/* ===== Skywork design-system tokens (additive, Track A1) ===== */
:root {
  /* Platform brand colors (identity — stay fixed in dark mode) */
  --brand-line: #06C755;   --brand-line-ink: #fff;
  --brand-fb: #1877F2;     --brand-fb-ink: #fff;
  --brand-shopee: #EE4D2D; --brand-shopee-ink: #fff;
  --brand-ig: #E1306C;     --brand-ig-ink: #fff;
  --brand-ig-gradient: linear-gradient(135deg, #833AB4 0%, #FD1D1D 55%, #FCAF45 100%);

  /* Semantic status colors (strong value + soft tint for badge backgrounds) */
  --status-unread: #0EA5E9;  --status-unread-soft: #E0F2FE;
  --status-online: #16A34A;  --status-online-soft: #DCFCE7;
  --status-event: #D97706;   --status-event-soft: #FEF3C7;
  --status-urgent: #DC2626;  --status-urgent-soft: #FEE2E2;

  /* Elevation — formal aliases of the existing shadows */
  --elevation-1: var(--shadow-sm);
  --elevation-2: var(--shadow);
  --elevation-3: var(--shadow-lg);

  /* Radius scale — NEW namespace (non-breaking vs legacy --radius-sm/--radius/--radius-lg) */
  --r-xs: 4px;
  --r-sm: 6px;
  --r-md: 8px;
  --r-lg: 12px;
  --r-xl: 16px;
  --r-pill: 9999px;

  /* Typography scale */
  --fs-title: 18px;   --fw-title: 600;  --lh-title: 1.3;
  --fs-body: 13px;    --fw-body: 400;   --lh-body: 1.55;
  --fs-caption: 12px; --fw-caption: 400; --lh-caption: 1.4;
  --fs-label: 11px;   --fw-label: 700;  --ls-label: .04em;
}

/* Typography helper classes (bundle size + weight + line-height for direct use) */
.ds-title   { font-size: var(--fs-title);   font-weight: var(--fw-title);   line-height: var(--lh-title); }
.ds-body    { font-size: var(--fs-body);    font-weight: var(--fw-body);    line-height: var(--lh-body); }
.ds-caption { font-size: var(--fs-caption); font-weight: var(--fw-caption); line-height: var(--lh-caption); }
.ds-label   { font-size: var(--fs-label);   font-weight: var(--fw-label);   letter-spacing: var(--ls-label); text-transform: uppercase; }
```

- [ ] **Step 2: Verify the build still compiles**

Run: `cd frontend && npm run build`
Expected: `tsc -b` + `vite build` succeed, no errors.

- [ ] **Step 3: Verify the tokens are present**

Run: `cd frontend && grep -c -E "\-\-brand-line|\-\-status-unread|\-\-elevation-1|\-\-r-pill|\-\-fs-title" src/styles/theme.css`
Expected: `5` (one match per pattern, at minimum).

- [ ] **Step 4: Commit**

```bash
git add frontend/src/styles/theme.css
git commit -m "feat(design-system): add platform/semantic/elevation/radius/type tokens"
```

---

## Task 2: theme.ts util (TDD)

**Files:**
- Create: `src/theme.ts`
- Test: `src/__tests__/theme.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/__tests__/theme.test.ts`:

```ts
import { beforeEach, describe, expect, it, vi } from 'vitest'

import {
  applyTheme,
  currentTheme,
  initTheme,
  resolveInitialTheme,
  storedTheme,
  toggleTheme,
} from '../theme'

function mockMatchMedia(matches: boolean) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    configurable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches,
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  })
}

describe('theme', () => {
  beforeEach(() => {
    localStorage.clear()
    delete document.documentElement.dataset.theme
    mockMatchMedia(false)
  })

  it('resolves from localStorage when set', () => {
    localStorage.setItem('theme', 'dark')
    expect(storedTheme()).toBe('dark')
    expect(resolveInitialTheme()).toBe('dark')
  })

  it('falls back to OS preference when unset', () => {
    mockMatchMedia(true)
    expect(storedTheme()).toBeNull()
    expect(resolveInitialTheme()).toBe('dark')
  })

  it('applyTheme sets data-theme and persists', () => {
    applyTheme('dark')
    expect(document.documentElement.dataset.theme).toBe('dark')
    expect(localStorage.getItem('theme')).toBe('dark')
  })

  it('toggleTheme flips current and persists', () => {
    applyTheme('light')
    expect(toggleTheme()).toBe('dark')
    expect(currentTheme()).toBe('dark')
    expect(toggleTheme()).toBe('light')
    expect(localStorage.getItem('theme')).toBe('light')
  })

  it('initTheme applies the resolved theme without persisting the OS default', () => {
    mockMatchMedia(true)
    initTheme()
    expect(document.documentElement.dataset.theme).toBe('dark')
    expect(localStorage.getItem('theme')).toBeNull()
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/__tests__/theme.test.ts`
Expected: FAIL — cannot resolve module `../theme`.

- [ ] **Step 3: Write the implementation**

Create `src/theme.ts`:

```ts
// App-wide light/dark theme. Resolves from localStorage or the OS preference,
// applies via the [data-theme] attribute on <html>, and persists explicit choices.

export type Theme = 'light' | 'dark'

const STORAGE_KEY = 'theme'

export function storedTheme(): Theme | null {
  const v = localStorage.getItem(STORAGE_KEY)
  return v === 'light' || v === 'dark' ? v : null
}

export function systemTheme(): Theme {
  return typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-color-scheme: dark)').matches
    ? 'dark'
    : 'light'
}

export function resolveInitialTheme(): Theme {
  return storedTheme() ?? systemTheme()
}

export function currentTheme(): Theme {
  return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light'
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme
  localStorage.setItem(STORAGE_KEY, theme)
}

export function toggleTheme(): Theme {
  const next: Theme = currentTheme() === 'dark' ? 'light' : 'dark'
  applyTheme(next)
  return next
}

// Applies the resolved theme on boot. Does NOT persist, so an OS-preference
// user is not locked in until they explicitly toggle.
export function initTheme(): void {
  document.documentElement.dataset.theme = resolveInitialTheme()
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/__tests__/theme.test.ts`
Expected: PASS — 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/theme.ts frontend/src/__tests__/theme.test.ts
git commit -m "feat(theme): add light/dark theme util with OS-preference fallback"
```

---

## Task 3: Dark-mode token overrides in theme.css

**Files:**
- Modify: `src/styles/theme.css` (append the dark block after the Task 1 block)

- [ ] **Step 1: Append the dark-mode override block**

Add to the end of `src/styles/theme.css`:

```css
/* ===== Dark mode — overrides neutral tokens only; brand colors stay fixed ===== */
:root[data-theme="dark"] {
  color-scheme: dark;

  --bg: #0B1220;
  --surface: #111A2B;
  --surface-strong: #16223A;
  --ink: #E8EEF6;
  --ink-2: #C3CFDD;
  --muted: #93A2B6;
  --muted-2: #5F7088;
  --line: #1F2C40;
  --line-2: #18222F;

  --primary-tint: rgba(56, 189, 248, .14);
  --accent-soft: rgba(56, 189, 248, .16);

  /* Shadows darken so existing components (which read --shadow*) also adapt */
  --shadow-sm: 0 1px 2px rgba(0, 0, 0, .5);
  --shadow:    0 1px 3px rgba(0, 0, 0, .5), 0 6px 18px rgba(0, 0, 0, .45);
  --shadow-lg: 0 2px 6px rgba(0, 0, 0, .5), 0 16px 40px rgba(0, 0, 0, .55);

  /* Semantic soft tints become low-alpha so badges read on dark surfaces */
  --status-unread-soft: rgba(14, 165, 233, .18);
  --status-online-soft: rgba(22, 163, 74, .20);
  --status-event-soft:  rgba(217, 119, 6, .20);
  --status-urgent-soft: rgba(220, 38, 38, .22);
}
```

- [ ] **Step 2: Verify the build compiles**

Run: `cd frontend && npm run build`
Expected: succeeds, no errors.

- [ ] **Step 3: Verify the dark block exists**

Run: `cd frontend && grep -c 'data-theme="dark"' src/styles/theme.css`
Expected: `1`.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/styles/theme.css
git commit -m "feat(design-system): add dark-mode neutral token overrides"
```

---

## Task 4: Initialize theme on boot (main.tsx)

**Files:**
- Modify: `src/main.tsx`

- [ ] **Step 1: Add the import and the init call**

In `src/main.tsx`, add the import after the existing imports (e.g. after line 8) and call `initTheme()` before `ReactDOM.createRoot(...)`. Resulting file:

```tsx
import React from 'react'
import ReactDOM from 'react-dom/client'
import { RouterProvider } from 'react-router-dom'

import './styles/theme.css'
import { router } from './router'
import { session } from './auth/session'
import { connectRealtime } from './realtime/client'
import { initTheme } from './theme'

// Apply the persisted/OS theme before first paint to avoid a flash.
initTheme()

// Establish the realtime channel once a session exists (CRD §8.3).
void session.init().then(() => {
  if (session.lifecycle() === 'authenticated') connectRealtime()
})

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <RouterProvider router={router} />
  </React.StrictMode>,
)
```

- [ ] **Step 2: Verify build + tests**

Run: `cd frontend && npm run build && npx vitest run`
Expected: build succeeds; all tests pass (theme tests + existing suite).

- [ ] **Step 3: Commit**

```bash
git add frontend/src/main.tsx
git commit -m "feat(theme): apply resolved theme on app boot"
```

---

## Task 5: Theme toggle in the top bar

**Files:**
- Modify: `src/components/Icon.tsx` (add `sun` + `moon` to `ICONS`)
- Create: `src/components/ThemeToggle.tsx`
- Modify: `src/components/AppShell.tsx` (render the toggle in the top bar)

- [ ] **Step 1: Add sun + moon glyphs to ICONS**

In `src/components/Icon.tsx`, inside the `ICONS` record (it ends before line 34), add two entries:

```ts
  sun: 'M12 17a5 5 0 1 0 0-10 5 5 0 0 0 0 10M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42',
  moon: 'M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z',
```

(The `Icon` component splits the path string on `M` and renders one `<path>` per segment, so the multi-segment `sun` works as-is.)

- [ ] **Step 2: Create the ThemeToggle component**

Create `src/components/ThemeToggle.tsx`:

```tsx
import { useState } from 'react'

import { currentTheme, toggleTheme, type Theme } from '../theme'
import { Icon } from './Icon'

// Minimal top-bar control: shows a moon in light mode (click → dark) and a sun
// in dark mode (click → light). Holds its own display state; theme.ts owns the
// actual document/localStorage side effect.
export function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(currentTheme())
  return (
    <button
      className="cs-icon-btn"
      aria-label={theme === 'dark' ? '切換為淺色' : '切換為深色'}
      onClick={() => setTheme(toggleTheme())}
    >
      <Icon name={theme === 'dark' ? 'sun' : 'moon'} w={18} />
    </button>
  )
}
```

- [ ] **Step 3: Render the toggle in the top bar**

In `src/components/AppShell.tsx`:

(a) Add the import near the other component imports:

```tsx
import { ThemeToggle } from './ThemeToggle'
```

(b) In the topbar `<header className="cs-topbar">`, insert the toggle immediately before the notifications `<Link to="/notifications" …>` block (around line 378):

```tsx
          {/* Light/dark toggle */}
          <ThemeToggle />

          {/* Bell icon — links to /notifications, shows alert dot when unread > 0 */}
          <Link to="/notifications" style={{ textDecoration: 'none' }}>
```

- [ ] **Step 4: Verify build + tests**

Run: `cd frontend && npm run build && npx vitest run`
Expected: build succeeds; all tests pass.

- [ ] **Step 5: Manual verification**

Run: `cd frontend && npm run dev`, open the app, click the moon/sun button in the top bar.
Expected: the whole app switches light↔dark; reload preserves the choice (localStorage); in light mode every existing page looks unchanged from before.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/Icon.tsx frontend/src/components/ThemeToggle.tsx frontend/src/components/AppShell.tsx
git commit -m "feat(theme): add top-bar light/dark toggle"
```

---

## Final verification (after all tasks)

- [ ] `cd frontend && npm run build` — clean
- [ ] `cd frontend && npx vitest run` — all suites green (theme + existing 22)
- [ ] Manual: dark/light toggle works app-wide and persists; light mode visually identical to pre-change (proves additivity / no functional change).

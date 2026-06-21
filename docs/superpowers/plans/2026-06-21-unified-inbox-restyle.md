# Unified Inbox Restyle (Track A2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle the existing 3-column Inbox to the Skywork reference and add badge×tag (IG/Shopee platforms) plus a collapsible customer panel, without changing any inbox logic.

**Architecture:** Extend the existing `CHANNELS`/`ChanGlyph`/`Tag` components and the inbox-only `.cs-*` CSS; add a small `useCollapsed` localStorage hook and reuse the inbox's existing `onToggleCustPanel`/`showCustToggle` plumbing to make the customer panel collapsible at wide width.

**Tech Stack:** React 18 + TypeScript + Vite, vitest (jsdom), CSS custom properties (A1 tokens already merged).

**Spec:** `docs/superpowers/specs/2026-06-21-unified-inbox-restyle-design.md`

---

## File Structure

- `src/components/channels.ts` — **modify**: add `ig`/`shopee` to `CHANNELS`, extend `PLATFORM_MAP`, use A1 brand tokens for colors.
- `src/components/ChanGlyph.tsx` — **modify**: widen `type` union, add labels, IG gradient background.
- `src/pages/Inbox.tsx` — **modify**: widen the three `ChanGlyph` casts; wire wide-layout panel collapse + section accordion. No logic changes.
- `src/hooks/useCollapsed.ts` — **create**: localStorage-backed open/closed state hook (one responsibility).
- `src/__tests__/useCollapsed.test.ts` — **create**: hook unit tests.
- `src/styles/theme.css` — **modify**: restyle inbox-only `.cs-*` classes (bubble/conv/badge/tag) to the reference using A1 tokens.

---

## Task 1: Badge × Tag — add IG + Shopee platforms

**Files:**
- Modify: `src/components/channels.ts`
- Modify: `src/components/ChanGlyph.tsx`
- Modify: `src/pages/Inbox.tsx` (cast sites at lines ~118, ~502, ~1032)

- [ ] **Step 1: Extend CHANNELS + PLATFORM_MAP**

In `src/components/channels.ts`, replace the `CHANNELS` map and `PLATFORM_MAP` with (keep `ChannelDef` and `channelOf` as-is):

```ts
export const CHANNELS: Record<string, ChannelDef> = {
  chat:   { name: '線上即時聊天', short: 'Live Chat', color: 'var(--chat-blue)',     glyph: 'chat' },
  line:   { name: 'LINE',        short: 'LINE',       color: 'var(--brand-line)',    glyph: 'chat' },
  wa:     { name: 'WhatsApp',    short: 'WhatsApp',   color: 'var(--wa-green)',      glyph: 'phone' },
  fb:     { name: 'Messenger',   short: 'Messenger',  color: 'var(--brand-fb)',      glyph: 'chat' },
  ig:     { name: 'Instagram',   short: 'IG',         color: 'var(--brand-ig)',      glyph: 'chat' },
  shopee: { name: 'Shopee',      short: 'Shopee',     color: 'var(--brand-shopee)',  glyph: 'chat' },
}

const PLATFORM_MAP: Record<string, string> = {
  line:      'line',
  facebook:  'fb',
  messenger: 'fb',
  fb:        'fb',
  instagram: 'ig',
  ig:        'ig',
  shopee:    'shopee',
  whatsapp:  'wa',
  wa:        'wa',
  webchat:   'chat',
  chat:      'chat',
  livechat:  'chat',
}
```

- [ ] **Step 2: Widen ChanGlyph + IG gradient**

In `src/components/ChanGlyph.tsx`, replace the type union, the `GLYPH_LABEL` map, and the background so IG renders its gradient:

```tsx
export interface ChanGlyphProps {
  type: 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'
  size?: number
}

const GLYPH_LABEL: Record<string, string> = {
  chat: 'C',
  line: 'L',
  wa:   'W',
  fb:   'M',
  ig:   'IG',
  shopee: 'S',
}

export function ChanGlyph({ type, size = 18 }: ChanGlyphProps) {
  const c = CHANNELS[type] ?? CHANNELS.chat
  const label = GLYPH_LABEL[type] ?? 'C'
  const background = type === 'ig' ? 'var(--brand-ig-gradient)' : c.color
  return (
    <span
      style={{
        width: size,
        height: size,
        borderRadius: '50%',
        background,
        color: '#fff',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontSize: type === 'ig' ? size * 0.4 : size * 0.52,
        fontWeight: 700,
        fontFamily: 'var(--mono)',
        flexShrink: 0,
      }}
    >
      {label}
    </span>
  )
}
```

- [ ] **Step 3: Widen the three casts in Inbox.tsx**

In `src/pages/Inbox.tsx`, change every `as 'chat' | 'line' | 'wa' | 'fb'` to `as 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'`. There are three occurrences:
- line ~118: `const chanKey = channelOf(platform) as 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'`
- line ~502: `<ChanGlyph type={chanKey as 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'} size={17} />`
- line ~1032: `<ChanGlyph type={chanKey as 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'} size={14} />`

Run `grep -n "as 'chat' | 'line' | 'wa' | 'fb'$" src/pages/Inbox.tsx` first to confirm the exact sites, then widen each. Change nothing else.

- [ ] **Step 4: Verify build**

Run: `cd frontend && npm run build`
Expected: `tsc -b` + `vite build` succeed (the widened union type-checks the casts).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/channels.ts frontend/src/components/ChanGlyph.tsx frontend/src/pages/Inbox.tsx
git commit -m "feat(inbox): add Instagram + Shopee platform badges (badge×tag)"
```

---

## Task 2: useCollapsed hook + collapsible customer panel

**Files:**
- Create: `src/hooks/useCollapsed.ts`
- Test: `src/__tests__/useCollapsed.test.ts`
- Modify: `src/pages/Inbox.tsx` (wide-layout panel collapse + section accordion)

- [ ] **Step 1: Write the failing test**

Create `src/__tests__/useCollapsed.test.ts`:

```ts
import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import { useCollapsed } from '../hooks/useCollapsed'

describe('useCollapsed', () => {
  beforeEach(() => localStorage.clear())

  it('uses the default when nothing is stored', () => {
    const { result } = renderHook(() => useCollapsed('k1', true))
    expect(result.current[0]).toBe(true)
  })

  it('reads a stored value over the default', () => {
    localStorage.setItem('collapsed.k2', 'false')
    const { result } = renderHook(() => useCollapsed('k2', true))
    expect(result.current[0]).toBe(false)
  })

  it('toggle flips and persists', () => {
    const { result } = renderHook(() => useCollapsed('k3', true))
    act(() => result.current[1]())
    expect(result.current[0]).toBe(false)
    expect(localStorage.getItem('collapsed.k3')).toBe('false')
    act(() => result.current[1]())
    expect(result.current[0]).toBe(true)
    expect(localStorage.getItem('collapsed.k3')).toBe('true')
  })
})
```

- [ ] **Step 2: Check the test dependency, then run to verify it fails**

`@testing-library/react` provides `renderHook`. Check it is installed: `cd frontend && node -e "require.resolve('@testing-library/react')"`.
- If it prints a path: proceed.
- If it throws "Cannot find module": install it as a dev dependency — `cd frontend && npm i -D @testing-library/react` — then proceed.

Run: `cd frontend && npx vitest run src/__tests__/useCollapsed.test.ts`
Expected: FAIL — cannot resolve `../hooks/useCollapsed`.

- [ ] **Step 3: Implement the hook**

Create `src/hooks/useCollapsed.ts`:

```ts
import { useCallback, useState } from 'react'

// Persisted open/closed (collapsed) boolean keyed in localStorage under
// `collapsed.<key>`. Returns [collapsed, toggle]. `defaultCollapsed` applies
// only when nothing is stored yet.
export function useCollapsed(
  key: string,
  defaultCollapsed: boolean,
): [boolean, () => void] {
  const storageKey = `collapsed.${key}`
  const [collapsed, setCollapsed] = useState<boolean>(() => {
    const v = localStorage.getItem(storageKey)
    return v === null ? defaultCollapsed : v === 'true'
  })
  const toggle = useCallback(() => {
    setCollapsed((prev) => {
      const next = !prev
      localStorage.setItem(storageKey, String(next))
      return next
    })
  }, [storageKey])
  return [collapsed, toggle]
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/__tests__/useCollapsed.test.ts`
Expected: PASS — 3 tests.

- [ ] **Step 5: Wire the wide-layout panel collapse in Inbox.tsx**

The inbox already supports a panel toggle: `Thread` accepts `onToggleCustPanel` + `showCustToggle`, and the medium-width branch uses it for an overlay drawer. Reuse it for the WIDE (3-column) branch, backed by `useCollapsed`.

(a) Add the import at the top of `src/pages/Inbox.tsx` (with the other hook/util imports):
```tsx
import { useCollapsed } from '../hooks/useCollapsed'
```

(b) In the `Inbox()` component body (function starts at line ~1074), near the other `useState` hooks, add:
```tsx
const [custCollapsed, toggleCustCollapsed] = useCollapsed('inbox.custPanel', false)
```

(c) In the WIDE return (the final `return (` around line 1230, the `<div className="cs-inbox">` containing `ConvList` + `Thread` + `CustPanel`), change the `Thread` + `CustPanel` block to:
```tsx
      <Thread
        convId={selectedId}
        meta={meta}
        onMetaLoaded={handleMetaLoaded}
        onToggleCustPanel={toggleCustCollapsed}
        showCustToggle
      />
      {!custCollapsed && <CustPanel meta={meta} />}
```
(Do not change the medium-width overlay branch.)

- [ ] **Step 6: Wire the section accordion in the customer panel**

In the `CustPanel` component (renders `<div className="cs-cust">`, around lines 975-1070), make each `cs-cust-block-label` section header a button that toggles its body via `useCollapsed`. For EACH section (聯絡資訊, 統計, 標籤 — whichever `cs-cust-block-label` blocks exist), wrap as follows. Example for 聯絡資訊 (apply the same shape to the others, with distinct keys `inbox.cust.contact`, `inbox.cust.stats`, `inbox.cust.tags`):

```tsx
      {/* Contact info */}
      {(() => {
        const [secCollapsed, toggleSec] = useCollapsed('inbox.cust.contact', false)
        return (
          <div>
            <button
              className="cs-cust-block-label"
              onClick={toggleSec}
              style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', width: '100%', background: 'none', border: 'none', padding: 0, cursor: 'pointer' }}
            >
              聯絡資訊
              <Icon name={secCollapsed ? 'down' : 'up'} w={14} />
            </button>
            {!secCollapsed && (
              <>
                {/* ...the existing section body (cs-kv rows) unchanged... */}
              </>
            )}
          </div>
        )
      })()}
```

IMPORTANT: hooks cannot be called conditionally or in loops. Do NOT call `useCollapsed` inside `.map()` or an `if`. The IIFE pattern above is acceptable ONLY because each section is a fixed, top-level block rendered unconditionally (the IIFE runs once per render in source order). If `CustPanel` early-returns before these sections (it does, for the empty/overlay states), ensure the IIFE sections all live in the single main `return` after those early returns, so the hook call order is stable across renders. Read the component first and confirm the early returns precede the section blocks.

(Alternative if the IIFE feels risky: define a small `CollapsibleSection({ id, title, children })` component in the same file that calls `useCollapsed(id, false)` and renders the labelled toggle + body, then use `<CollapsibleSection id="inbox.cust.contact" title="聯絡資訊">…</CollapsibleSection>` for each. This keeps hook calls at the top level of a component and is the safer choice — prefer it.)

- [ ] **Step 7: Verify build + tests + manual**

Run: `cd frontend && npm run build && npx vitest run`
Expected: build clean; all suites pass (incl. useCollapsed).
Manual (`npm run dev`): at wide width, the thread-header toggle hides/shows the customer panel and the choice survives reload; each panel section collapses/expands and persists.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/hooks/useCollapsed.ts frontend/src/__tests__/useCollapsed.test.ts frontend/src/pages/Inbox.tsx frontend/package.json frontend/package-lock.json
git commit -m "feat(inbox): collapsible customer panel + section accordion (persisted)"
```

---

## Task 3: Visual restyle of inbox CSS

**Files:**
- Modify: `src/styles/theme.css` (the inbox-only `.cs-*` classes, currently around lines 351-417)

- [ ] **Step 1: Restyle the conversation rows, badges, tags, and bubbles**

These classes are used ONLY by the inbox. Update them to the reference using A1 tokens. Apply these specific changes (edit the existing rules in place; keep all other `.cs-*` rules unchanged):

- `.cs-conv-item`: keep layout; ensure the active accent bar uses `--primary` (`background: var(--blue-600)` is fine) and the unread dot uses the status token.
- `.cs-conv-unread`: change `background: var(--blue-500)` → `background: var(--status-unread)`.
- `.cs-tag`: set `border-radius: var(--r-sm)` and `padding: 3px 8px` and `font-size: 11px` (reference tag spec). Keep the inline color from `TAG_COLORS`.
- `.cs-chip`: set `border-radius: var(--r-sm)`.
- `.cs-conv-chan`: keep the 18px avatar-overlay badge; ensure `border: 2px solid var(--surface)` (already present) so it reads in dark mode too.
- `.cs-bubble`: set `border-radius: var(--r-lg)` (12px) with the tail corner `border-bottom-left-radius: var(--r-xs)`; `padding: 10px 14px`; `max-width: 60%` on the row container `.cs-bubble-row` (change `max-width: 74%` → `max-width: 60%`).
- `.cs-bubble--me`: `border-radius: var(--r-lg)` with `border-bottom-right-radius: var(--r-xs)`.

- [ ] **Step 2: Verify build + manual light/dark**

Run: `cd frontend && npm run build && npx vitest run`
Expected: build clean; tests still green.
Manual: bubbles/tags/badges match the reference; toggle dark mode — the inbox reads correctly in both (these classes consume neutral tokens that A1's dark block already overrides).

- [ ] **Step 3: Commit**

```bash
git add frontend/src/styles/theme.css
git commit -m "style(inbox): restyle conversation rows, badges, tags, bubbles to reference"
```

---

## Final verification (after all tasks)

- [ ] `cd frontend && npm run build` — clean
- [ ] `cd frontend && npx vitest run` — all suites green (useCollapsed + existing)
- [ ] Manual: 5 platform badges (chat/line/fb/ig/shopee) render with correct brand colors/gradient; customer panel collapses (whole + sections) and persists; light/dark both correct; conversation load/send/realtime behave exactly as before.
```

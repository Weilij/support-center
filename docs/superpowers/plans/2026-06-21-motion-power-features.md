# Motion & Power-User Features (Track A3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `/` canned-reply menu, keyboard shortcuts (⌘K / ⌘Enter / Esc), and a conversation-list reorder animation to the inbox, without changing existing logic.

**Architecture:** Three self-contained, unit-tested utilities (`lib/templates.ts`, `hooks/useHotkeys.ts`, `lib/flip.ts`) plus a presentational `SlashMenu`, wired into the existing `Inbox.tsx` composer/list and `AppShell` top level. Canned replies live in localStorage; animation honors `prefers-reduced-motion`.

**Tech Stack:** React 18 + TypeScript + Vite, vitest (jsdom) + @testing-library/react, CSS custom properties (A1 tokens).

**Spec:** `docs/superpowers/specs/2026-06-21-motion-power-features-design.md`

---

## File Structure

- `src/lib/templates.ts` — **create**: localStorage canned-reply store (pure API).
- `src/hooks/useTemplates.ts` — **create**: React wrapper over the store.
- `src/components/SlashMenu.tsx` — **create**: presentational filtered template popover.
- `src/components/TemplateManager.tsx` — **create**: add/edit/delete modal.
- `src/hooks/useHotkeys.ts` — **create**: `matchHotkey` + `useHotkeys` global key binding.
- `src/lib/flip.ts` — **create**: FLIP position-diff + animate helpers.
- `src/pages/Inbox.tsx` — **modify**: slash menu + manage modal + ⌘Enter in composer; FLIP + `data-inbox-search` in ConvList; Esc closes overlay panel.
- `src/components/AppShell.tsx` — **modify**: global ⌘K via `useHotkeys`.
- `src/styles/theme.css` — **modify**: slash-menu popover + conv-item enter animation (inbox-scoped).

---

## Task 1: `/` canned-reply menu

**Files:**
- Create: `src/lib/templates.ts`, `src/__tests__/templates.test.ts`, `src/hooks/useTemplates.ts`, `src/components/SlashMenu.tsx`, `src/components/TemplateManager.tsx`
- Modify: `src/pages/Inbox.tsx` (composer in the `Thread` component, ~lines 657-740)

- [ ] **Step 1: Write the failing store test**

Create `src/__tests__/templates.test.ts`:

```ts
import { beforeEach, describe, expect, it } from 'vitest'

import { addTemplate, listTemplates, removeTemplate, updateTemplate } from '../lib/templates'

describe('templates store', () => {
  beforeEach(() => localStorage.clear())

  it('seeds defaults on first read and persists them', () => {
    const list = listTemplates()
    expect(list.length).toBeGreaterThan(0)
    expect(localStorage.getItem('cannedReplies')).not.toBeNull()
  })

  it('adds, updates, and removes', () => {
    localStorage.setItem('cannedReplies', '[]')
    const t = addTemplate({ title: '問候', body: '您好，有什麼能幫您？' })
    expect(t.id).toBeTruthy()
    expect(listTemplates()).toHaveLength(1)

    updateTemplate(t.id, { body: '您好！' })
    expect(listTemplates()[0].body).toBe('您好！')

    removeTemplate(t.id)
    expect(listTemplates()).toHaveLength(0)
  })
})
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/__tests__/templates.test.ts`
Expected: FAIL — cannot resolve `../lib/templates`.

- [ ] **Step 3: Implement the store**

Create `src/lib/templates.ts`:

```ts
// Frontend-local canned replies, persisted in localStorage under `cannedReplies`.
// Backend agent-template API is deferred (Track B).

export interface Template {
  id: string
  title: string
  body: string
}

const KEY = 'cannedReplies'

const DEFAULTS: Template[] = [
  { id: 'seed-greet', title: '問候', body: '您好，很高興為您服務，請問有什麼能幫您的嗎？' },
  { id: 'seed-wait', title: '請稍候', body: '好的，請您稍候，我馬上為您查詢。' },
  { id: 'seed-thanks', title: '感謝', body: '感謝您的耐心等候！還有其他需要協助的地方嗎？' },
]

function read(): Template[] {
  const raw = localStorage.getItem(KEY)
  if (raw === null) {
    localStorage.setItem(KEY, JSON.stringify(DEFAULTS))
    return [...DEFAULTS]
  }
  try {
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed) ? (parsed as Template[]) : []
  } catch {
    return []
  }
}

function write(list: Template[]): void {
  localStorage.setItem(KEY, JSON.stringify(list))
}

export function listTemplates(): Template[] {
  return read()
}

export function addTemplate(input: { title: string; body: string }): Template {
  const t: Template = { id: `t-${Date.now()}-${Math.floor(Math.random() * 1e6)}`, ...input }
  write([...read(), t])
  return t
}

export function updateTemplate(id: string, patch: Partial<Omit<Template, 'id'>>): void {
  write(read().map((t) => (t.id === id ? { ...t, ...patch } : t)))
}

export function removeTemplate(id: string): void {
  write(read().filter((t) => t.id !== id))
}
```

(Note: `Date.now()`/`Math.random()` run in the browser at click time — fine for a local id. The test sets `cannedReplies` to `'[]'` first so seeding doesn't interfere.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/__tests__/templates.test.ts`
Expected: PASS — 2 tests.

- [ ] **Step 5: Create the React wrapper hook**

Create `src/hooks/useTemplates.ts`:

```ts
import { useCallback, useState } from 'react'

import { addTemplate, listTemplates, removeTemplate, updateTemplate, type Template } from '../lib/templates'

// Re-renders consumers after each mutation by re-reading the store.
export function useTemplates() {
  const [list, setList] = useState<Template[]>(() => listTemplates())
  const refresh = useCallback(() => setList(listTemplates()), [])
  return {
    list,
    add: useCallback((input: { title: string; body: string }) => { addTemplate(input); refresh() }, [refresh]),
    update: useCallback((id: string, patch: Partial<Omit<Template, 'id'>>) => { updateTemplate(id, patch); refresh() }, [refresh]),
    remove: useCallback((id: string) => { removeTemplate(id); refresh() }, [refresh]),
  }
}
```

- [ ] **Step 6: Create the SlashMenu component**

Create `src/components/SlashMenu.tsx`:

```tsx
import type { Template } from '../lib/templates'

// Presentational filtered template list shown above the composer when the draft
// starts with '/'. The parent owns filtering, the active index, and key handling.
export function SlashMenu({
  templates,
  activeIndex,
  onPick,
}: {
  templates: Template[]
  activeIndex: number
  onPick: (t: Template) => void
}) {
  if (templates.length === 0) return null
  return (
    <div
      className="cs-slash-menu"
      role="listbox"
      aria-label="罐頭回覆"
    >
      {templates.map((t, i) => (
        <button
          key={t.id}
          type="button"
          role="option"
          aria-selected={i === activeIndex}
          className={`cs-slash-item${i === activeIndex ? ' cs-slash-item--active' : ''}`}
          onMouseDown={(e) => { e.preventDefault(); onPick(t) }}
        >
          <span className="cs-slash-title">{t.title}</span>
          <span className="cs-slash-body">{t.body}</span>
        </button>
      ))}
    </div>
  )
}
```

- [ ] **Step 7: Create the TemplateManager modal**

Create `src/components/TemplateManager.tsx`:

```tsx
import { useState } from 'react'

import { useTemplates } from '../hooks/useTemplates'
import { Modal } from './Modal'

// Add/edit/delete canned replies. Opened from the composer's 快捷回覆 button.
export function TemplateManager({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { list, add, update, remove } = useTemplates()
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')

  return (
    <Modal open={open} title="管理罐頭回覆" onClose={onClose} width={480}>
      <div style={{ display: 'grid', gap: 8, marginBottom: 16 }}>
        {list.map((t) => (
          <div key={t.id} style={{ display: 'flex', gap: 8, alignItems: 'flex-start' }}>
            <input
              value={t.title}
              onChange={(e) => update(t.id, { title: e.target.value })}
              style={{ width: 120, flexShrink: 0 }}
            />
            <textarea
              value={t.body}
              onChange={(e) => update(t.id, { body: e.target.value })}
              rows={2}
              style={{ flex: 1 }}
            />
            <button type="button" onClick={() => remove(t.id)} aria-label="刪除">✕</button>
          </div>
        ))}
      </div>
      <div style={{ display: 'flex', gap: 8, alignItems: 'flex-start', borderTop: '1px solid var(--line)', paddingTop: 12 }}>
        <input placeholder="標題" value={title} onChange={(e) => setTitle(e.target.value)} style={{ width: 120, flexShrink: 0 }} />
        <textarea placeholder="內容" value={body} onChange={(e) => setBody(e.target.value)} rows={2} style={{ flex: 1 }} />
        <button
          type="button"
          className="cs-btn cs-btn--primary"
          disabled={!title.trim() || !body.trim()}
          onClick={() => { add({ title: title.trim(), body: body.trim() }); setTitle(''); setBody('') }}
        >
          新增
        </button>
      </div>
    </Modal>
  )
}
```

- [ ] **Step 8: Wire the composer in Inbox.tsx (Thread component)**

READ `src/pages/Inbox.tsx` around the `Thread` component (composer at lines ~657-740). The composer uses `draft`/`setDraft` and a textarea with an `onKeyDown` that sends on Enter. Make these edits:

(a) Add imports at the top of the file (with other component/hook imports):
```tsx
import { SlashMenu } from '../components/SlashMenu'
import { TemplateManager } from '../components/TemplateManager'
import { useTemplates } from '../hooks/useTemplates'
```

(b) Inside the `Thread` component body (near its other `useState`), add:
```tsx
const { list: templates } = useTemplates()
const [slashIndex, setSlashIndex] = useState(0)
const [mgrOpen, setMgrOpen] = useState(false)
const slashOpen = draft.startsWith('/')
const slashQuery = slashOpen ? draft.slice(1).toLowerCase() : ''
const slashMatches = slashOpen
  ? templates.filter((t) => t.title.toLowerCase().includes(slashQuery) || t.body.toLowerCase().includes(slashQuery))
  : []
```

(c) Replace the textarea's `onKeyDown` with a version that handles the slash menu first:
```tsx
onKeyDown={(e) => {
  if (slashOpen && slashMatches.length > 0) {
    if (e.key === 'ArrowDown') { e.preventDefault(); setSlashIndex((i) => (i + 1) % slashMatches.length); return }
    if (e.key === 'ArrowUp') { e.preventDefault(); setSlashIndex((i) => (i - 1 + slashMatches.length) % slashMatches.length); return }
    if (e.key === 'Enter') { e.preventDefault(); setDraft(slashMatches[Math.min(slashIndex, slashMatches.length - 1)].body); setSlashIndex(0); return }
    if (e.key === 'Escape') { e.preventDefault(); setDraft(''); return }
  }
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    void send(e as unknown as React.FormEvent)
  }
}}
```

(d) Render the `SlashMenu` just inside the `cs-composer-box` (above the textarea), only when open. Place it right after the opening `<div className="cs-composer-box" …>` (and its drag overlay), before the `<textarea …>`:
```tsx
{slashOpen && slashMatches.length > 0 && (
  <SlashMenu templates={slashMatches} activeIndex={Math.min(slashIndex, slashMatches.length - 1)} onPick={(t) => { setDraft(t.body); setSlashIndex(0) }} />
)}
```

(e) Wire the 快捷回覆 button (the one with `aria-label="快捷回覆"`, currently no onClick) to open the manager, and render the manager at the end of the composer block:
```tsx
<button type="button" className="cs-composer-ico" aria-label="快捷回覆" onClick={() => setMgrOpen(true)}>
  <Icon name="zap" w={20} />
</button>
```
and after the composer `</form>` (before the closing `</div>` of `.cs-composer`):
```tsx
<TemplateManager open={mgrOpen} onClose={() => setMgrOpen(false)} />
```

- [ ] **Step 9: Add SlashMenu CSS**

In `src/styles/theme.css`, append (inbox-scoped):
```css
/* Slash command menu (composer canned replies) */
.cs-slash-menu {
  position: absolute; left: 0; right: 0; bottom: calc(100% + 6px);
  background: var(--surface); border: 1px solid var(--line);
  border-radius: var(--r-md); box-shadow: var(--elevation-2);
  max-height: 240px; overflow-y: auto; z-index: 20; padding: 4px;
}
.cs-slash-item {
  display: flex; flex-direction: column; gap: 2px; width: 100%; text-align: left;
  background: none; border: none; border-radius: var(--r-sm); padding: 8px 10px; cursor: pointer;
}
.cs-slash-item--active, .cs-slash-item:hover { background: var(--primary-tint, var(--blue-50)); }
.cs-slash-title { font-size: 13px; font-weight: 600; color: var(--ink); }
.cs-slash-body { font-size: 12px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
```
The `.cs-composer-box` must be a positioning context for the absolute menu — ensure it has `position: relative` (the drag style already sets it conditionally; add `position: relative` to the base `.cs-composer-box` rule in theme.css if not present).

- [ ] **Step 10: Verify + commit**

Run: `cd frontend && npm run build && npx vitest run`
Expected: build clean; all suites pass (templates + existing).
Manual: type `/` in the composer → menu with seeded templates; type `/問` → filters; ArrowDown/Enter inserts; Esc clears; 快捷回覆 opens the manager (add/edit/delete persists across reload).

```bash
git add frontend/src/lib/templates.ts frontend/src/__tests__/templates.test.ts frontend/src/hooks/useTemplates.ts frontend/src/components/SlashMenu.tsx frontend/src/components/TemplateManager.tsx frontend/src/pages/Inbox.tsx frontend/src/styles/theme.css
git commit -m "feat(inbox): slash canned-reply menu + template manager"
```

---

## Task 2: Keyboard shortcuts (⌘K / ⌘Enter / Esc)

**Files:**
- Create: `src/hooks/useHotkeys.ts`, `src/__tests__/useHotkeys.test.ts`
- Modify: `src/components/AppShell.tsx` (mount ⌘K), `src/pages/Inbox.tsx` (⌘Enter in composer; `data-inbox-search` on the conv-list search input; Esc closes overlay panel)

- [ ] **Step 1: Write the failing matcher test**

Create `src/__tests__/useHotkeys.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { matchHotkey } from '../hooks/useHotkeys'

function ev(key: string, mods: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }> = {}) {
  return { key, metaKey: false, ctrlKey: false, shiftKey: false, ...mods } as KeyboardEvent
}

describe('matchHotkey', () => {
  it('matches mod+k on meta or ctrl', () => {
    expect(matchHotkey(ev('k', { metaKey: true }), 'mod+k')).toBe(true)
    expect(matchHotkey(ev('k', { ctrlKey: true }), 'mod+k')).toBe(true)
  })
  it('does not match without the modifier', () => {
    expect(matchHotkey(ev('k'), 'mod+k')).toBe(false)
  })
  it('matches mod+enter and is case-insensitive on the key', () => {
    expect(matchHotkey(ev('Enter', { metaKey: true }), 'mod+enter')).toBe(true)
    expect(matchHotkey(ev('K', { ctrlKey: true }), 'mod+k')).toBe(true)
  })
})
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npx vitest run src/__tests__/useHotkeys.test.ts`
Expected: FAIL — cannot resolve `../hooks/useHotkeys`.

- [ ] **Step 3: Implement matchHotkey + useHotkeys**

Create `src/hooks/useHotkeys.ts`:

```ts
import { useEffect } from 'react'

// Combo grammar: optional "mod+" prefix (meta on mac / ctrl elsewhere) + key name
// (case-insensitive), e.g. "mod+k", "mod+enter".
export function matchHotkey(e: KeyboardEvent, combo: string): boolean {
  const parts = combo.toLowerCase().split('+')
  const key = parts[parts.length - 1]
  const needMod = parts.includes('mod')
  const hasMod = e.metaKey || e.ctrlKey
  if (needMod !== hasMod) return false
  return e.key.toLowerCase() === key
}

// Binds a single keydown listener for the given combo→handler map.
export function useHotkeys(map: Record<string, (e: KeyboardEvent) => void>): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      for (const [combo, handler] of Object.entries(map)) {
        if (matchHotkey(e, combo)) { handler(e); return }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [map])
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/__tests__/useHotkeys.test.ts`
Expected: PASS — 3 tests.

- [ ] **Step 5: Mount ⌘K in AppShell**

In `src/components/AppShell.tsx`: add imports `import { useHotkeys } from '../hooks/useHotkeys'` and `useNavigate` from `react-router-dom` (if not already imported). Inside the AppShell component body, add:
```tsx
const navigate = useNavigate()
useHotkeys({
  'mod+k': (e) => {
    e.preventDefault()
    const search = document.querySelector<HTMLInputElement>('[data-inbox-search]')
    if (search) search.focus()
    else navigate('/messages/search')
  },
})
```
(If `useNavigate` is already imported/used, reuse the existing `navigate`.)

- [ ] **Step 6: Tag the inbox search input + ⌘Enter + Esc-closes-panel in Inbox.tsx**

In `src/pages/Inbox.tsx`:

(a) On the conv-list search `<input type="search" …>` (in `ConvList`, ~line 223), add the attribute `data-inbox-search` so ⌘K can find it:
```tsx
<input
  type="search"
  data-inbox-search
  value={search}
  …
/>
```

(b) In the composer textarea `onKeyDown` (edited in Task 1 step 8c), add ⌘Enter send. Right after the slash-menu block and before the plain-Enter block, insert:
```tsx
  if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
    e.preventDefault()
    void send(e as unknown as React.FormEvent)
    return
  }
```

(c) The overlay customer panel (the medium/narrow branch, `custPanelOpen` state) should close on Esc. In the `Inbox()` component, add:
```tsx
useHotkeys({ 'escape': () => setCustPanelOpen(false) })
```
Add `import { useHotkeys } from '../hooks/useHotkeys'` if not already imported (it may be added in Task 1). (Note: `matchHotkey` treats `'escape'` as no-mod + key `escape`; pressing Esc with no modifier matches.) The `Modal` already closes itself on Esc; the slash menu closes via its own Esc (Task 1). This covers the overlay panel.

- [ ] **Step 7: Verify + commit**

Run: `cd frontend && npm run build && npx vitest run`
Expected: build clean; all suites green (useHotkeys + existing).
Manual: ⌘K focuses the conversation search (and from a non-inbox page navigates to /messages/search); ⌘Enter sends; Esc closes the overlay customer panel / the slash menu / an open modal.

```bash
git add frontend/src/hooks/useHotkeys.ts frontend/src/__tests__/useHotkeys.test.ts frontend/src/components/AppShell.tsx frontend/src/pages/Inbox.tsx
git commit -m "feat(inbox): keyboard shortcuts ⌘K / ⌘Enter / Esc"
```

---

## Task 3: Conversation-list reorder animation (FLIP)

**Files:**
- Create: `src/lib/flip.ts`, `src/__tests__/flip.test.ts`
- Modify: `src/pages/Inbox.tsx` (ConvList FLIP), `src/styles/theme.css` (enter fade)

- [ ] **Step 1: Write the failing diff test**

Create `src/__tests__/flip.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { movedIds } from '../lib/flip'

describe('movedIds', () => {
  it('returns ids whose top changed between snapshots', () => {
    const prev = new Map([['a', 0], ['b', 60], ['c', 120]])
    const next = new Map([['a', 60], ['b', 0], ['c', 120]])
    expect(movedIds(prev, next).sort()).toEqual(['a', 'b'])
  })
  it('ignores ids missing from either snapshot', () => {
    const prev = new Map([['a', 0]])
    const next = new Map([['a', 0], ['d', 60]])
    expect(movedIds(prev, next)).toEqual([])
  })
})
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npx vitest run src/__tests__/flip.test.ts`
Expected: FAIL — cannot resolve `../lib/flip`.

- [ ] **Step 3: Implement flip.ts**

Create `src/lib/flip.ts`:

```ts
// FLIP (First-Last-Invert-Play) helpers for the conversation list. The pure
// position-diff (`movedIds`) is unit-tested; `animateMoves` does the DOM play.

export type PosMap = Map<string, number>

// Records each element's top offset, keyed by its data-flip-id.
export function recordPositions(container: HTMLElement): PosMap {
  const map: PosMap = new Map()
  container.querySelectorAll<HTMLElement>('[data-flip-id]').forEach((el) => {
    map.set(el.dataset.flipId!, el.getBoundingClientRect().top)
  })
  return map
}

// Ids present in both snapshots whose top changed.
export function movedIds(prev: PosMap, next: PosMap): string[] {
  const ids: string[] = []
  for (const [id, top] of next) {
    if (prev.has(id) && prev.get(id) !== top) ids.push(id)
  }
  return ids
}

// Inverts each moved element to its old position then transitions to zero.
export function animateMoves(container: HTMLElement, prev: PosMap, durationMs = 220): void {
  const next = recordPositions(container)
  for (const id of movedIds(prev, next)) {
    const el = container.querySelector<HTMLElement>(`[data-flip-id="${id}"]`)
    if (!el) continue
    const delta = (prev.get(id) ?? 0) - (next.get(id) ?? 0)
    el.style.transition = 'none'
    el.style.transform = `translateY(${delta}px)`
    // Force reflow so the inverted transform applies before the play frame.
    void el.offsetHeight
    el.style.transition = `transform ${durationMs}ms cubic-bezier(.22,.61,.36,1)`
    el.style.transform = ''
  }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/__tests__/flip.test.ts`
Expected: PASS — 2 tests.

- [ ] **Step 5: Wire FLIP into ConvList**

In `src/pages/Inbox.tsx`, `ConvList` (renders the list at ~line 259-274):

(a) Add imports: `import { useLayoutEffect, useRef } from 'react'` (extend the existing react import) and `import { recordPositions, animateMoves } from '../lib/flip'`.

(b) Add a `data-flip-id` to each `ConvItem`'s root element. In the `ConvItem` component (~line 124, the `<div className={`cs-conv-item…`}>`), add `data-flip-id={conv.id}` to that div.

(c) In `ConvList`, add a ref to the scroll container and a layout effect keyed on the ordered ids:
```tsx
const listRef = useRef<HTMLDivElement>(null)
const prevPos = useRef<ReturnType<typeof recordPositions> | null>(null)
const orderKey = filtered.map((c) => c.id).join(',')
useLayoutEffect(() => {
  const reduce = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
  if (!reduce && listRef.current && prevPos.current) {
    animateMoves(listRef.current, prevPos.current)
  }
  if (listRef.current) prevPos.current = recordPositions(listRef.current)
}, [orderKey])
```
Attach `ref={listRef}` to the list scroll container `<div style={{ flex: 1, overflowY: 'auto' }}>` (line 259).

(d) For the new-row fade, add `className` on `ConvItem` is unnecessary; the fade is handled by the CSS rule in Step 6 applied to freshly mounted items via an animation on `.cs-conv-item`. Keep it simple: the CSS in Step 6 gives every newly mounted `.cs-conv-item` a brief fade-in; React only mounts genuinely new rows, so existing rows won't re-fade (they keep their DOM node across reorders because of the stable `key={c.id}`).

- [ ] **Step 6: Add the enter-fade CSS**

In `src/styles/theme.css`, append:
```css
/* New conversation row fade-in (respects reduced motion via the media query) */
@media (prefers-reduced-motion: no-preference) {
  .cs-conv-item { animation: cs-conv-enter .18s ease-out; }
  @keyframes cs-conv-enter { from { opacity: 0; transform: translateY(-4px); } to { opacity: 1; transform: none; } }
}
```

- [ ] **Step 7: Verify + commit**

Run: `cd frontend && npm run build && npx vitest run`
Expected: build clean; all suites green (flip + existing).
Manual: when a conversation jumps to the top (e.g. a new message, or switching tabs that reorders), rows glide smoothly and a genuinely new row fades in; enabling OS "reduce motion" disables the slide.

```bash
git add frontend/src/lib/flip.ts frontend/src/__tests__/flip.test.ts frontend/src/pages/Inbox.tsx frontend/src/styles/theme.css
git commit -m "feat(inbox): FLIP conversation-list reorder animation"
```

---

## Final verification (after all tasks)

- [ ] `cd frontend && npm run build` — clean
- [ ] `cd frontend && npx vitest run` — all suites green (templates + useHotkeys + flip + existing)
- [ ] Manual: `/` menu filters + inserts + manager persists; ⌘K / ⌘Enter / Esc work; list animates on reorder and respects reduced-motion; light + dark correct; conversation load/send/realtime unchanged.
```

# Motion & Power-User Features (Skywork Part 3) — Design Spec

**Date:** 2026-06-21
**Track:** A③ (frontend, third sub-project of the Skywork redesign)
**Depends on:** A① tokens (merged), A② inbox restyle (merged)
**Status:** design approved, pending written-spec review

---

## 0. Context

Reference Part 3 ("高效小編工作流微互動") adds power-user/motion polish to the inbox: a `/` quick-command menu in the composer, keyboard shortcuts, and a new-message list-reorder animation. The current inbox (`pages/Inbox.tsx`) has a composer with Enter-to-send + a "快捷回覆" button, a per-list search, and a `MessageSearch` page — but **no slash menu, no global shortcut layer, and no list animation**. There is **no backend agent-template data** (only auto-reply rules / report templates), so canned replies are frontend-local.

**Guiding rule:** style/behavior per the reference; do not rewrite existing working logic. These are net-new additive features.

---

## 1. Goal & non-goals

**Goal:** the inbox gains a `/` canned-reply menu (frontend-local), the shortcuts ⌘K / ⌘Enter / Esc, and a conversation-list reorder animation.

**Non-goals:**
- No backend changes (canned replies live in localStorage).
- No ↑/↓ list navigation (not selected); no shortcut help/cheatsheet panel (YAGNI).
- No changes to conversation loading / sending / realtime logic beyond wiring these features in.
- No restyle beyond what these features need (A② already restyled the inbox).

---

## 2. Feature 1 — `/` command menu (canned replies)

**Store — `src/lib/templates.ts`:** localStorage-backed canned replies under key `cannedReplies`. Seeded with a few defaults on first use. Pure API:
- `listTemplates(): Template[]` — read (seed + persist defaults if empty).
- `addTemplate(t: {title: string; body: string}): Template` — append with a generated id, persist.
- `updateTemplate(id, patch): void` / `removeTemplate(id): void` — persist.
- `Template = { id: string; title: string; body: string }`.
A `useTemplates()` hook wraps the store for React (returns list + mutators, re-renders on change).

**`SlashMenu` Popover — `src/components/SlashMenu.tsx`:** given the current composer text and caret, when the text starts with `/`, show a Popover listing templates filtered by the substring after `/` (match title or body, case-insensitive). Arrow keys move selection; Enter/click inserts the selected template's `body`, replacing the `/query` token in the composer; Esc or blur closes. Styled with A① tokens (`--elevation-2`, `--r-md`, status/neutral colors) per the reference Popover spec.

**Manage UI (D1):** the existing composer **"快捷回覆" button** (`cs-composer-ico`, aria-label="快捷回覆") opens a small `Modal` listing templates with add / edit / delete (title + body fields). No new entry point in the menu.

**Composer wiring (`pages/Inbox.tsx`):** detect a leading `/` in the composer textarea value to toggle the `SlashMenu`; on insert, set the textarea value to the template body and close. No change to the send path itself.

---

## 3. Feature 2 — Keyboard shortcuts (⌘K / ⌘Enter / Esc)

**`src/hooks/useHotkeys.ts`:** a small hook taking a map of `{ combo: handler }` and binding a single `keydown` listener (cleaned up on unmount). A pure `matchHotkey(event, combo)` helper (e.g. combo `"mod+k"`, where `mod` = metaKey on mac / ctrlKey elsewhere) is exported and unit-tested.

- **⌘K / Ctrl+K** (global, mounted in `AppShell`): if the inbox conversation search input exists in the DOM, focus it; otherwise (D2) navigate to `/messages/search`. `preventDefault()`.
- **Esc** (global, in `AppShell`): close the topmost transient layer — dispatch a documented close order: `/` menu → overlay customer panel → open `Modal`. Implemented by each layer listening for Esc itself where it already can (Modal already handles Esc); the global handler covers the inbox overlay/slash-menu via state.
- **⌘Enter / Ctrl+Enter** (composer-local in `Inbox.tsx`): in the composer textarea `onKeyDown`, send when `(metaKey||ctrlKey) && Enter`, alongside the existing plain-Enter send; Shift+Enter still inserts a newline.

Plain typing and existing inputs are unaffected (⌘K only fires with the modifier).

---

## 4. Feature 3 — List-reorder animation (reference §3.1)

**`src/lib/flip.ts`:** a tiny FLIP helper — `recordPositions(elements): Map<id, DOMRect>` and `animateFromPositions(elements, prev)` that, for each element whose position changed, applies an inverted transform then transitions it to zero (the "First-Last-Invert-Play" pattern). Pure-ish DOM helper; the position-diff math is unit-tested with synthetic rects.

**Wiring (`pages/Inbox.tsx`):** the `ConvList` records `.cs-conv-item` positions (keyed by conv id) before each items-array change and runs the FLIP after the DOM updates (via `useLayoutEffect` on the ordered id list), so a conversation bumped to the top slides into place and a new row fades in. Honor `prefers-reduced-motion` — when set, skip the animation entirely.

**CSS (`styles/theme.css`):** add the transition/keyframes the FLIP/new-row fade use (e.g. a `.cs-conv-item--enter` fade), inbox-scoped.

---

## 5. Files

- **Add:** `src/lib/templates.ts` (+ `__tests__/templates.test.ts`), `src/components/SlashMenu.tsx`, `src/hooks/useHotkeys.ts` (+ `__tests__/useHotkeys.test.ts`), `src/lib/flip.ts` (+ `__tests__/flip.test.ts`).
- **Modify:** `src/pages/Inbox.tsx` (slash menu + manage modal trigger + ⌘Enter + conv-list FLIP), `src/components/AppShell.tsx` (global ⌘K / Esc via `useHotkeys`), `src/styles/theme.css` (slash-menu Popover + conv-item animation, inbox-scoped).

---

## 6. Verification

- `npm run build` (tsc -b + vite) clean; `vitest` green incl. new unit tests:
  - `templates`: seeds defaults, add/update/remove persist to localStorage.
  - `matchHotkey`: `mod+k`/`mod+enter` match on meta/ctrl, ignore without modifier.
  - `flip`: position-diff detects moved elements from synthetic rects.
- Manual: typing `/` opens the filterable menu and inserts a template; 快捷回覆 button opens the manage modal (add/edit/delete persists); ⌘K focuses search (and navigates off-inbox); ⌘Enter sends; Esc closes the menu/panel/modal in order; a new inbound message animates the conversation to the top; `prefers-reduced-motion` disables the animation; light + dark both correct; sending/loading/realtime unchanged.

---

## 7. Resolved decisions
- **D1** — manage-templates entry: reuse the composer **"快捷回覆" button** → manage modal.
- **D2** — ⌘K off the inbox: navigate to `/messages/search`.
- Canned replies are **frontend-local** (localStorage), seeded with defaults; backend template API deferred to Track B.
- Three stages (slash menu / shortcuts / animation), each subagent-built, reviewed, committed.

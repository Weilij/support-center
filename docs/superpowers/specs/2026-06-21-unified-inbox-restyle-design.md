# Unified Inbox Restyle + Badge×Tag + Collapsible Panel — Design Spec

**Date:** 2026-06-21
**Track:** A② (frontend, second sub-project of the Skywork redesign)
**Depends on:** A① Foundation tokens (merged — platform brand, semantic, `--r-*`, type tokens, dark mode)
**Status:** design approved, pending written-spec review

---

## 0. Context

The 3-column Unified Inbox **already exists** in `frontend/src/pages/Inbox.tsx` (~1252 lines): conversation list (`.cs-conv-list`) │ thread (`.cs-thread`) │ customer panel (`.cs-cust`), with `ConvItem`/`ConvList` sub-components, `ChanGlyph` channel badges, `Tag` chips, and message bubbles (`.cs-bubble`). Track A② **restyles** that existing inbox to the Skywork reference using A① tokens and **adds the two design features it lacks**: the formalized **badge × tag** combo (reference §2.1) and the **collapsible customer panel** (reference §2.3).

**Guiding rule:** style/layout reference — do **not** change existing logic or data flow; **may add** the net-new UX the design introduces. No file split. No Part-3 features (`/` command, keyboard shortcuts, motion → Track A③).

---

## 1. Goal & non-goals

**Goal:** the inbox visually matches the reference (bubbles, badges, tags, spacing, dark mode) and gains a badge × tag treatment for all 5 platforms plus a collapsible/accordion customer panel whose state persists.

**Non-goals:**
- No changes to conversation loading, sending, realtime, or any handler logic.
- No splitting `Inbox.tsx` into per-column files (deferred).
- No `/` command menu, keyboard shortcuts, or list-reorder animation (Track A③).
- No backend changes (IG/Shopee have no backend data yet — added visually, ready for data).

---

## 2. Current state (verified)

- `components/ChanGlyph.tsx`: round letter badge; prop `type: 'chat' | 'line' | 'wa' | 'fb'`; color from `CHANNELS[type].color`; `GLYPH_LABEL` maps type→letter.
- `components/channels.ts`: `CHANNELS` (chat/line/wa/fb, each `{name, short, color, glyph}`); `channelOf(platform)` maps backend strings via `PLATFORM_MAP` (line/facebook/messenger/whatsapp/webchat…) → channel key, default `'chat'`.
- `components/Chip.tsx`: `Tag({label})` colors from `TAG_COLORS` (訂單/優惠/退換貨/客訴/會員/已結案/運送中), neutral fallback; `.cs-tag` class.
- `pages/Inbox.tsx`: `ConvItem` renders avatar + `cs-conv-chan` channel overlay (`ChanGlyph`) + name + time + preview + `Tag`s; customer panel (`.cs-cust`) always shown with `cs-cust-block-label` sections (聯絡資訊 / 統計 / 標籤). Several spots cast `chanKey as 'chat'|'line'|'wa'|'fb'`.
- `styles/theme.css`: inbox-only classes `.cs-inbox/.cs-conv-list/.cs-conv-item/.cs-conv-chan/.cs-thread/.cs-bubble/.cs-cust/.cs-tag/.cs-chip` etc. (used only by the inbox).

---

## 3. Feature 1 — Badge × Tag (reference §2.1)

**Platform badge** (stays as the avatar overlay `.cs-conv-chan`, restyled):
- Extend `ChannelDef` usage so `CHANNELS` gains `ig` and `shopee`:
  - `ig`: `{ name: 'Instagram', short: 'IG', color: '#E1306C', glyph: 'chat' }` — but the badge background uses `var(--brand-ig-gradient)` (gradient), so `ChanGlyph` special-cases IG to a gradient background.
  - `shopee`: `{ name: 'Shopee', short: 'Shopee', color: 'var(--brand-shopee)', glyph: 'chat' }`.
  - Existing line/fb colors switch to the A① brand tokens (`--brand-line`, `--brand-fb`) for consistency (same hue, formalized).
- Widen `ChanGlyph` prop type to `'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'`; add `GLYPH_LABEL` entries (`ig: 'IG'`, `shopee: 'S'`); when `type === 'ig'`, use `background: var(--brand-ig-gradient)` instead of the solid color.
- Extend `PLATFORM_MAP` in `channels.ts`: `instagram → 'ig'`, `ig → 'ig'`, `shopee → 'shopee'`.
- Update the `chanKey as 'chat'|'line'|'wa'|'fb'` casts in `Inbox.tsx` to the widened union (so ig/shopee type-check).

**Tags** (keep `Tag`/`TAG_COLORS`): restyle `.cs-tag` to the reference (radius via `--r-sm`, padding/size per the badge/tag spec). Conversation rows show the platform badge (avatar) + tag chips together — the "combo".

---

## 4. Feature 2 — Collapsible customer panel (reference §2.3)

- **Whole-panel toggle:** a collapse button in the thread header (`.cs-thread-head`) hides/shows the `.cs-cust` panel (thread widens when hidden). State persisted globally in `localStorage` under one key (e.g. `inbox.custPanel`).
- **Section accordion:** the panel's section blocks (聯絡資訊 / 統計 / 標籤) become individually collapsible by clicking the `cs-cust-block-label`; each section's open state persisted (e.g. `inbox.custSection.<name>`).
- **`useCollapsed(key, defaultOpen)` hook** (`hooks/useCollapsed.ts`): returns `[open, toggle]`, reads/writes `localStorage`. Single responsibility; unit-tested (jsdom localStorage). Used by both the whole-panel toggle and each section.

---

## 5. Feature 3 — Visual restyle (reference §2.1–2.2)

Adjust the inbox-only `.cs-*` classes in `styles/theme.css` to the reference, using A① tokens (these classes are used only by the inbox, so no cross-page impact):
- **Bubbles** (`.cs-bubble`, `.cs-bubble--me`): radius and padding per the reference bubble spec (outer radius `--r-lg`/16, tail corner small; max-width ~60% of the thread; row gap per spec).
- **Conversation item** (`.cs-conv-item`, `.cs-conv-*`): spacing, the active-row accent, unread dot using `--status-unread`.
- **Badge/Tag** (`.cs-conv-chan`, `.cs-tag`, `.cs-chip`): sizing/radius per the badge/tag spec.
- Verify both light and dark render correctly (dark already overrides the neutral tokens these classes consume).

---

## 6. Files

- **Modify** `components/channels.ts` — add ig/shopee to `CHANNELS`; extend `PLATFORM_MAP`; brand-token colors.
- **Modify** `components/ChanGlyph.tsx` — widen `type` union; add labels; IG gradient background.
- **Modify** `components/Chip.tsx` — `.cs-tag` restyle if any inline style needs it (likely CSS-only).
- **Modify** `pages/Inbox.tsx` — panel collapse toggle + section accordion wiring; update the `chanKey` casts to the widened union. No logic changes.
- **Add** `hooks/useCollapsed.ts` + `__tests__/useCollapsed.test.ts`.
- **Modify** `styles/theme.css` — restyle the inbox-only `.cs-*` classes (badge/tag/bubble/spacing).

---

## 7. Verification

- `npm run build` (tsc -b + vite) clean; `npx vitest run` green incl. a new `useCollapsed` test (default open, toggle flips + persists, re-read from localStorage).
- Manual: all 5 platform badges render (chat/line/fb/ig/shopee) with correct brand colors/gradient; tags show colored; the whole customer panel collapses/expands and each section accordions; both states survive reload; light + dark both correct; sending/loading/realtime behave exactly as before (no logic touched).

---

## 8. Resolved decisions
- Platform badge stays the **avatar overlay** (`.cs-conv-chan`), restyled — not an inline row pill.
- Inbox-only `.cs-*` classes may be **adjusted in place**.
- IG + Shopee added **visually now** (ready for backend Track B data).
- Panel collapse: **whole-panel toggle + per-section accordion**, persisted to localStorage.

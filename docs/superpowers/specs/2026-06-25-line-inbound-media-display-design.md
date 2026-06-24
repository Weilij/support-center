# LINE Inbound Media Display — Design Spec

**Date:** 2026-06-25
**Track:** inbox media rendering (backend proxy + frontend)
**Status:** design approved, pending written-spec review

---

## 0. Context

Inbound LINE messages of kind image/video/audio/file/sticker/location are ingested
and stored: `messages.content_type` holds the kind, `messages.platform_message_id`
holds the LINE message id, and the normalized media descriptor is in
`messages.metadata.media` (`backend/src/domain/webhooks/parse.rs::normalize_line`).
The conversation message API (`conversations/handlers.rs::message_view`) already
returns `messageType` (= `content_type`) and `metadata` (including `media`) to the
frontend.

Two gaps make media invisible in the inbox:
1. **The frontend renders only `msg.content` text** (`Inbox.tsx` bubble), so an image
   shows as the literal `"[Image]"`, a sticker as `"[Sticker]"`, etc.
2. **Downloadable LINE media can't be loaded by the browser.** The stored
   `contentUrl` is `https://api-data.line.me/v2/bot/message/{id}/content`, which
   **requires the channel access token** (Bearer) — a browser `<img>` gets 401.

Stickers are different: they are non-downloadable but available from a **public**
LINE CDN, so the frontend can render them directly from `stickerId`.

---

## 1. Goal & non-goals

**Goal:** Render inbound LINE media in the conversation thread — images and stickers
inline (the explicit complaint), plus video/audio/file/location handled properly —
both on history load and live via realtime.

**Non-goals:**
- **No outbound media send** — that is a separate sub-project (①).
- **No FB/IG media** — out of scope here; the proxy is LINE-specific. The frontend
  `MessageMedia` component is generic enough to extend later.
- **No mirroring/persistence** — media is proxied on demand (chosen over
  download-at-ingest). Consequence: media for very old messages may 404 once LINE
  expires its content; the UI degrades to the text label.
- **No `send_batch` / `message_view` change** — `message_view` already exposes
  `messageType` + `metadata.media`; the gateway is untouched.
- **No new DB column / migration.**

---

## 2. Backend — on-demand media proxy

### 2.1 Routes

```
GET /api/conversations/{conv_id}/messages/{msg_id}/media
GET /api/conversations/{conv_id}/messages/{msg_id}/media/preview
```

Registered in `backend/src/domain/conversations/mod.rs` alongside the existing
message routes. Handler lives in `conversations/handlers.rs` (or a focused new
`conversations/media.rs` module if `handlers.rs` is already large — implementer's
call, following the file's existing size/conventions).

### 2.2 Handler behavior

```text
proxy_message_media(state, user, Path((conv_id, msg_id)), preview: bool):
  1. Access gate: if !store::can_act_on(&state.db, &user, &conv_id) → 403
     (the same gate list_messages uses — "can see the messages ⇒ can see their media").
  2. Load the message row by (id = msg_id AND conversation_id = conv_id AND deleted_at IS NULL):
       SELECT content_type, platform_message_id, sender_type FROM messages WHERE ...
     Missing → 404.
  3. Only downloadable LINE kinds proceed: content_type ∈ {image, video, audio, file}.
     Anything else (text/sticker/location/unknown) → 404 (stickers/locations never
     hit this route; the frontend renders them without the proxy).
  4. platform_message_id must be present (the LINE message id) → else 404.
  5. line_channel_access_token must be configured → else 404 (best-effort, no token in dev/tests).
  6. Fetch upstream with a 5s connect / streaming-friendly client:
       GET https://api-data.line.me/v2/bot/message/{platform_message_id}/content[/preview]
       Authorization: Bearer {line_channel_access_token}
     - `/preview` suffix only for the preview route AND only for image/video
       (audio/file have no preview — preview route falls back to the full content URL).
  7. Non-2xx upstream → 502 (frontend onError → text fallback).
  8. Stream the response body back to the client:
     - Set `Content-Type` from the upstream response (default `application/octet-stream`).
     - For `content_type == "file"`: add `Content-Disposition: inline; filename="<fileName>"`
       using `metadata.media.fileName` when available (sanitized; fall back to the msg_id).
     - Use axum `Body::from_stream(resp.bytes_stream())` so large video/file are not
       fully buffered in memory.
```

### 2.3 Shared client / helper

Reuse `channels.rs::http_client()` (the shared pooled reqwest client). Add a small
helper there or in the media module that issues the authenticated GET and returns
the `reqwest::Response` (so the handler can stream it). Keep `send_batch` and
`fetch_profile` untouched.

---

## 3. Frontend — media rendering

### 3.1 Message type + data plumbing

`frontend/src/pages/Inbox.tsx` `interface Message` gains:
```ts
  messageType?: string
  media?: Record<string, unknown>   // from metadata.media (mediaId, stickerId, fileName, latitude, …)
```

- **History load** (the `list_messages` fetch mapping, ~line 424-447): set
  `messageType: m.messageType` and `media: m.metadata?.media`.
- **Realtime** (`frontend/src/realtime/client.ts::readMessageEvent`): also return
  `messageType` and `media`. The two wire shapes differ, so normalize both:
  - **kind:** `nested.type ?? payload.messageType ?? nested.messageType`.
  - **media:** start from `nested.media` (GET-style). If absent and `nested.metadata`
    exists, parse it when it's a string, then take `meta.media` if present **else the
    parsed object itself** — because the realtime inbound payload puts the media JSON
    *directly* in `message.metadata` (e.g. `{"type":"image","mediaId":…}`), whereas
    the `message_view` GET nests it under `metadata.media`.

  The Inbox `new_message` handler then sets `messageType`/`media` on the appended
  message so live inbound media renders without a refresh.

### 3.2 `MessageMedia` component

New `frontend/src/components/MessageMedia.tsx`. Props: `{ convId: string; msgId: string;
messageType: string; media?: Record<string, unknown>; content: string }`. Renders by
`messageType`:

| kind | render |
|------|--------|
| `image` | `<img src={`/api/conversations/${convId}/messages/${msgId}/media/preview`} onClick→lightbox onError→text>` (max-width thumbnail) |
| `sticker` | `<img src={stickerUrl(media.stickerId)} onError→text>` — transparent, ~120px, no bubble background |
| `video` | `<video controls preload="metadata" src={`…/media`}>` |
| `audio` | `<audio controls src={`…/media`}>` |
| `file` | a chip: file icon + `media.fileName` + formatted `media.fileSize`, wrapped in `<a href={`…/media`} download>` |
| `location` | `<a href={`https://www.google.com/maps?q=${lat},${lng}`} target="_blank">` showing the title/address from `content` |
| else | the plain `content` text |

`stickerUrl(stickerId)` = `https://stickershop.line-scdn.net/stickershop/v1/sticker/${stickerId}/iPhone/sticker.png` (static PNG; animated stickers fall back to this still frame, `onError` → `[Sticker]` text).

**Lightbox:** clicking an image opens a minimal fixed-overlay (dark backdrop,
centered `<img>` from the full `…/media` URL, click/Esc to close). Implemented inside
`MessageMedia` (local `open` state) or a tiny shared `Lightbox` — implementer's call;
keep it dependency-free.

### 3.3 Bubble integration

In `Inbox.tsx` the thread bubble currently renders `{msg.content}`. Change to: when
`msg.messageType` is a media kind (`image|sticker|video|audio|file|location`), render
`<MessageMedia convId={convId} msgId={msg.id} messageType={msg.messageType}
media={msg.media} content={msg.content} />`; otherwise render `{msg.content}` as today.
For `sticker`, render without the `cs-bubble` background (stickers float).

---

## 4. Error handling

- Proxy: any failure (no access → 403; missing/non-media message → 404; no token →
  404; upstream non-2xx → 502). Never panics; never streams partial garbage with a
  200.
- Frontend: every media `<img>/<video>/<audio>` has an `onError` that falls back to
  the text label (`content`, e.g. `"[Image]"`), so an expired/broken media URL
  degrades gracefully to today's behavior.
- A message with `messageType` media but missing `media`/`msgId` → text fallback.

---

## 5. Testing

**Backend (`backend/tests/`):**
- Access gate: the media route returns 403 for an agent who fails `can_act_on` for
  the conversation (reuse an existing conversations-test helper/fixture).
- Non-media / missing message: requesting `/media` for a text message or an unknown
  msg_id returns 404.
- No-token: with the harness default (`line_channel_access_token = None`), an
  image message's `/media` returns 404 (no network call) — keeps tests network-free.
- Real LINE byte streaming is verified manually against the live OA (like the profile
  feature), not in the suite.

**Frontend (`frontend/src/components/MessageMedia.test.tsx`, vitest):**
- `image` → renders an `<img>` whose src is the `…/media/preview` proxy URL.
- `sticker` → renders an `<img>` whose src is the LINE sticker CDN URL built from
  `media.stickerId`.
- `file` → renders a download `<a>` with the file name.
- `text`/unknown → renders the plain content (no `<img>`).

---

## 6. Verification

- `cd backend && cargo build && cargo build --tests && cargo test` — green.
- `cd frontend && npm run build && npx vitest run` — green.
- `impact()` before editing the conversations router / message handlers;
  `detect_changes()` before commits. `send_batch` and `message_view` must not appear
  in the changed set (proxy is additive; `message_view` already exposes the data).
- Manual live check: a LINE user sends a photo and a sticker → both render inline in
  the thread (photo click → lightbox); a sent file shows a download chip.

---

## 7. Resolved decisions

- **On-demand proxy** for downloadable LINE media (not mirror-at-ingest): simpler,
  no storage; accepted tradeoff that long-expired LINE content 404s → text fallback.
- **All inbound kinds** rendered: image + sticker inline; video/audio playable; file
  as a download chip; location as a maps link.
- **Stickers are frontend-only** via the public LINE CDN (`stickerId`); no proxy.
- Access gate = `store::can_act_on` (same as `list_messages`).
- Media descriptor is already on the wire (`messageType` + `metadata.media`); only a
  proxy route is added backend-side. `send_batch`/`message_view` untouched.
- LINE only; FB/IG media and outbound media send are separate efforts.

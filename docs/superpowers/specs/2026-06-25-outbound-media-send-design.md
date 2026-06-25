# Send File / Photo (Outbound Media) — Design Spec

**Date:** 2026-06-25
**Track:** inbox composer + outbound delivery (frontend + backend)
**Status:** design approved, pending written-spec review

---

## 0. Context

Agents cannot send media to customers. The composer only has a "files drawer"
(uploads to the conversation file store) — there is no attach-to-message flow.
And the outbound path is broken: `send_message` turns each attachment into
`OutboundItem { content: file_url }` (a relative `/uploads/...` path), and
`build_push_body` wraps **every** item as a LINE `type:"text"` message — so the
customer receives a path string, not a photo.

**LINE constraint:** the Messaging API push supports `text`, `image`, `video`,
`audio`, `sticker`, `location`, `imagemap`, `template`, `flex` — there is **no
generic "file" message**. Image/video/audio need publicly fetchable HTTPS URLs;
video also needs a `previewImageUrl` (thumbnail) and audio a `duration` (ms).

The repo already has the pieces we need: `signed_public_url(state, key, ttl)`
(`files/handlers.rs`) builds a signature-gated public URL
(`{backend_url}/api/files/public/{key}?expires=…&sig=…`) that LINE can fetch
without auth; the `POST /api/conversations/{id}/attachments` upload endpoint;
and the `MessageMedia` renderer from the inbound-media sub-project (③).

---

## 1. Goal & non-goals

**Goal:** Agents attach photos/files in the composer (button + drag-drop + paste,
multiple files, preview chips) and send them; the customer receives a native LINE
**image/video/audio** message, or a **text + public link** for any other file type;
the sent media renders in the agent's thread.

**Non-goals:**
- **No FB/IG native media** yet — on those platforms media items degrade to a text
  link (LINE is the live platform; FB/IG native is a later effort).
- **No server-side thumbnail/transcode** (no ffmpeg) — video preview uses a small
  embedded placeholder image; audio duration uses a best-effort/default value.
- **No new storage** — reuse the existing attachment upload + `signed_public_url`.
- **No change to the inbound-media proxy** (③) — agent media renders from the
  attachment URL, not the LINE proxy.

---

## 2. Backend — outbound media

### 2.1 `OutboundItem` carries an optional media descriptor (`channels.rs`)

```rust
#[derive(Clone)]
pub struct OutboundItem {
    pub content: String,            // text body, or the link text for a `file`
    pub media: Option<OutboundMedia>,
}
impl OutboundItem {
    pub fn text(content: impl Into<String>) -> Self {
        Self { content: content.into(), media: None }
    }
}

#[derive(Clone)]
pub struct OutboundMedia {
    pub kind: MediaKind,            // Image | Video | Audio | File
    pub url: String,               // public original-content URL
    pub preview_url: Option<String>, // image/video preview URL
    pub file_name: Option<String>, // for File (and download display)
    pub duration_ms: Option<i64>,  // for Audio
}

#[derive(Clone, Copy, PartialEq)]
pub enum MediaKind { Image, Video, Audio, File }
```

There are **13** existing `OutboundItem { content: … }` construction sites. **12**
become `OutboundItem::text(…)` (they all send text): `realtime/customer.rs` (×2),
`customer_conversations/handlers.rs` (×2), `auto_reply/engine.rs`,
`conversations/handlers.rs` (the **text** push at ~line 820), `channels.rs` test
(×2), `queue/worker.rs` (×3), `messaging/service.rs` (×2). The **13th** —
`conversations/handlers.rs` attachment loop (~line 833) — is rewritten to build a
media item (§2.3). Internal reads of `it.content` in `build_push_body`/`fb_send`
stay valid.

### 2.2 LINE message bodies per item (`build_push_body`)

`build_push_body(recipient, items)` maps each item to a LINE message object:

| item | LINE message |
|------|--------------|
| `media: None` (text) | `{type:"text", text: content}` |
| `Image` | `{type:"image", originalContentUrl: url, previewImageUrl: preview_url ∥ url}` |
| `Video` | `{type:"video", originalContentUrl: url, previewImageUrl: preview_url}` |
| `Audio` | `{type:"audio", originalContentUrl: url, duration: duration_ms ∥ DEFAULT}` |
| `File`  | `{type:"text", text: "📎 {file_name}\n{url}"}` |

`DEFAULT` audio duration = `60000` (ms) — display-only; LINE plays the full clip
regardless. `fb_send` (FB/IG): for any item with `media`, send a text fallback
`"📎 {file_name ∥ ''}\n{url}"` (native FB media is out of scope here).

### 2.3 Classify attachments + resolve public URLs (`send_message`)

Replace the attachment loop (currently `OutboundItem { content: url }`). For each
attachment, select `content_type` (mime), `storage_key`, `file_name`, `file_url`;
then:

- Resolve the public original URL via `signed_public_url(&state, &storage_key, TTL)`
  (make that fn `pub(crate)`; `TTL` = the existing download TTL). If `backend_url`
  is **unset** (no public base), fall back to a `File` item whose link is the
  relative `file_url` — degraded but non-crashing (dev without a tunnel / tests).
- Classify by mime prefix → `MediaKind`:
  - `image/*` → `Image` (preview_url = same public URL),
  - `video/*` → `Video` (preview_url = the video placeholder URL, §2.4),
  - `audio/*` → `Audio` (duration_ms = None → default),
  - else → `File` (content = `file_name`, url = public URL).
- Push `OutboundItem { content: <file_name or "">, media: Some(OutboundMedia{…}) }`.

The realtime `message_sent`/`new_message` broadcast payload and the persisted
message are unchanged (attachments already linked via `attachment_ids`).

### 2.4 Video preview placeholder (small public asset)

Add a public route `GET /api/assets/video-placeholder.png` returning a small
embedded PNG (a const byte array — a plain dark rectangle with a play glyph is
fine; no repo binary, no auth). Its absolute URL (`{backend_url}/api/assets/
video-placeholder.png`) is the `previewImageUrl` for outbound video. If
`backend_url` is unset, video degrades to a `File` text link (same as §2.3).

---

## 3. Frontend — composer attach

### 3.1 Pending-attachment state + inputs (`Inbox.tsx` composer)

- An **attach button** (📎) triggers a hidden `<input type="file" multiple
  accept="image/*,video/*,audio/*,application/pdf,...">`.
- **Drag-drop** onto the composer and **paste** (`onPaste` image/file from the
  clipboard) add files too. (The composer already has drag handlers for the files
  drawer; this adds an "attach to message" target.)
- On add, each file uploads immediately via the existing
  `uploadConversationFile(convId, file)` (`POST /attachments`), yielding an
  attachment `{ id, filename, mimeType, url }`. Store a pending list:
  `{ id, name, mime, previewUrl }` where `previewUrl` is a local `URL.createObjectURL`
  for images (instant thumbnail).
- Render **preview chips** above the input: image thumbnail or file icon + name,
  each with a remove (×) that drops it from the pending list (and revokes the
  object URL).

### 3.2 Send

`send()` may fire with attachments and empty text. It POSTs
`/api/conversations/{id}/messages` with `{ content, senderId, attachmentIds:
pending.map(p => p.id) }`, then clears the draft + pending list. The optimistic
bubble shows the attachments (image thumbnails / file chips) immediately.
Upload failure → toast, the chip is not added. Send failure → keep draft + chips,
show the error (existing pattern).

---

## 4. Frontend — display sent media (reuse ③)

- Extend `MessageMedia` with an optional `srcUrl?: string`. When present, it is used
  directly for image/video/audio/file (no proxy URL is built); `messageType` still
  selects the renderer. Sticker/location are unaffected.
- `Message` gains `attachments?: Array<{ id; filename?; mimeType?; url?; downloadUrl? }>`
  (already returned by `list_messages`). The thread bubble: if `msg.attachments?.length`,
  render each attachment with `<MessageMedia srcUrl={att.url} messageType={kindFromMime(att.mimeType)} content={att.filename} />`; else fall back to the existing inbound media/text logic (③).
- `kindFromMime(mime)`: `image/*→image`, `video/*→video`, `audio/*→audio`, else `file`.

---

## 5. Error handling

- Upload failure → toast; no chip added; send unaffected.
- Send failure → draft + pending chips preserved; error shown.
- Outbound with no public base (`backend_url` unset) → media items become `File`
  text links (customer still gets a clickable link); never crashes.
- LINE rejects a media message (bad URL/size) → the existing `deliver_pending`
  failure path marks delivery failed (no new behavior needed).
- Display: `MessageMedia` `onError` already falls back to text/icon.

---

## 6. Testing

**Backend (unit, `channels.rs`):**
- `build_push_body`: text→`text`; an `Image` item→`{type:"image",originalContentUrl,previewImageUrl}`; `Video`→`{type:"video",…previewImageUrl}`; `Audio`→`{type:"audio",…,duration}`; `File`→`{type:"text"}` with `📎`.
- mime→`MediaKind` classification helper truth table.

**Backend (integration, `conversations.rs`):**
- Send a message with an uploaded image attachment (harness has no LINE token → stub delivery, no network): assert the message persists with the attachment linked and the response is 200/queued (behavior preserved; the media path is exercised without hitting LINE).
- `GET /api/assets/video-placeholder.png` → 200, `image/png`.

**Frontend (vitest):**
- Composer: adding a file shows a chip; removing it drops the chip; `send` includes `attachmentIds`. (Mock `uploadConversationFile`.)
- `MessageMedia` with `srcUrl` renders an `<img>/<video>/<a>` pointing at `srcUrl` (not the proxy).
- `kindFromMime` truth table.

---

## 7. Verification

- `cd backend && cargo build && cargo build --tests && cargo test` — green; `grep` the 13 migrated sites build clean.
- `cd frontend && npm run build && npx vitest run` — green.
- `impact()` on `send_batch`/`build_push_body`/`OutboundItem` before editing (CRITICAL hub — proceed carefully, additive per-item dispatch); `detect_changes()` before commits.
- Manual live (LINE OA): agent sends a **photo** (customer receives a real image), a **video** (image-preview placeholder, plays on tap), and a **PDF** (customer receives `📎 name + link`); all three render in the agent thread.

---

## 8. Resolved decisions

- **Image + video + audio native**; other files → text + public link. FB/IG media → text link (later).
- `OutboundItem` gains `media: Option<OutboundMedia>` + `::text()` ctor; 13 text sites migrated.
- Public URL via `signed_public_url` (base = `backend_url`); no public base → graceful text-link degrade.
- Video preview = small embedded placeholder PNG served at `/api/assets/video-placeholder.png`; audio duration default 60000 ms.
- Composer: attach button + drag-drop + paste, multiple files, preview chips; send allows attachments-only.
- Sent media renders via `MessageMedia` with a direct `srcUrl` (attachment URL), reusing ③; inbound LINE media keeps the proxy.

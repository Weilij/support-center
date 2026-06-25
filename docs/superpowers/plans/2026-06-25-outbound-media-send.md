# Send File / Photo (Outbound Media) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let agents attach photos/files in the composer (button + drag-drop + paste, multiple, preview chips) and deliver them to the customer as native LINE image/video/audio messages (other files → text + public link), rendering the sent media in the thread.

**Architecture:** `OutboundItem` gains an optional media descriptor; `build_push_body` emits the right LINE message per item kind; `send_message` classifies each attachment by mime and resolves a signature-gated public URL (`signed_public_url`). The composer uploads on add and sends `attachmentIds`; the thread reuses the `MessageMedia` renderer (from ③) with a direct `srcUrl` for agent-uploaded media. `send_batch` is a CRITICAL hub — changes are additive per-item dispatch.

**Tech Stack:** Rust, axum, sqlx, reqwest; React 18 + TypeScript + Vite; vitest.

**Spec:** `docs/superpowers/specs/2026-06-25-outbound-media-send-design.md`

---

## File Structure

- `backend/src/domain/conversations/channels.rs` — **modify**: `OutboundItem{content,media}` + `OutboundMedia`/`MediaKind` + `OutboundItem::text` + `classify_mime` + per-item `build_push_body`/`fb_send` dispatch; unit tests.
- 12 call-site files — **modify**: convert text `OutboundItem` literals to `::text(...)` (compiler-enumerated).
- `backend/src/domain/files/handlers.rs` — **modify**: make `signed_public_url` `pub(crate)`; add `video_placeholder` handler.
- `backend/src/domain/files/mod.rs` — **modify**: register `/api/assets/video-placeholder.png`.
- `backend/assets/video-placeholder.png` — **create**: tiny embedded PNG.
- `backend/src/domain/conversations/handlers.rs` — **modify**: `send_message` attachment loop → media items.
- `backend/tests/conversations.rs` — **modify**: send-with-attachment + asset-route tests.
- `frontend/src/pages/Inbox.tsx` — **modify**: composer attach (button/drag/paste/chips/send) + sent-media bubble.
- `frontend/src/components/MessageMedia.tsx` — **modify**: optional `srcUrl`.
- `frontend/src/components/MessageMedia.test.tsx` — **modify**: `srcUrl` + `kindFromMime` tests.

---

## Task 1: `OutboundItem` media model + LINE dispatch (CRITICAL hub)

**Files:**
- Modify: `backend/src/domain/conversations/channels.rs`
- Modify (compiler-enumerated): `backend/src/realtime/customer.rs`, `backend/src/domain/customer_conversations/handlers.rs`, `backend/src/domain/auto_reply/engine.rs`, `backend/src/domain/conversations/handlers.rs`, `backend/src/domain/queue/worker.rs`, `backend/src/domain/messaging/service.rs`

- [ ] **Step 1: Write the failing unit tests**

In the `gateway_tests` module of `backend/src/domain/conversations/channels.rs`, add:

```rust
    #[test]
    fn push_body_dispatches_by_media_kind() {
        let items = vec![
            OutboundItem::text("hello"),
            OutboundItem { content: "pic.jpg".into(), media: Some(OutboundMedia {
                kind: MediaKind::Image, url: "https://h/o.jpg".into(),
                preview_url: Some("https://h/p.jpg".into()), file_name: None, duration_ms: None }) },
            OutboundItem { content: "clip".into(), media: Some(OutboundMedia {
                kind: MediaKind::Video, url: "https://h/v.mp4".into(),
                preview_url: Some("https://h/ph.png".into()), file_name: None, duration_ms: None }) },
            OutboundItem { content: "voice".into(), media: Some(OutboundMedia {
                kind: MediaKind::Audio, url: "https://h/a.m4a".into(),
                preview_url: None, file_name: None, duration_ms: None }) },
            OutboundItem { content: "doc".into(), media: Some(OutboundMedia {
                kind: MediaKind::File, url: "https://h/d.pdf".into(),
                preview_url: None, file_name: Some("report.pdf".into()), duration_ms: None }) },
        ];
        let b = build_push_body("U1", &items);
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["type"], "text");
        assert_eq!(m[1]["type"], "image");
        assert_eq!(m[1]["originalContentUrl"], "https://h/o.jpg");
        assert_eq!(m[1]["previewImageUrl"], "https://h/p.jpg");
        assert_eq!(m[2]["type"], "video");
        assert_eq!(m[2]["previewImageUrl"], "https://h/ph.png");
        assert_eq!(m[3]["type"], "audio");
        assert_eq!(m[3]["duration"], 60000);
        assert_eq!(m[4]["type"], "text");
        assert!(m[4]["text"].as_str().unwrap().contains("report.pdf"));
        assert!(m[4]["text"].as_str().unwrap().contains("📎"));
    }

    #[test]
    fn classify_mime_maps_kinds() {
        assert_eq!(classify_mime("image/png"), MediaKind::Image);
        assert_eq!(classify_mime("video/mp4"), MediaKind::Video);
        assert_eq!(classify_mime("audio/m4a"), MediaKind::Audio);
        assert_eq!(classify_mime("application/pdf"), MediaKind::File);
        assert_eq!(classify_mime(""), MediaKind::File);
    }
```

- [ ] **Step 2: Run → fail to compile**

Run: `cd backend && cargo test --lib gateway_tests 2>&1 | tail -15` → FAIL (`OutboundMedia`/`MediaKind`/`classify_mime`/`OutboundItem::text` + the `media` field don't exist).

- [ ] **Step 3: Extend the model + dispatch in `channels.rs`**

Replace the existing `OutboundItem` struct (currently `pub struct OutboundItem { pub content: String }`) and `build_push_body` with:

```rust
/// One outbound unit: a text body, or a media attachment.
pub struct OutboundItem {
    pub content: String,
    pub media: Option<OutboundMedia>,
}

impl OutboundItem {
    pub fn text(content: impl Into<String>) -> Self {
        Self { content: content.into(), media: None }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MediaKind { Image, Video, Audio, File }

#[derive(Clone)]
pub struct OutboundMedia {
    pub kind: MediaKind,
    pub url: String,
    pub preview_url: Option<String>,
    pub file_name: Option<String>,
    pub duration_ms: Option<i64>,
}

/// Display-only audio length when the real duration is unknown (LINE plays the
/// full clip regardless).
const DEFAULT_AUDIO_DURATION_MS: i64 = 60_000;

/// Classify an attachment mime into a LINE-deliverable kind.
pub fn classify_mime(mime: &str) -> MediaKind {
    if mime.starts_with("image/") {
        MediaKind::Image
    } else if mime.starts_with("video/") {
        MediaKind::Video
    } else if mime.starts_with("audio/") {
        MediaKind::Audio
    } else {
        MediaKind::File
    }
}

/// One LINE message object for an outbound item (pure — unit-tested).
fn line_message(it: &OutboundItem) -> serde_json::Value {
    match &it.media {
        None => json!({ "type": "text", "text": it.content }),
        Some(m) => match m.kind {
            MediaKind::Image => json!({
                "type": "image",
                "originalContentUrl": m.url,
                "previewImageUrl": m.preview_url.clone().unwrap_or_else(|| m.url.clone()),
            }),
            MediaKind::Video => json!({
                "type": "video",
                "originalContentUrl": m.url,
                "previewImageUrl": m.preview_url.clone().unwrap_or_else(|| m.url.clone()),
            }),
            MediaKind::Audio => json!({
                "type": "audio",
                "originalContentUrl": m.url,
                "duration": m.duration_ms.unwrap_or(DEFAULT_AUDIO_DURATION_MS),
            }),
            MediaKind::File => json!({
                "type": "text",
                "text": format!("📎 {}\n{}", m.file_name.clone().unwrap_or_default(), m.url),
            }),
        },
    }
}

/// The outbound message body for a LINE push (pure — unit-tested).
pub fn build_push_body(recipient: &str, items: &[OutboundItem]) -> serde_json::Value {
    json!({
        "to": recipient,
        "messages": items.iter().map(line_message).collect::<Vec<_>>(),
    })
}
```

In `fb_send`, replace the per-item body construction so media items degrade to a text link. Find the loop line `.json(&fb_send_body(recipient, &it.content))` and change it to compute the content first:
```rust
        let content = match &it.media {
            Some(m) => format!("📎 {}\n{}", m.file_name.clone().unwrap_or_default(), m.url),
            None => it.content.clone(),
        };
        let resp = http_client()
            .post(&url)
            .json(&fb_send_body(recipient, &content))
            .send()
            .await
            .map_err(|e| format!("Facebook request failed: {e}"))?;
```

- [ ] **Step 4: Migrate every text `OutboundItem` literal to `::text(...)`**

`cargo build` now fails at each `OutboundItem { content: x }` literal (missing `media`). The compiler enumerates them. Convert EACH to `OutboundItem::text(x)`, including the `conversations/handlers.rs` attachment loop (≈line 833) — leave it as `OutboundItem::text(url)` for now (Task 2 rewrites it into media). Known sites: `realtime/customer.rs` (×2), `customer_conversations/handlers.rs` (×2), `auto_reply/engine.rs`, `conversations/handlers.rs` (text push ≈820 + attachment ≈833), `channels.rs` test (×2, the `vec![...]`), `queue/worker.rs` (×3), `messaging/service.rs` (×2). Run `cd backend && cargo build 2>&1 | grep -E "OutboundItem|error" | head` and fix until clean — there must be NO remaining `OutboundItem { content` literal except the test you wrote in Step 1 (which sets `media`).

- [ ] **Step 5: Run unit tests + build**

- `cd backend && cargo test --lib gateway_tests 2>&1 | grep "test result"` → `ok.`
- `cd backend && cargo build 2>&1 | tail -3` → success.
- `cd backend && grep -rn 'OutboundItem {' src/ | grep -v 'media:' | grep -v 'pub struct'` → only the Step-1 test lines (which include `media:`) — i.e. no stray text literals.

- [ ] **Step 6: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add backend/src
git commit -m "feat(channels): OutboundItem media model + per-kind LINE message dispatch"
```

---

## Task 2: Classify attachments → media on send + video placeholder asset

**Files:**
- Modify: `backend/src/domain/files/handlers.rs`
- Modify: `backend/src/domain/files/mod.rs`
- Create: `backend/assets/video-placeholder.png`
- Modify: `backend/src/domain/conversations/handlers.rs`
- Test: `backend/tests/conversations.rs`

- [ ] **Step 1: Create the placeholder PNG**

Run (creates a valid 1×1 PNG; it is a functional preview placeholder and can be swapped for a nicer asset later):
```bash
cd /Users/kkllzz_0/support-center
mkdir -p backend/assets
printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==' | base64 --decode > backend/assets/video-placeholder.png
ls -l backend/assets/video-placeholder.png   # ~70 bytes
```

- [ ] **Step 2: Add the asset handler + route**

In `backend/src/domain/files/handlers.rs`, add a handler (place near `public_proxy`); reuse the existing `stream_bytes` helper in this file:
```rust
/// GET /api/assets/video-placeholder.png — a static thumbnail used as the
/// `previewImageUrl` for outbound LINE video messages (public, no auth).
pub async fn video_placeholder() -> Response {
    const PNG: &[u8] = include_bytes!("../../../assets/video-placeholder.png");
    stream_bytes(PNG.to_vec(), "image/png", None, "public, max-age=604800")
}
```
Also change the existing `fn signed_public_url(...)` declaration to `pub(crate) fn signed_public_url(...)` (so `conversations/handlers.rs` can call it).

In `backend/src/domain/files/mod.rs`, register the route in the **public** group (next to `/api/files/public/{*path}`):
```rust
        .route("/api/assets/video-placeholder.png", get(handlers::video_placeholder))
```
Run `cd backend && cargo build 2>&1 | tail -3` → success.

- [ ] **Step 3: Rewrite the `send_message` attachment loop to build media items**

In `backend/src/domain/conversations/handlers.rs`, near the top add a TTL const (place by the other consts):
```rust
/// Signed-URL lifetime for outbound media (LINE fetches at send time).
const OUTBOUND_MEDIA_TTL_SECS: i64 = 7 * 24 * 3600;
```
Add a row struct (near `MediaMsgRow`):
```rust
#[derive(sqlx::FromRow)]
struct OutAttRow {
    content_type: Option<String>,
    storage_key: Option<String>,
    file_name: Option<String>,
    file_url: Option<String>,
}
```
Replace the current attachment block in `send_message` (the loop that does
`for url in q.bind(&message_id)...{ items.push(OutboundItem::text(url)) }`, ≈ lines 825-835 after Task 1) with:
```rust
    if !attachment_ids.is_empty() {
        let placeholders = vec!["?"; attachment_ids.len()].join(", ");
        let sql = format!(
            "SELECT content_type, storage_key, file_name, file_url FROM attachments
             WHERE id IN ({placeholders}) AND message_id = $1"
        );
        let sql = crate::db::pg_params(&sql);
        let mut q = sqlx::query_as::<_, OutAttRow>(&sql);
        for aid in &attachment_ids {
            q = q.bind(aid);
        }
        let has_public_base = state.config.backend_url.is_some();
        for a in q.bind(&message_id).fetch_all(&state.db).await? {
            let name = a.file_name.clone();
            // A public URL requires both a stored object key and a public base.
            let public_url = match (has_public_base, a.storage_key.as_deref()) {
                (true, Some(key)) => Some(crate::domain::files::handlers::signed_public_url(
                    &state, key, OUTBOUND_MEDIA_TTL_SECS,
                )),
                _ => None,
            };
            match public_url {
                Some(url) => {
                    let kind = channels::classify_mime(a.content_type.as_deref().unwrap_or(""));
                    let preview_url = match kind {
                        channels::MediaKind::Image => Some(url.clone()),
                        channels::MediaKind::Video => Some(format!(
                            "{}/api/assets/video-placeholder.png",
                            state.config.backend_url.clone().unwrap_or_default()
                        )),
                        _ => None,
                    };
                    items.push(OutboundItem {
                        content: name.clone().unwrap_or_default(),
                        media: Some(channels::OutboundMedia {
                            kind,
                            url,
                            preview_url,
                            file_name: name,
                            duration_ms: None,
                        }),
                    });
                }
                // No public base / no stored object: degrade to a text link.
                None => items.push(OutboundItem::text(format!(
                    "📎 {}\n{}",
                    name.unwrap_or_default(),
                    a.file_url.unwrap_or_default()
                ))),
            }
        }
    }
```
Ensure `channels::{OutboundMedia, MediaKind, classify_mime}` resolve (the file already `use`s the `channels` module / `OutboundItem`; add `use` items if needed). Run `cd backend && cargo build 2>&1 | tail -3` → success.

- [ ] **Step 4: Tests**

In `backend/tests/conversations.rs` (reuse the helpers from the ③ media tests / existing send-message tests):
1. **Asset route:** `GET /api/assets/video-placeholder.png` (no auth needed) → status 200 and `content-type: image/png`.
2. **Send with image attachment (no token → stub, no network):** seed a conversation, upload/seed an attachment row (mirror how the existing attachment tests insert one: `content_type='image/png'`, a `storage_key`, `file_url`), then `POST /api/conversations/{id}/messages` with `{ content: "", senderId, attachmentIds: [thatId] }`; assert 200 and that the attachment is now linked to the created message (`SELECT message_id FROM attachments WHERE id = …` is non-null). The delivery runs against the no-token stub, so no network call.

Study the file for the exact seed/auth/post helpers and match them.

- [ ] **Step 5: Run**

- `cd backend && cargo build --tests 2>&1 | tail -5` → success.
- `cd backend && cargo test --test conversations 2>&1 | grep -E "Running|test result|error\[|FAILED"` → green incl. the new tests.

- [ ] **Step 6: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add backend/src backend/assets/video-placeholder.png backend/tests/conversations.rs
git commit -m "feat(conversations): deliver attachments as LINE media (image/video/audio/file)"
```

---

## Task 3: Composer attach UX (button + drag + paste + chips + send)

**Files:**
- Modify: `frontend/src/pages/Inbox.tsx`

- [ ] **Step 1: Pending-attachment state + a hidden file input**

In the `Thread` component (where `draft`/`send` live), add state + a ref near the other `useState`/`useRef` hooks:
```tsx
  const fileInput = useRef<HTMLInputElement | null>(null)
  const [pending, setPending] = useState<Array<{ id: string; name: string; mime: string; previewUrl?: string }>>([])
```
Add an uploader that the button/drop/paste all call:
```tsx
  const addFiles = useCallback(async (files: FileList | File[]) => {
    if (!convId) return
    for (const file of Array.from(files)) {
      const { attachment, error } = await uploadConversationFile(convId, file)
      if (error || !attachment) { setToast(`上傳失敗：${error ?? file.name}`); continue }
      setPending((p) => [...p, {
        id: attachment.id,
        name: attachment.filename ?? file.name,
        mime: attachment.mimeType ?? file.type,
        previewUrl: file.type.startsWith('image/') ? URL.createObjectURL(file) : undefined,
      }])
    }
  }, [convId])
```
(`uploadConversationFile` and `setToast` already exist in this component; `Attachment` has `id`/`filename`/`mimeType`.)

- [ ] **Step 2: Wire the 📎 button, drag-drop, and paste**

- The composer already has a paperclip button `<button type="button" className="cs-composer-ico" aria-label="附件"><Icon name="paperclip" w={20} /></button>` with no handler. Add `onClick={() => fileInput.current?.click()}` to it, and add a hidden input right after it:
```tsx
              <input
                ref={fileInput}
                type="file"
                multiple
                accept="image/*,video/*,audio/*,application/pdf,.doc,.docx,.xls,.xlsx,.zip"
                style={{ display: 'none' }}
                onChange={(e) => { if (e.target.files) void addFiles(e.target.files); e.target.value = '' }}
              />
```
- Repoint the composer drop to message attachments: change `handleDrop` so that, instead of uploading to the files drawer, it calls `addFiles(e.dataTransfer.files)`. Concretely, replace the body of `handleDrop` that currently does the single-file `uploadConversationFile`/`refreshFiles` with:
```tsx
    const dropped = e.dataTransfer.files
    if (dropped && dropped.length) await addFiles(dropped)
```
(keep the `preventDefault`/`setDragOver(false)` lines). Update the drag-overlay text from `放開以上傳檔案到此對話` to `放開以附加到訊息`.
- Add paste support on the textarea — add `onPaste` to the `<textarea className="cs-composer-input" …>`:
```tsx
              onPaste={(e) => {
                const files = Array.from(e.clipboardData.files)
                if (files.length) { e.preventDefault(); void addFiles(files) }
              }}
```

- [ ] **Step 3: Preview chips above the input**

Directly above the `<textarea className="cs-composer-input" …>`, render the chips:
```tsx
            {pending.length > 0 && (
              <div className="cs-attach-row" style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginBottom: 8 }}>
                {pending.map((p) => (
                  <div key={p.id} className="cs-attach-chip" style={{ position: 'relative', display: 'flex', alignItems: 'center', gap: 6, padding: '4px 8px', border: '1px solid var(--border)', borderRadius: 8 }}>
                    {p.previewUrl
                      ? <img src={p.previewUrl} alt={p.name} style={{ width: 36, height: 36, objectFit: 'cover', borderRadius: 4 }} />
                      : <span>📄</span>}
                    <span style={{ maxWidth: 140, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{p.name}</span>
                    <button type="button" aria-label="移除" onClick={() => setPending((list) => {
                      const found = list.find((x) => x.id === p.id)
                      if (found?.previewUrl) URL.revokeObjectURL(found.previewUrl)
                      return list.filter((x) => x.id !== p.id)
                    })} style={{ border: 'none', background: 'transparent', cursor: 'pointer' }}>×</button>
                  </div>
                ))}
              </div>
            )}
```

- [ ] **Step 4: Send with attachments**

In `send()`:
- Change the guard `if (!convId || !draft.trim()) return` to:
```tsx
    if (!convId || (!draft.trim() && pending.length === 0)) return
```
- Capture + clear pending alongside the draft. After `const text = draft.trim()` add `const atts = pending`; after `setDraft('')` add `setPending([])`.
- Include the optimistic attachments on the temp message object (so they render immediately): add to the optimistic `setMessages([...prev, { id: tempId, content: text, senderType: 'agent', … }])` object:
```tsx
      attachments: atts.map((a) => ({ id: a.id, filename: a.name, mimeType: a.mime, url: a.previewUrl })),
```
- Change the POST body to include attachment ids:
```tsx
    const resp = await post<{ message?: Message; id?: string }>(
      `/api/conversations/${convId}/messages`,
      { content: text, senderId: who?.id, attachmentIds: atts.map((a) => a.id) },
    )
```
- On failure (the existing `else` that restores the draft), also restore pending: `setPending(atts)`.
- Enable the send button when attachments exist: change `disabled={!draft.trim()}` on the submit button to `disabled={!draft.trim() && pending.length === 0}`.

- [ ] **Step 5: Build + suite**

- `cd frontend && npm run build 2>&1 | tail -6` → `tsc -b` clean + vite success.
- `cd frontend && npx vitest run 2>&1 | tail -6` → green (no existing test broken).

- [ ] **Step 6: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/pages/Inbox.tsx
git commit -m "feat(frontend): composer attach (button/drag/paste), preview chips, send attachments"
```

---

## Task 4: Render sent media in the thread (reuse MessageMedia)

**Files:**
- Modify: `frontend/src/components/MessageMedia.tsx`
- Modify: `frontend/src/components/MessageMedia.test.tsx`
- Modify: `frontend/src/pages/Inbox.tsx`

- [ ] **Step 1: Write the failing tests**

In `frontend/src/components/MessageMedia.test.tsx`, add (import `kindFromMime` too):
```tsx
import { MessageMedia, isMediaKind, kindFromMime } from './MessageMedia'

  it('uses srcUrl directly for an agent image (no proxy URL)', () => {
    const { container } = render(
      <MessageMedia {...base} messageType="image" srcUrl="https://files/x.png" />,
    )
    expect(container.querySelector('img')?.getAttribute('src')).toBe('https://files/x.png')
  })

  it('kindFromMime maps mimes', () => {
    expect(kindFromMime('image/png')).toBe('image')
    expect(kindFromMime('video/mp4')).toBe('video')
    expect(kindFromMime('audio/m4a')).toBe('audio')
    expect(kindFromMime('application/pdf')).toBe('file')
    expect(kindFromMime(undefined)).toBe('file')
  })
```
Run `cd frontend && npx vitest run src/components/MessageMedia.test.tsx 2>&1 | tail -10` → FAIL (`kindFromMime` missing; `srcUrl` ignored).

- [ ] **Step 2: Add `srcUrl` + `kindFromMime` to `MessageMedia.tsx`**

Add to `MessageMediaProps`: `srcUrl?: string`. Add the export:
```tsx
export function kindFromMime(mime?: string): string {
  if (!mime) return 'file'
  if (mime.startsWith('image/')) return 'image'
  if (mime.startsWith('video/')) return 'video'
  if (mime.startsWith('audio/')) return 'audio'
  return 'file'
}
```
In the component, derive the source so `srcUrl` overrides the proxy. Replace the
`const mediaUrl = …` / `const previewUrl = …` lines with:
```tsx
  const mediaUrl = srcUrl ?? `/api/conversations/${convId}/messages/${msgId}/media`
  const previewUrl = srcUrl ?? `${mediaUrl}/preview`
```
(Add `srcUrl` to the destructured props.) Everything else stays — image/video/audio use `mediaUrl`/`previewUrl`, file uses `mediaUrl`, sticker/location are unaffected.

- [ ] **Step 3: Run the tests**

Run: `cd frontend && npx vitest run src/components/MessageMedia.test.tsx 2>&1 | tail -10` → PASS (all, incl. the 2 new).

- [ ] **Step 4: Carry `attachments` on `Message` + render in the bubble**

In `frontend/src/pages/Inbox.tsx`:
- Add to `interface Message`:
```ts
  attachments?: Array<{ id: string; filename?: string; mimeType?: string; url?: string; downloadUrl?: string }>
```
- Import `kindFromMime` alongside the existing `MessageMedia`/`isMediaKind` import:
```ts
import { MessageMedia, isMediaKind, kindFromMime } from '../components/MessageMedia'
```
- In the thread bubble (the block from ③ that branches on `isMediaKind(msg.messageType)`), add an attachments branch FIRST. Replace that block's opening so agent-uploaded attachments render from their own URL:
```tsx
                {msg.attachments && msg.attachments.length > 0 ? (
                  <div className={`cs-bubble${isMe ? ' cs-bubble--me' : ''}`}>
                    {msg.content && <div style={{ marginBottom: 6 }}>{msg.content}</div>}
                    {msg.attachments.map((att) => (
                      <MessageMedia
                        key={att.id}
                        convId={convId!}
                        msgId={msg.id}
                        messageType={kindFromMime(att.mimeType)}
                        srcUrl={att.url}
                        content={att.filename}
                      />
                    ))}
                  </div>
                ) : convId && isMediaKind(msg.messageType) ? (
                  // Stickers float without a chat-bubble frame; other media sit inside one.
                  msg.messageType === 'sticker' ? (
                    <MessageMedia convId={convId} msgId={msg.id} messageType={msg.messageType!} media={msg.media} content={msg.content} />
                  ) : (
                    <div className={`cs-bubble${isMe ? ' cs-bubble--me' : ''}`}>
                      <MessageMedia convId={convId} msgId={msg.id} messageType={msg.messageType!} media={msg.media} content={msg.content} />
                    </div>
                  )
                ) : (
                  <div className={`cs-bubble${isMe ? ' cs-bubble--me' : ''}`}>{msg.content}</div>
                )}
```
(This preserves the ③ inbound-media + text branches; it only adds the attachments-first case. If `att.url` is undefined for a non-image pending item, `MessageMedia` file kind still renders a download chip whose href is `undefined` — acceptable for the optimistic moment; after refetch the server `url` is present.)

- [ ] **Step 5: Build + full suite**

- `cd frontend && npm run build 2>&1 | tail -6` → clean.
- `cd frontend && npx vitest run 2>&1 | tail -8` → green (incl. the new MessageMedia tests).

- [ ] **Step 6: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/components/MessageMedia.tsx frontend/src/components/MessageMedia.test.tsx frontend/src/pages/Inbox.tsx
git commit -m "feat(frontend): render sent attachments in the thread via MessageMedia srcUrl"
```

---

## Final verification (after all tasks)

- [ ] `cd backend && cargo build && cargo build --tests && cargo test 2>&1 | grep -E "test result|error\[" | tail -20` — green.
- [ ] `cd backend && grep -rn 'OutboundItem {' src/ | grep -v 'media:'` — no stray text literals (all `::text` or media).
- [ ] `cd frontend && npm run build && npx vitest run 2>&1 | tail -6` — green.
- [ ] `detect_changes()` before the final review; confirm the `send_batch` change is the intended additive per-item dispatch (no behavior change for text-only callers).
- [ ] Manual live (LINE OA): agent attaches a **photo** → customer receives a real image; a **PDF** → customer receives `📎 name + link`; (optional) a **short video** → placeholder preview, plays on tap. All render in the agent thread; multiple-attach, drag-drop, and paste all work.

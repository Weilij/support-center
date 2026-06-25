# LINE Inbound Media Display Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show inbound LINE media (images, stickers, video, audio, files, location) in the conversation thread, via an authenticated backend proxy for downloadable content plus a new frontend `MessageMedia` renderer.

**Architecture:** A new authenticated route proxies LINE's token-gated content API (the browser can't send the channel token). Stickers use the public LINE CDN (frontend-only). The message API already returns `messageType` + `metadata.media`, so the frontend just plumbs those fields through (history + realtime) and renders media bubbles. Downloadable bytes are buffered and returned the same way the existing file-download handler does (`(StatusCode::OK, bytes).into_response()`), which is fine for chat-sized media.

**Tech Stack:** Rust, axum, sqlx, reqwest; React 18 + TypeScript + Vite; vitest.

**Spec:** `docs/superpowers/specs/2026-06-25-line-inbound-media-display-design.md`

---

## File Structure

- `backend/src/domain/conversations/channels.rs` — **modify**: add `fetch_line_media` (authenticated GET → bytes + content-type, best-effort).
- `backend/src/domain/conversations/handlers.rs` — **modify**: add the media proxy handlers + a `file_name_from_metadata` helper.
- `backend/src/domain/conversations/mod.rs` — **modify**: register the two media routes.
- `backend/tests/conversations.rs` — **modify**: proxy access/404 tests.
- `frontend/src/pages/Inbox.tsx` — **modify**: `Message` type + history/realtime plumbing + bubble integration.
- `frontend/src/realtime/client.ts` — **modify**: `readMessageEvent` returns `messageType` + `media`.
- `frontend/src/components/MessageMedia.tsx` — **create**: the media renderer + sticker CDN URL + lightbox.
- `frontend/src/components/MessageMedia.test.tsx` — **create**: render tests.

---

## Task 1: Backend media proxy

**Files:**
- Modify: `backend/src/domain/conversations/channels.rs`
- Modify: `backend/src/domain/conversations/handlers.rs`
- Modify: `backend/src/domain/conversations/mod.rs`
- Test: `backend/tests/conversations.rs`

- [ ] **Step 1: Add the LINE content fetcher to `channels.rs`**

Append to `backend/src/domain/conversations/channels.rs` (after `fetch_profile`'s impl block / near the other free functions; it uses the existing private `http_client()`):

```rust
/// Fetch LINE message content (image/video/audio/file) with the channel token.
/// `preview` requests the smaller preview rendition (image/video only). Returns
/// `(bytes, content_type)` or `None` on any failure — best-effort, never panics.
pub(crate) async fn fetch_line_media(
    token: &str,
    message_id: &str,
    preview: bool,
) -> Option<(Vec<u8>, String)> {
    let suffix = if preview { "/preview" } else { "" };
    let url = format!("https://api-data.line.me/v2/bot/message/{message_id}/content{suffix}");
    let resp = http_client()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = resp.bytes().await.ok()?;
    Some((bytes.to_vec(), content_type))
}
```

- [ ] **Step 2: Build to confirm the helper compiles**

Run: `cd backend && cargo build 2>&1 | tail -3` → success (a dead-code warning for the unused fn is acceptable until Step 3 wires it).

- [ ] **Step 3: Add the proxy handlers to `handlers.rs`**

First ensure these imports exist at the top of `backend/src/domain/conversations/handlers.rs` (add any that are missing):
```rust
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;
```
(`State`, `Extension`, `Path`, `Arc<AppState>`, `AuthUser`, `AppError`, `Result`, `store`, `channels`, `permission_denied`, `json`/`serde_json` are already used in this file.)

Add the helper + handlers (place near `list_messages`):

```rust
#[derive(sqlx::FromRow)]
struct MediaMsgRow {
    content_type: String,
    platform_message_id: Option<String>,
    metadata: Option<String>,
}

/// `metadata.media.fileName` if present (for the download filename).
fn file_name_from_metadata(metadata: Option<&str>) -> Option<String> {
    let v: Value = serde_json::from_str(metadata?).ok()?;
    v.get("media")
        .and_then(|m| m.get("fileName"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|s| s.replace(['"', '\\', '\r', '\n'], "_"))
}

async fn proxy_media_inner(
    state: &Arc<AppState>,
    user: &AuthUser,
    conv_id: &str,
    msg_id: &str,
    preview: bool,
) -> Result {
    // Same access gate as list_messages: can-see-messages ⇒ can-see-their-media.
    if !store::can_act_on(&state.db, user, conv_id).await? {
        return Err(permission_denied());
    }
    let row: Option<MediaMsgRow> = sqlx::query_as(
        "SELECT content_type, platform_message_id, metadata FROM messages
         WHERE id = $1 AND conversation_id = $2 AND deleted_at IS NULL",
    )
    .bind(msg_id)
    .bind(conv_id)
    .fetch_optional(&state.db)
    .await?;
    let row = row.ok_or_else(|| AppError::NotFound("Message not found".into()))?;
    if !["image", "video", "audio", "file"].contains(&row.content_type.as_str()) {
        return Err(AppError::NotFound("No downloadable media for this message".into()));
    }
    let message_id = row
        .platform_message_id
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::NotFound("Media unavailable".into()))?;
    let token = state
        .config
        .line_channel_access_token
        .clone()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::NotFound("Media unavailable".into()))?;
    // Preview rendition exists only for image/video.
    let use_preview = preview && (row.content_type == "image" || row.content_type == "video");
    let (bytes, content_type) = channels::fetch_line_media(&token, &message_id, use_preview)
        .await
        .ok_or_else(|| AppError::NotFound("Media unavailable".into()))?;

    let mut resp = (StatusCode::OK, bytes).into_response();
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if row.content_type == "file" {
        let name = file_name_from_metadata(row.metadata.as_deref())
            .unwrap_or_else(|| msg_id.to_string());
        if let Ok(v) = HeaderValue::from_str(&format!("inline; filename=\"{name}\"")) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    Ok(resp)
}

/// GET /api/conversations/{id}/messages/{msgId}/media
pub async fn proxy_media(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((conv_id, msg_id)): Path<(String, String)>,
) -> Result {
    proxy_media_inner(&state, &user, &conv_id, &msg_id, false).await
}

/// GET /api/conversations/{id}/messages/{msgId}/media/preview
pub async fn proxy_media_preview(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((conv_id, msg_id)): Path<(String, String)>,
) -> Result {
    proxy_media_inner(&state, &user, &conv_id, &msg_id, true).await
}
```

> If `Result` in this file is aliased without a default type param, write the handler return type exactly as the other handlers in the file do (e.g. `-> Result` or `-> Result<Response>`); match the existing convention. `Value` is `serde_json::Value`, already imported in this file.

- [ ] **Step 4: Register the routes in `mod.rs`**

In `backend/src/domain/conversations/mod.rs`, inside `routes()`, add after the `/messages` route:

```rust
        .route(
            "/api/conversations/{id}/messages/{msgId}/media",
            get(handlers::proxy_media),
        )
        .route(
            "/api/conversations/{id}/messages/{msgId}/media/preview",
            get(handlers::proxy_media_preview),
        )
```

(`get` is already imported in this file — it is used by the existing `/messages` route.)

- [ ] **Step 5: Build**

Run: `cd backend && cargo build 2>&1 | tail -3` → success (no dead-code warning now).

- [ ] **Step 6: Write the proxy tests**

Study `backend/tests/conversations.rs` for the existing helpers: the app spawner, the admin/agent auth cookie helper, the authenticated GET helper, and how a conversation + message are seeded (look for an existing test that lists messages or seeds a `messages` row). Reuse them. Add tests that, with the harness default (no LINE token):

1. **Text message → 404:** seed a conversation with a normal text message (use the existing message-seeding path), GET `/api/conversations/{id}/messages/{msgId}/media` → status `404`.
2. **Image message, no token → 404:** seed a message with `content_type = 'image'` and a non-empty `platform_message_id` (direct `INSERT INTO messages (...)` via the test pool is fine — mirror the columns the existing tests insert), GET its `/media` → `404` (the route reaches the token check and bails without any network call).
3. **Unknown message id → 404:** GET `/media` for a random `msgId` under a real conversation → `404`.

If the file already has a "forbidden agent" fixture (an agent who fails `can_act_on`), also assert that agent gets `403` from the media route; if no such fixture exists, skip the 403 case and note it in the commit body.

Keep the tests consistent with the file's style (same request helpers, same assertion macros).

- [ ] **Step 7: Run the suite**

- `cd backend && cargo build --tests 2>&1 | tail -5` → success.
- `cd backend && cargo test --test conversations 2>&1 | grep -E "Running|test result|error\[|FAILED"` → green, including the new tests.

- [ ] **Step 8: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add backend/src/domain/conversations/channels.rs backend/src/domain/conversations/handlers.rs backend/src/domain/conversations/mod.rs backend/tests/conversations.rs
git commit -m "feat(conversations): authenticated proxy for inbound LINE media"
```

---

## Task 2: Frontend — carry messageType + media through history and realtime

**Files:**
- Modify: `frontend/src/realtime/client.ts`
- Modify: `frontend/src/pages/Inbox.tsx`

- [ ] **Step 1: Extend `readMessageEvent` (realtime)**

In `frontend/src/realtime/client.ts`, add two fields to the `IncomingMessage` interface:
```ts
  messageType: string
  media?: Record<string, unknown>
```
Then in `readMessageEvent`, compute them and include them in the returned object. The inbound realtime payload nests fields under `message` as `type` (kind) and `metadata` (the media JSON, possibly a string), while the REST shape uses `messageType` + `metadata.media` — normalize both:

```ts
export function readMessageEvent(payload: Record<string, unknown>): IncomingMessage {
  const nested = (payload.message ?? {}) as Record<string, unknown>
  const senderId = String(nested.senderId ?? payload.senderId ?? '')
  const senderType = String(nested.senderType ?? payload.senderType ?? 'customer')
  const me = session.identity()?.id
  const messageType = String(nested.type ?? payload.messageType ?? nested.messageType ?? 'text')
  let media = nested.media as Record<string, unknown> | undefined
  if (!media && nested.metadata != null) {
    let meta: Record<string, unknown> | undefined
    if (typeof nested.metadata === 'string') {
      try { meta = JSON.parse(nested.metadata) as Record<string, unknown> } catch { meta = undefined }
    } else {
      meta = nested.metadata as Record<string, unknown>
    }
    // REST nests under metadata.media; the realtime inbound payload puts the
    // media object directly in metadata — accept either.
    media = (meta?.media as Record<string, unknown> | undefined) ?? meta
  }
  return {
    conversationId: String(payload.conversationId ?? ''),
    id: String(nested.id ?? payload.messageId ?? ''),
    content: String(nested.content ?? payload.content ?? ''),
    senderType,
    senderId,
    timestamp: String(nested.timestamp ?? payload.timestamp ?? ''),
    isOwn: senderType === 'agent' && me != null && senderId === String(me),
    messageType,
    media,
  }
}
```

(Keep the existing imports and the rest of the file unchanged.)

- [ ] **Step 2: Extend the `Message` type + history mapping (Inbox)**

In `frontend/src/pages/Inbox.tsx`, add to `interface Message`:
```ts
  messageType?: string
  media?: Record<string, unknown>
```

In the messages fetch (the `get<{ items?: Message[]; messages?: Message[] }>(`/api/conversations/${convId}/messages`)` block), the API objects carry `messageType` and `metadata.media`. Map each item to surface `media`:
```ts
      if (resp.success && resp.data) {
        const items = (resp.data.items ?? resp.data.messages ?? []) as Array<
          Message & { metadata?: { media?: Record<string, unknown> } }
        >
        const mapped = items.map((m) => ({ ...m, media: m.media ?? m.metadata?.media }))
        setMessages([...mapped].reverse())
      } else {
        setError(resp.message ?? null)
      }
```
(`messageType` is already on the API object and carried by the spread.)

- [ ] **Step 3: Carry the fields on the realtime-appended message**

In the same file, the `onEvent('new_message', …)` handler appends `{ id, content, senderType, createdAt }`. Add the two fields:
```ts
          : [...prev, {
              id: m.id || crypto.randomUUID(),
              content: m.content,
              senderType: m.senderType,
              createdAt: m.timestamp,
              messageType: m.messageType,
              media: m.media,
            }],
```

- [ ] **Step 4: Type-check**

Run: `cd frontend && npm run build 2>&1 | tail -5` → `tsc -b` clean + vite success. (No visual change yet — Task 3 adds rendering.)

- [ ] **Step 5: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/realtime/client.ts frontend/src/pages/Inbox.tsx
git commit -m "feat(frontend): carry messageType + media through history and realtime"
```

---

## Task 3: Frontend — `MessageMedia` renderer + bubble integration

**Files:**
- Create: `frontend/src/components/MessageMedia.tsx`
- Create: `frontend/src/components/MessageMedia.test.tsx`
- Modify: `frontend/src/pages/Inbox.tsx`

- [ ] **Step 1: Write the failing component tests**

Create `frontend/src/components/MessageMedia.test.tsx`:
```tsx
import { render } from '@testing-library/react'
import { describe, it, expect } from 'vitest'
import { MessageMedia } from './MessageMedia'

const base = { convId: 'c1', msgId: 'm1', content: '[x]' }

describe('MessageMedia', () => {
  it('image → <img> pointing at the preview proxy URL', () => {
    const { container } = render(<MessageMedia {...base} messageType="image" />)
    const img = container.querySelector('img')
    expect(img?.getAttribute('src')).toBe('/api/conversations/c1/messages/m1/media/preview')
  })

  it('sticker → <img> from the LINE sticker CDN', () => {
    const { container } = render(
      <MessageMedia {...base} messageType="sticker" media={{ stickerId: '52002734' }} />,
    )
    const src = container.querySelector('img')?.getAttribute('src') ?? ''
    expect(src).toContain('stickershop.line-scdn.net')
    expect(src).toContain('52002734')
  })

  it('file → download link with the file name', () => {
    const { container } = render(
      <MessageMedia {...base} messageType="file" media={{ fileName: 'report.pdf' }} />,
    )
    const a = container.querySelector('a')
    expect(a?.getAttribute('href')).toBe('/api/conversations/c1/messages/m1/media')
    expect(a?.textContent).toContain('report.pdf')
  })

  it('text/unknown → plain content, no <img>', () => {
    const { container } = render(<MessageMedia {...base} messageType="text" content="hello" />)
    expect(container.querySelector('img')).toBeNull()
    expect(container.textContent).toContain('hello')
  })
})
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npx vitest run src/components/MessageMedia.test.tsx 2>&1 | tail -12` → FAIL (module not found).

- [ ] **Step 3: Implement `MessageMedia`**

Create `frontend/src/components/MessageMedia.tsx`:
```tsx
// Renders one message's media by kind. Downloadable LINE media (image/video/
// audio/file) loads through the authenticated proxy; stickers come from the
// public LINE CDN. Anything else falls back to the text content.
import { useState } from 'react'

export interface MessageMediaProps {
  convId: string
  msgId: string
  messageType: string
  media?: Record<string, unknown>
  content?: string
}

const MEDIA_KINDS = ['image', 'sticker', 'video', 'audio', 'file', 'location']
export function isMediaKind(t?: string): boolean {
  return !!t && MEDIA_KINDS.includes(t)
}

function stickerUrl(stickerId: string): string {
  return `https://stickershop.line-scdn.net/stickershop/v1/sticker/${stickerId}/iPhone/sticker.png`
}

function fmtSize(n: unknown): string {
  const b = typeof n === 'number' ? n : Number(n)
  if (!Number.isFinite(b) || b <= 0) return ''
  if (b < 1024) return `${b} B`
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(0)} KB`
  return `${(b / 1024 / 1024).toFixed(1)} MB`
}

export function MessageMedia({ convId, msgId, messageType, media, content }: MessageMediaProps) {
  const [failed, setFailed] = useState(false)
  const [zoom, setZoom] = useState(false)
  const mediaUrl = `/api/conversations/${convId}/messages/${msgId}/media`
  const previewUrl = `${mediaUrl}/preview`
  const text = <span>{content}</span>

  if (failed) return text

  switch (messageType) {
    case 'image':
      return (
        <>
          <img
            className="cs-media-img"
            src={previewUrl}
            alt={content ?? 'image'}
            onClick={() => setZoom(true)}
            onError={() => setFailed(true)}
            style={{ maxWidth: 240, maxHeight: 240, borderRadius: 10, cursor: 'zoom-in', display: 'block' }}
          />
          {zoom && (
            <div
              onClick={() => setZoom(false)}
              style={{
                position: 'fixed', inset: 0, background: 'rgba(0,0,0,.8)', display: 'flex',
                alignItems: 'center', justifyContent: 'center', zIndex: 1000, cursor: 'zoom-out',
              }}
            >
              <img src={mediaUrl} alt={content ?? 'image'} style={{ maxWidth: '90vw', maxHeight: '90vh' }} />
            </div>
          )}
        </>
      )
    case 'sticker': {
      const sid = media?.stickerId != null ? String(media.stickerId) : ''
      if (!sid) return text
      return (
        <img
          src={stickerUrl(sid)}
          alt="sticker"
          onError={() => setFailed(true)}
          style={{ width: 120, height: 120, objectFit: 'contain', display: 'block' }}
        />
      )
    }
    case 'video':
      return (
        <video
          className="cs-media-video"
          src={mediaUrl}
          controls
          preload="metadata"
          onError={() => setFailed(true)}
          style={{ maxWidth: 280, borderRadius: 10, display: 'block' }}
        />
      )
    case 'audio':
      return <audio src={mediaUrl} controls onError={() => setFailed(true)} />
    case 'file': {
      const name = media?.fileName != null ? String(media.fileName) : 'file'
      const size = fmtSize(media?.fileSize)
      return (
        <a href={mediaUrl} download={name} className="cs-media-file" style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
          📄 <span>{name}</span>{size && <span style={{ opacity: 0.6 }}>{size}</span>}
        </a>
      )
    }
    case 'location': {
      const lat = media?.latitude
      const lng = media?.longitude
      if (lat == null || lng == null) return text
      return (
        <a href={`https://www.google.com/maps?q=${lat},${lng}`} target="_blank" rel="noreferrer">
          📍 {content || 'Location'}
        </a>
      )
    }
    default:
      return text
  }
}
```

- [ ] **Step 4: Run the component tests**

Run: `cd frontend && npx vitest run src/components/MessageMedia.test.tsx 2>&1 | tail -10` → PASS (4/4).

- [ ] **Step 5: Integrate into the Inbox thread bubble**

In `frontend/src/pages/Inbox.tsx`, import the component near the other component imports:
```ts
import { MessageMedia, isMediaKind } from '../components/MessageMedia'
```

Find the thread bubble render (the block that renders `<div className={`cs-bubble${isMe ? ' cs-bubble--me' : ''}`}>{msg.content}</div>`). Replace that single `<div className="cs-bubble…">{msg.content}</div>` with:

```tsx
                {isMediaKind(msg.messageType) ? (
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

(`convId` is in scope in the `Thread` component. Stickers render bare — without the `cs-bubble` background — so they float like in messaging apps.)

- [ ] **Step 6: Build + full suite**

- `cd frontend && npm run build 2>&1 | tail -6` → `tsc -b` clean + vite success.
- `cd frontend && npx vitest run 2>&1 | tail -8` → green (incl. the 4 MessageMedia tests).

- [ ] **Step 7: Commit**

```bash
cd /Users/kkllzz_0/support-center
git add frontend/src/components/MessageMedia.tsx frontend/src/components/MessageMedia.test.tsx frontend/src/pages/Inbox.tsx
git commit -m "feat(frontend): render inbound media bubbles (image/sticker/video/audio/file/location)"
```

---

## Final verification (after all tasks)

- [ ] `cd backend && cargo build && cargo build --tests && cargo test 2>&1 | grep -E "test result|error\[" | tail -20` — all green.
- [ ] `cd frontend && npm run build && npx vitest run 2>&1 | tail -6` — green.
- [ ] `detect_changes()` before the final review — `send_batch` and `message_view` must NOT be in the changed set (proxy is additive; the message API already exposed `messageType`/`metadata.media`).
- [ ] Manual live check against the real LINE OA: a LINE user sends a **photo** (renders inline, click → lightbox), a **sticker** (renders from CDN), and a **file** (download chip). Confirm an agent without access to the conversation gets 403 from the media route.

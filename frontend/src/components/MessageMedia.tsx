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

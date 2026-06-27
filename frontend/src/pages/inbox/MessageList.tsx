import type { RefObject } from 'react'

import { Avatar } from '../../components/Avatar'
import { MessageMedia, isMediaKind, kindFromMime } from '../../components/MessageMedia'
import type { InboxMessage } from './types'

function dayLabel(iso?: string): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const now = new Date()
  if (
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  ) {
    return `今天 · ${d.getMonth() + 1} 月 ${d.getDate()} 日`
  }
  return d.toLocaleDateString('zh-TW', { month: 'long', day: 'numeric' })
}

export function MessageList({
  convId,
  messages,
  error,
  customerName,
  customerAvatarUrl,
  bottomRef,
}: {
  convId: string
  messages: InboxMessage[]
  error: string | null
  customerName: string
  customerAvatarUrl?: string
  bottomRef: RefObject<HTMLDivElement>
}) {
  const messagesWithSeps: Array<{ type: 'sep'; label: string } | { type: 'msg'; msg: InboxMessage }> = []
  let lastDay = ''
  for (const message of messages) {
    const day = message.createdAt ? new Date(message.createdAt).toDateString() : ''
    if (day && day !== lastDay) {
      messagesWithSeps.push({ type: 'sep', label: dayLabel(message.createdAt) })
      lastDay = day
    }
    messagesWithSeps.push({ type: 'msg', msg: message })
  }

  return (
    <div className="cs-thread-body" style={{ overflowY: 'auto' }}>
      {error && (
        <p role="alert" style={{ color: 'crimson', fontSize: 13 }}>{error}</p>
      )}
      {messagesWithSeps.map((item, index) => {
        if (item.type === 'sep') {
          return <div key={`sep-${index}`} className="cs-day-sep">{item.label}</div>
        }
        return (
          <MessageRow
            key={item.msg.id}
            convId={convId}
            message={item.msg}
            customerName={customerName}
            customerAvatarUrl={customerAvatarUrl}
          />
        )
      })}
      <div ref={bottomRef} />
    </div>
  )
}

function MessageRow({
  convId,
  message,
  customerName,
  customerAvatarUrl,
}: {
  convId: string
  message: InboxMessage
  customerName: string
  customerAvatarUrl?: string
}) {
  const isMe = message.senderType === 'agent'
  return (
    <div
      className={`cs-bubble-row${isMe ? ' cs-bubble-row--me' : ''}`}
      style={{ opacity: message.pending ? 0.55 : 1 }}
    >
      {!isMe && (
        <Avatar name={customerName || '?'} src={customerAvatarUrl} size="sm" />
      )}
      <div>
        <MessageContent convId={convId} message={message} isMe={isMe} />
        <div
          className="cs-bubble-time"
          style={{ textAlign: isMe ? 'right' : 'left' }}
        >
          {message.createdAt
            ? new Date(message.createdAt).toLocaleTimeString('zh-TW', {
                hour: '2-digit',
                minute: '2-digit',
                hour12: false,
              })
            : ''}
          {isMe && !message.pending && ' · 已讀'}
        </div>
      </div>
    </div>
  )
}

function MessageContent({
  convId,
  message,
  isMe,
}: {
  convId: string
  message: InboxMessage
  isMe: boolean
}) {
  if (message.attachments && message.attachments.length > 0) {
    return (
      <div className={`cs-bubble${isMe ? ' cs-bubble--me' : ''}`}>
        {message.content && <div style={{ marginBottom: 6 }}>{message.content}</div>}
        {message.attachments.map((attachment) => (
          <MessageMedia
            key={attachment.id}
            convId={convId}
            msgId={message.id}
            messageType={kindFromMime(attachment.mimeType)}
            srcUrl={attachment.url}
            content={attachment.filename}
          />
        ))}
      </div>
    )
  }

  if (isMediaKind(message.messageType)) {
    if (message.messageType === 'sticker') {
      return (
        <MessageMedia
          convId={convId}
          msgId={message.id}
          messageType={message.messageType}
          media={message.media}
          content={message.content}
        />
      )
    }
    return (
      <div className={`cs-bubble${isMe ? ' cs-bubble--me' : ''}`}>
        <MessageMedia
          convId={convId}
          msgId={message.id}
          messageType={message.messageType!}
          media={message.media}
          content={message.content}
        />
      </div>
    )
  }

  return <div className={`cs-bubble${isMe ? ' cs-bubble--me' : ''}`}>{message.content}</div>
}

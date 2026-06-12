// Single-conversation screen (CRD §8.2): message history, sending with
// optimistic append, realtime updates via the shared channel.

import { useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'

import { get, post } from '../api/client'
import { onEvent, subscribeConversation } from '../realtime/client'
import { session } from '../auth/session'

interface Message {
  id: string
  content?: string
  senderType?: string
  senderName?: string
  createdAt?: string
  pending?: boolean
}

export default function ConversationDetail() {
  const { id } = useParams<{ id: string }>()
  const [messages, setMessages] = useState<Message[]>([])
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const bottom = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!id) return
    void get<{ items?: Message[]; messages?: Message[] }>(
      `/api/conversations/${id}/messages`,
    ).then((resp) => {
      if (resp.success && resp.data) {
        const items = resp.data.items ?? resp.data.messages ?? []
        setMessages([...items].reverse())
      } else {
        setError(resp.message ?? null)
      }
    })
    subscribeConversation(id)
    // Realtime reconciliation: append pushed messages for this conversation.
    return onEvent('new_message', (payload) => {
      if (String(payload.conversationId) !== id) return
      const m = (payload.message ?? {}) as Record<string, unknown>
      setMessages((prev) =>
        prev.some((x) => x.id === m.id)
          ? prev
          : [...prev, {
              id: String(m.id ?? crypto.randomUUID()),
              content: String(m.content ?? ''),
              senderType: String(m.senderType ?? 'customer'),
              createdAt: String(m.timestamp ?? ''),
            }],
      )
    })
  }, [id])

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages.length])

  const send = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!id || !draft.trim()) return
    const text = draft.trim()
    setDraft('')
    // Optimistic append; replaced or reverted after the server answers.
    const tempId = `pending-${Date.now()}`
    const who = session.identity()
    setMessages((prev) => [...prev, {
      id: tempId, content: text, senderType: 'agent',
      senderName: who?.displayName, pending: true,
    }])
    const resp = await post<{ message?: Message; id?: string }>(
      `/api/conversations/${id}/messages`,
      { content: text },
    )
    if (resp.success) {
      const confirmed = resp.data?.message ?? { id: resp.data?.id ?? tempId, content: text }
      setMessages((prev) => prev.map((m) => (m.id === tempId ? { ...m, ...confirmed, pending: false } : m)))
    } else {
      setMessages((prev) => prev.filter((m) => m.id !== tempId)) // rollback
      setError(resp.message ?? null)
      setDraft(text)
    }
  }

  return (
    <main style={{ maxWidth: 720, margin: '3vh auto', fontFamily: 'sans-serif' }}>
      <h1>對話 {id}</h1>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      <div style={{ height: '60vh', overflowY: 'auto', border: '1px solid #eee', padding: 8 }}>
        {messages.map((m) => (
          <div key={m.id} style={{
            textAlign: m.senderType === 'customer' ? 'left' : 'right',
            opacity: m.pending ? 0.5 : 1, margin: '4px 0',
          }}>
            <span style={{
              display: 'inline-block', padding: '6px 10px', borderRadius: 12,
              background: m.senderType === 'customer' ? '#f0f0f0' : '#d2e9ff',
            }}>
              {m.content}
            </span>
          </div>
        ))}
        <div ref={bottom} />
      </div>
      <form onSubmit={send} style={{ display: 'flex', gap: 8, marginTop: 8 }}>
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="輸入訊息…"
          style={{ flex: 1 }}
        />
        <button type="submit">送出</button>
      </form>
    </main>
  )
}

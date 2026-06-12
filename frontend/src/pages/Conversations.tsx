// Conversation list screen wired to the conversations store.

import { useEffect } from 'react'
import { useNavigate } from 'react-router-dom'

import { conversationsStore, loadConversations, markConversationRead } from '../stores/conversations'
import { useStore } from '../stores/store'

export default function Conversations() {
  const navigate = useNavigate()
  const state = useStore(conversationsStore)
  useEffect(() => {
    void loadConversations()
  }, [])
  return (
    <main style={{ maxWidth: 720, margin: '5vh auto', fontFamily: 'sans-serif' }}>
      <h1>對話</h1>
      {state.busy && <p>載入中…</p>}
      {state.error && <p role="alert" style={{ color: 'crimson' }}>{state.error}</p>}
      <ul style={{ listStyle: 'none', padding: 0 }}>
        {state.items.map((c) => (
          <li
            key={c.id}
            style={{ padding: 8, borderBottom: '1px solid #eee', cursor: 'pointer' }}
            onClick={() => { void markConversationRead(c.id); navigate(`/conversations/${c.id}`) }}
          >
            <strong>{c.customerName ?? c.id}</strong>
            {(c.unreadCount ?? 0) > 0 && (
              <span style={{ marginLeft: 8, color: 'white', background: 'crimson',
                             borderRadius: 8, padding: '0 6px' }}>
                {c.unreadCount}
              </span>
            )}
            <div style={{ color: '#666', fontSize: 13 }}>{c.lastMessage}</div>
          </li>
        ))}
      </ul>
      <p>共 {state.total} 筆</p>
    </main>
  )
}

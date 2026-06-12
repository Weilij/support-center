// Notification center screen (CRD §8.2).

import { useEffect } from 'react'

import { notificationsStore, loadNotifications, markRead, markAllRead } from '../stores/notifications'
import { useStore } from '../stores/store'

export default function Notifications() {
  const state = useStore(notificationsStore)
  useEffect(() => {
    void loadNotifications()
  }, [])
  return (
    <main style={{ maxWidth: 720, margin: '5vh auto', fontFamily: 'sans-serif' }}>
      <h1>通知中心 {state.unread > 0 && <small>（未讀 {state.unread}）</small>}</h1>
      {state.error && <p role="alert" style={{ color: 'crimson' }}>{state.error}</p>}
      <button onClick={() => void markAllRead()} disabled={state.unread === 0}>
        全部標示為已讀
      </button>
      <ul style={{ listStyle: 'none', padding: 0 }}>
        {state.items.map((n) => (
          <li
            key={n.id}
            onClick={() => { if (!n.isRead) void markRead(n.id) }}
            style={{
              padding: 8, borderBottom: '1px solid #eee', cursor: 'pointer',
              fontWeight: n.isRead ? 'normal' : 'bold',
            }}
          >
            <span>{n.title}</span>
            {n.priority === 'high' || n.priority === 'urgent' ? ' ❗' : ''}
            <div style={{ color: '#666', fontSize: 13, fontWeight: 'normal' }}>{n.content}</div>
          </li>
        ))}
      </ul>
    </main>
  )
}

// Notification center (CRD §8.2) + admin broadcast composer and delivery stats
// (Phase 3.2).

import { useEffect, useState } from 'react'

import { notificationsStore, loadNotifications, markRead, markAllRead } from '../stores/notifications'
import { useStore } from '../stores/store'
import { get, post } from '../api/client'
import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { StatCard, Toast } from '../components/ui'
import { PageHeader } from '../components/PageHeader'
import { Card } from '../components/Card'

interface NotifStats {
  total?: number
  unread?: number
  sent?: number
  delivered?: number
  failed?: number
}

export default function Notifications() {
  const state = useStore(notificationsStore)
  const canAccessSystem = can(session.position(), 'system')
  const [stats, setStats] = useState<NotifStats>({})
  const [title, setTitle] = useState('')
  const [content, setContent] = useState('')
  const [priority, setPriority] = useState('normal')
  const [toast, setToast] = useState<string | null>(null)

  useEffect(() => {
    void loadNotifications()
    if (canAccessSystem) void get<NotifStats>('/api/notifications/stats').then((r) => r.success && r.data && setStats(r.data))
  }, [canAccessSystem])

  const broadcast = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!title.trim() || !content.trim()) return
    const resp = await post<{ recipientCount?: number }>('/api/notifications/broadcast', {
      title: title.trim(),
      content: content.trim(),
      priority,
    })
    if (resp.success) {
      setToast(`已廣播給 ${resp.data?.recipientCount ?? 0} 位使用者`)
      setTitle('')
      setContent('')
    } else {
      setToast(resp.message ?? '廣播失敗')
    }
  }

  const pageTitle = (
    <>
      通知中心{state.unread > 0 && <small style={{ fontWeight: 400, fontSize: '0.7em', marginLeft: 8, color: 'var(--muted)' }}>（未讀 {state.unread}）</small>}
    </>
  )

  const markAllActions = (
    <button onClick={() => void markAllRead()} disabled={state.unread === 0}>
      全部標示為已讀
    </button>
  )

  return (
    <div style={{ maxWidth: 720, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title={pageTitle} actions={markAllActions} />

      {canAccessSystem && (
        <>
          <div style={{ display: 'flex', gap: 'var(--sp-4)', flexWrap: 'wrap', marginBottom: 'var(--sp-5)' }}>
            <StatCard label="總通知" value={stats.total ?? 0} />
            <StatCard label="未讀" value={stats.unread ?? 0} />
            <StatCard label="已送達" value={stats.delivered ?? stats.sent ?? 0} />
            <StatCard label="失敗" value={stats.failed ?? 0} />
          </div>
          <Card title="廣播通知" style={{ marginBottom: 'var(--sp-5)' }}>
            <form onSubmit={broadcast} style={{ display: 'grid', gap: 'var(--sp-3)' }}>
              <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="標題" />
              <textarea value={content} onChange={(e) => setContent(e.target.value)} placeholder="內容" style={{ minHeight: 60 }} />
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <select value={priority} onChange={(e) => setPriority(e.target.value)}>
                  <option value="normal">一般</option>
                  <option value="high">高</option>
                  <option value="urgent">緊急</option>
                </select>
                <button type="submit">發送廣播</button>
              </div>
            </form>
          </Card>
        </>
      )}

      {state.error && <p role="alert" style={{ color: 'crimson' }}>{state.error}</p>}

      <Card>
        <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
          {state.items.map((n) => (
            <li
              key={n.id}
              onClick={() => {
                if (!n.isRead) void markRead(n.id)
              }}
              style={{
                padding: '10px 0',
                borderBottom: '1px solid var(--hairline)',
                cursor: 'pointer',
                fontWeight: n.isRead ? 'normal' : 'bold',
              }}
            >
              <span>{n.title}</span>
              {n.priority === 'high' || n.priority === 'urgent' ? ' ❗' : ''}
              <div style={{ color: 'var(--muted)', fontSize: 13, fontWeight: 'normal' }}>{n.content}</div>
            </li>
          ))}
        </ul>
      </Card>

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </div>
  )
}

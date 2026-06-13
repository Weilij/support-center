// Shared authenticated shell: grouped navigation, unread badge, logout
// (CRD §8.2). Nav is data-driven (NAV_GROUPS) so later feature epics slot new
// destinations into a group without touching layout. Items are gated by `area`
// against the current user's position; `badge` injects a live counter.

import { Link, useNavigate } from 'react-router-dom'

import { can, type Area } from './auth/permissions'
import { session, authChanged } from './auth/session'
import { notificationsStore } from './stores/notifications'
import { useStore } from './stores/store'

interface NavItem {
  to: string
  label: string
  area: Area
  badge?: 'unread'
}

const NAV_GROUPS: { title: string; items: NavItem[] }[] = [
  {
    title: '日常',
    items: [
      { to: '/dashboard', label: '儀表板', area: 'daily' },
      { to: '/conversations', label: '對話', area: 'daily' },
      { to: '/customers', label: '客戶', area: 'daily' },
      { to: '/messages/search', label: '訊息搜尋', area: 'daily' },
      { to: '/reminders', label: '提醒', area: 'daily' },
      { to: '/auto-reply', label: '自動回覆', area: 'daily' },
      { to: '/tags', label: '標籤', area: 'daily' },
      { to: '/notifications', label: '通知', area: 'daily', badge: 'unread' },
    ],
  },
  {
    title: '營運管理',
    items: [
      { to: '/agents', label: '客服人員', area: 'ops' },
      { to: '/teams', label: '團隊', area: 'ops' },
      { to: '/sessions', label: '工作階段', area: 'ops' },
    ],
  },
  {
    title: '分析',
    items: [
      { to: '/analytics', label: '數據分析', area: 'analytics' },
      { to: '/reports', label: '報表', area: 'analytics' },
      { to: '/activity', label: '活動日誌', area: 'analytics' },
    ],
  },
  {
    title: '系統',
    items: [
      { to: '/system/monitoring', label: '監控', area: 'system' },
      { to: '/system/alerts', label: '告警', area: 'system' },
      { to: '/system/maintenance', label: '維護', area: 'system' },
      { to: '/liff', label: 'LIFF', area: 'system' },
      { to: '/settings', label: '設定', area: 'system' },
    ],
  },
]

export default function Shell({ children }: { children: React.ReactNode }) {
  const navigate = useNavigate()
  const notifications = useStore(notificationsStore)
  const who = session.identity()
  const pos = session.position()

  const logout = async () => {
    const sessionId = session.sessionId()
    // Server logout is best-effort (CRD §8.1 sign-out: failures ignored).
    try {
      await fetch('/api/auth/logout', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${session.accessToken() ?? ''}`,
          'X-Session-ID': sessionId ?? '',
        },
        body: JSON.stringify({ refreshToken: session.refreshToken() }),
      })
    } catch { /* ignored */ }
    session.clear()
    authChanged.emit()
    navigate('/login', { replace: true })
  }

  const badgeText = (item: NavItem) =>
    item.badge === 'unread' && notifications.unread > 0 ? ` (${notifications.unread})` : ''

  return (
    <div style={{ fontFamily: 'sans-serif' }}>
      <nav
        style={{
          display: 'flex',
          gap: 20,
          alignItems: 'center',
          flexWrap: 'wrap',
          padding: '10px 16px',
          position: 'sticky', top: 0, zIndex: 100,
          background: 'var(--glass-bg)',
          backdropFilter: 'blur(var(--glass-blur))',
          WebkitBackdropFilter: 'blur(var(--glass-blur))',
          borderBottom: '1px solid var(--glass-border)',
          boxShadow: 'var(--shadow)',
        }}
      >
        {NAV_GROUPS.map((group) => {
          const visible = group.items.filter((i) => can(pos, i.area))
          if (visible.length === 0) return null
          return (
            <div key={group.title} style={{ display: 'flex', gap: 12, alignItems: 'center' }}>
              <span style={{ fontSize: 11, color: '#aaa', textTransform: 'uppercase' }}>
                {group.title}
              </span>
              {visible.map((item) => (
                <Link key={item.to} to={item.to}>
                  {item.label}
                  {badgeText(item)}
                </Link>
              ))}
            </div>
          )
        })}
        <span style={{ marginLeft: 'auto' }}>{who?.displayName}</span>
        <button onClick={() => void logout()}>登出</button>
      </nav>
      {children}
    </div>
  )
}

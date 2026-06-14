// Sidebar + header app shell (visual redesign Task R2).
// Replaces the top-nav Shell with a fixed-height "refined glass" layout:
//   - Left sidebar (~240px): brand, grouped nav (position-gated, unread badge)
//   - Top header bar: page title, notifications pill, user avatar + name, logout
//   - Content area: children on gradient background (no extra card)

import { Link, useLocation, useNavigate } from 'react-router-dom'

import { can, type Area } from '../auth/permissions'
import { session, authChanged } from '../auth/session'
import { notificationsStore } from '../stores/notifications'
import { useStore } from '../stores/store'

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

// Active-nav logic: exact match, plus /conversations/:id counts as /conversations.
function isActive(pathname: string, to: string): boolean {
  if (pathname === to) return true
  if (to === '/conversations' && pathname.startsWith('/conversations/')) return true
  return false
}

export default function AppShell({
  title,
  children,
}: {
  title?: string
  children: React.ReactNode
}) {
  const navigate = useNavigate()
  const location = useLocation()
  const notifications = useStore(notificationsStore)
  const who = session.identity()
  const pos = session.position()

  const logout = async () => {
    // Server logout is best-effort (CRD §8.1 sign-out: failures ignored).
    // The backend clears the HttpOnly cookies via the mcss_refresh cookie;
    // we send X-CSRF-Token from the readable mcss_csrf cookie.
    try {
      const csrf = document.cookie
        .split('; ')
        .find((row) => row.startsWith('mcss_csrf='))
        ?.split('=')[1]
      await fetch('/api/auth/logout', {
        method: 'POST',
        credentials: 'include',
        headers: {
          'Content-Type': 'application/json',
          ...(csrf ? { 'X-CSRF-Token': decodeURIComponent(csrf) } : {}),
        },
      })
    } catch { /* ignored */ }
    session.clear()
    authChanged.emit()
    navigate('/login', { replace: true })
  }

  // Sidebar glass surface styles (frosted floating card).
  const sidebarStyle: React.CSSProperties = {
    width: 240,
    flexShrink: 0,
    margin: 16,
    borderRadius: 20,
    background: 'var(--surface)',
    backdropFilter: 'blur(var(--blur))',
    WebkitBackdropFilter: 'blur(var(--blur))',
    border: '1px solid var(--surface-border)',
    boxShadow: 'var(--shadow)',
    display: 'flex',
    flexDirection: 'column',
    overflowY: 'auto',
    padding: '20px 12px',
    gap: 0,
  }

  return (
    // Full-viewport fixed-height flex wrapper
    <div
      style={{
        display: 'flex',
        height: '100vh',
        overflow: 'hidden',
      }}
    >
      {/* ── Sidebar ── */}
      <aside style={sidebarStyle}>
        {/* Brand row */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 10,
            marginBottom: 24,
            paddingLeft: 4,
          }}
        >
          <div
            style={{
              width: 30,
              height: 30,
              borderRadius: 8,
              background: 'linear-gradient(135deg,#6366f1,#3b82f6)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: '#fff',
              fontWeight: 700,
              fontSize: 15,
              flexShrink: 0,
            }}
          >
            客
          </div>
          <span style={{ fontWeight: 700, fontSize: 15, color: 'var(--text)' }}>客服中心</span>
        </div>

        {/* Navigation groups */}
        {NAV_GROUPS.map((group) => {
          const visible = group.items.filter((i) => can(pos, i.area))
          if (visible.length === 0) return null
          return (
            <div key={group.title} style={{ marginBottom: 20 }}>
              {/* Group label */}
              <div
                style={{
                  fontSize: 11,
                  color: 'var(--muted)',
                  textTransform: 'uppercase',
                  letterSpacing: '0.08em',
                  fontWeight: 600,
                  padding: '0 10px',
                  marginBottom: 4,
                }}
              >
                {group.title}
              </div>
              {/* Nav items */}
              {visible.map((item) => {
                const active = isActive(location.pathname, item.to)
                const unreadCount =
                  item.badge === 'unread' && notifications.unread > 0
                    ? notifications.unread
                    : 0
                return (
                  <Link
                    key={item.to}
                    to={item.to}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      padding: '8px 10px',
                      borderRadius: 'var(--radius-sm)',
                      textDecoration: 'none',
                      color: active ? 'var(--accent)' : 'var(--muted)',
                      background: active ? 'var(--surface-strong)' : 'transparent',
                      fontWeight: active ? 600 : 400,
                      fontSize: 14,
                      marginBottom: 2,
                      transition: 'background 0.12s ease, color 0.12s ease',
                    }}
                  >
                    <span>{item.label}</span>
                    {unreadCount > 0 && (
                      <span
                        style={{
                          background: 'var(--accent)',
                          color: '#fff',
                          borderRadius: 999,
                          fontSize: 11,
                          fontWeight: 700,
                          padding: '1px 6px',
                          minWidth: 18,
                          textAlign: 'center',
                        }}
                      >
                        {unreadCount}
                      </span>
                    )}
                  </Link>
                )
              })}
            </div>
          )
        })}
      </aside>

      {/* ── Main column ── */}
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0, overflow: 'hidden' }}>
        {/* Header bar */}
        <header
          style={{
            margin: '16px 16px 0',
            borderRadius: 18,
            background: 'var(--surface)',
            backdropFilter: 'blur(var(--blur))',
            WebkitBackdropFilter: 'blur(var(--blur))',
            border: '1px solid var(--surface-border)',
            boxShadow: 'var(--shadow)',
            padding: '0 22px',
            height: 60,
            display: 'flex',
            alignItems: 'center',
            flexShrink: 0,
            gap: 12,
          }}
        >
          {/* Page title */}
          <h1
            style={{
              margin: 0,
              fontSize: 18,
              fontWeight: 700,
              color: 'var(--text)',
              whiteSpace: 'nowrap',
            }}
          >
            {title ?? ''}
          </h1>

          {/* Right side */}
          <div style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 12 }}>
            {/* Notifications pill */}
            {notifications.unread > 0 && (
              <Link
                to="/notifications"
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 5,
                  padding: '5px 12px',
                  borderRadius: 999,
                  background: 'var(--accent-soft)',
                  color: 'var(--accent)',
                  fontWeight: 600,
                  fontSize: 13,
                  textDecoration: 'none',
                }}
              >
                🔔 {notifications.unread}
              </Link>
            )}

            {/* User avatar + display name */}
            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              <div
                style={{
                  width: 30,
                  height: 30,
                  borderRadius: '50%',
                  background: 'linear-gradient(135deg,#6366f1,#3b82f6)',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  color: '#fff',
                  fontWeight: 700,
                  fontSize: 13,
                  flexShrink: 0,
                }}
              >
                {who?.displayName?.[0] ?? '?'}
              </div>
              <span style={{ fontSize: 14, color: 'var(--text)', fontWeight: 500 }}>
                {who?.displayName}
              </span>
            </div>

            {/* Logout button */}
            <button
              onClick={() => void logout()}
              style={{ fontSize: 13, padding: '5px 12px' }}
            >
              登出
            </button>
          </div>
        </header>

        {/* Content area */}
        <main
          style={{
            flex: 1,
            overflowY: 'auto',
            padding: 16,
          }}
        >
          {children}
        </main>
      </div>
    </div>
  )
}

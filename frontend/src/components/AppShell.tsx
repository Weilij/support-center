// AppShell — light sidebar + topbar layout (clean-light redesign Task N2).
// Layout: fixed left sidebar (.cs-side) + main column (.cs-main) with topbar (.cs-topbar).
// All behaviour from previous version is preserved:
//   - NAV_GROUPS with position gating via can(pos, area)
//   - Active nav detection (exact + /conversations/:id → /conversations)
//   - Unread badge from notificationsStore
//   - logout (CSRF-safe, best-effort server call)
//   - Responsive isNarrow / drawerOpen mobile drawer

import { useEffect, useState } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { can, type Area } from '../auth/permissions'
import { session, authChanged } from '../auth/session'
import { useHotkeys } from '../hooks/useHotkeys'
import { notificationsStore } from '../stores/notifications'
import { useStore } from '../stores/store'
import { loadTeams, teamsStore } from '../stores/teams'
import { Icon } from './Icon'
import { ThemeToggle } from './ThemeToggle'

// ── Icon mapping per route ──────────────────────────────────────────────────
const ROUTE_ICON: Record<string, string> = {
  '/dashboard':        'grid',
  '/conversations':    'inbox',
  '/customers':        'users',
  '/messages/search':  'search',
  '/reminders':        'clock',
  '/auto-reply':       'bot',
  '/tags':             'tag',
  '/notifications':    'bell',
  '/agents':           'user',
  '/teams':            'users',
  '/sessions':         'chat',
  '/analytics':        'chart',
  '/reports':          'chart',
  '/activity':         'dots',
  '/system/monitoring':'chart',
  '/system/alerts':    'bell',
  '/system/maintenance':'settings',
  '/channels':         'channels',
  '/liff':             'channels',
  '/settings':         'settings',
}

interface NavItem {
  to: string
  label: string
  area: Area
  badge?: 'unread'
  show?: () => boolean
}

const NAV_GROUPS: { title: string; items: NavItem[] }[] = [
  {
    title: '日常',
    items: [
      { to: '/dashboard',       label: '儀表板',   area: 'daily' },
      { to: '/conversations',   label: '對話收件匣', area: 'daily' },
      { to: '/customers',       label: '客戶',     area: 'daily' },
      { to: '/messages/search', label: '訊息搜尋', area: 'daily' },
      { to: '/reminders',       label: '提醒',     area: 'daily' },
      { to: '/auto-reply',      label: '自動回覆', area: 'daily' },
      { to: '/tags',            label: '標籤',     area: 'daily' },
      { to: '/notifications',   label: '通知',     area: 'daily', badge: 'unread' },
      { to: '/teams',           label: '團隊',     area: 'daily' },
    ],
  },
  {
    title: '營運管理',
    items: [
      { to: '/agents',  label: '客服人員',   area: 'ops' },
      { to: '/sessions', label: '工作階段', area: 'ops' },
    ],
  },
  {
    title: '分析',
    items: [
      { to: '/analytics', label: '數據分析', area: 'analytics' },
      { to: '/reports',   label: '報表',     area: 'analytics' },
      { to: '/activity',  label: '活動日誌', area: 'analytics' },
    ],
  },
  {
    title: '系統',
    items: [
      { to: '/system/monitoring',  label: '監控', area: 'system' },
      { to: '/system/alerts',      label: '告警', area: 'system' },
      { to: '/system/maintenance', label: '維護', area: 'system' },
      { to: '/channels',           label: '頻道管理', area: 'system' },
      { to: '/liff',               label: 'LIFF', area: 'system' },
      { to: '/settings',           label: '設定', area: 'system' },
    ],
  },
]

// Active-nav logic: exact match, plus /conversations/:id counts as /conversations.
function isActive(pathname: string, to: string): boolean {
  if (pathname === to) return true
  if (to === '/conversations' && pathname.startsWith('/conversations/')) return true
  return false
}

// Avatar — simple initials span using cs-av classes.
// Uses last-two chars of name as initials (matches handoff Avatar behaviour).
const AV_COLORS = ['#0284c7','#0d9488','#7c3aed','#db2777','#ea580c','#4f46e5','#0891b2','#65a30d']
function avColor(name: string): string {
  let h = 0
  for (const ch of name) h = ((h * 31 + ch.charCodeAt(0)) >>> 0)
  return AV_COLORS[h % AV_COLORS.length]
}

function SidebarAvatar({ name, size = 'sm' }: { name: string; size?: 'sm' | 'md' | 'lg' }) {
  const initials = name.slice(-2)
  return (
    <span className={`cs-av cs-av-${size}`} style={{ background: avColor(name) }}>
      {initials}
    </span>
  )
}

// ── NavGroups: shared nav groups used in sidebar and mobile drawer ──────────
export function NavGroups({
  pathname,
  pos,
  unread,
  onLinkClick,
}: {
  pathname: string
  pos: ReturnType<typeof session.position>
  unread: number
  onLinkClick?: () => void
}) {
  return (
    <>
      {NAV_GROUPS.map((group) => {
        const visible = group.items.filter((i) => can(pos, i.area) && (i.show ? i.show() : true))
        if (visible.length === 0) return null
        return (
          <div key={group.title}>
            <div className="cs-nav-label">{group.title}</div>
            {visible.map((item) => {
              const active = isActive(pathname, item.to)
              const unreadCount = item.badge === 'unread' && unread > 0 ? unread : 0
              const iconName = ROUTE_ICON[item.to] ?? 'dots'
              return (
                <Link
                  key={item.to}
                  to={item.to}
                  onClick={onLinkClick}
                  className={`cs-nav${active ? ' cs-nav--active' : ''}`}
                >
                  <Icon name={iconName} w={19} />
                  <span>{item.label}</span>
                  {unreadCount > 0 && (
                    <span className="cs-nav-badge">{unreadCount}</span>
                  )}
                </Link>
              )
            })}
          </div>
        )
      })}
    </>
  )
}

// ── SidebarContent: brand + nav + footer ───────────────────────────────────
function SidebarContent({
  pathname,
  pos,
  unread,
  who,
  onLinkClick,
}: {
  pathname: string
  pos: ReturnType<typeof session.position>
  unread: number
  who: ReturnType<typeof session.identity>
  onLinkClick?: () => void
}) {
  const displayName = who?.displayName ?? ''
  const posLabel = pos ?? ''

  return (
    <>
      {/* Brand row */}
      <div className="cs-brand">
        <span className="cs-brand-mark">
          <Icon name="chat" w={19} />
        </span>
        <div>
          <div className="cs-brand-name">客服系統</div>
          <div className="cs-brand-sub">Support Center</div>
        </div>
      </div>

      {/* Nav groups */}
      <NavGroups
        pathname={pathname}
        pos={pos}
        unread={unread}
        onLinkClick={onLinkClick}
      />

      {/* Sidebar footer — avatar + name + position */}
      <div className="cs-side-foot">
        {displayName ? (
          <SidebarAvatar name={displayName} size="sm" />
        ) : (
          <span className="cs-av cs-av-sm" style={{ background: avColor('?') }}>?</span>
        )}
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--ink)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {displayName}
          </div>
          <div style={{ fontSize: 11, color: 'var(--muted)' }}>{posLabel}</div>
        </div>
      </div>
    </>
  )
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
  const teams = useStore(teamsStore)
  const who = session.identity()
  const pos = session.position()
  const unread = notifications.unread

  useHotkeys({
    'mod+k': (e) => {
      e.preventDefault()
      const search = document.querySelector<HTMLInputElement>('[data-inbox-search]')
      if (search) search.focus()
      else navigate('/messages/search')
    },
  })

  // ── Responsive state ──
  const [isNarrow, setIsNarrow] = useState(
    () =>
      typeof window !== 'undefined' && window.matchMedia
        ? window.matchMedia('(max-width: 1024px)').matches
        : false,
  )
  const [isMobile, setIsMobile] = useState(
    () =>
      typeof window !== 'undefined' && window.matchMedia
        ? window.matchMedia('(max-width: 640px)').matches
        : false,
  )
  const [drawerOpen, setDrawerOpen] = useState(false)

  // Subscribe to viewport width changes.
  useEffect(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return
    const mq = window.matchMedia('(max-width: 1024px)')
    const handler = (e: MediaQueryListEvent) => setIsNarrow(e.matches)
    mq.addEventListener('change', handler)
    const mqMobile = window.matchMedia('(max-width: 640px)')
    const handlerMobile = (e: MediaQueryListEvent) => setIsMobile(e.matches)
    mqMobile.addEventListener('change', handlerMobile)
    return () => {
      mq.removeEventListener('change', handler)
      mqMobile.removeEventListener('change', handlerMobile)
    }
  }, [])

  // Close the drawer on route change.
  useEffect(() => {
    setDrawerOpen(false)
  }, [location.pathname])

  useEffect(() => {
    if (session.isAdmin() && teams.items.length === 0) {
      void loadTeams()
    }
  }, [teams.items.length])

  const logout = async () => {
    // Server logout is best-effort (CRD §8.1 sign-out: failures ignored).
    // The backend looks up the session by X-Session-ID (the sessionId is not a
    // credential and is still kept), clears the HttpOnly cookies, and we send
    // X-CSRF-Token from the readable mcss_csrf cookie for the CSRF gate.
    try {
      const csrf = document.cookie
        .split('; ')
        .find((row) => row.startsWith('mcss_csrf='))
        ?.split('=').slice(1).join('=')
      await fetch('/api/auth/logout', {
        method: 'POST',
        credentials: 'include',
        headers: {
          'Content-Type': 'application/json',
          'X-Session-ID': session.sessionId() ?? '',
          ...(csrf ? { 'X-CSRF-Token': decodeURIComponent(csrf) } : {}),
        },
        // Send an empty JSON object: with a JSON content-type the backend's
        // Json extractor rejects an empty body (EOF). The refresh token is read
        // from the mcss_refresh cookie, so no fields are needed here.
        body: '{}',
      })
    } catch { /* ignored */ }
    session.clear()
    authChanged.emit()
    navigate('/login', { replace: true })
  }

  const displayName = who?.displayName ?? ''

  return (
    // Full-viewport fixed-height flex wrapper
    <div style={{ display: 'flex', height: '100vh', overflow: 'hidden' }}>

      {/* ── Desktop Sidebar (in-flow, only on wide screens) ── */}
      {!isNarrow && (
        <aside className="cs-side">
          <SidebarContent
            pathname={location.pathname}
            pos={pos}
            unread={unread}
            who={who}
          />
        </aside>
      )}

      {/* ── Mobile Drawer (fixed overlay, only on narrow screens) ── */}
      {isNarrow && (
        <>
          {/* Dim backdrop */}
          {drawerOpen && (
            <div
              onClick={() => setDrawerOpen(false)}
              style={{
                position: 'fixed',
                inset: 0,
                background: 'rgba(0,0,0,.35)',
                zIndex: 999,
              }}
            />
          )}
          {/* Slide-in drawer — mirrors .cs-side but positioned as overlay */}
          <aside
            className="cs-side"
            style={{
              position: 'fixed',
              top: 0,
              left: 0,
              height: '100vh',
              zIndex: 1000,
              transform: drawerOpen ? 'translateX(0)' : 'translateX(-110%)',
              transition: 'transform 0.25s ease',
              borderRadius: '0 16px 16px 0',
              boxShadow: 'var(--shadow-lg)',
            }}
          >
            <SidebarContent
              pathname={location.pathname}
              pos={pos}
              unread={unread}
              who={who}
              onLinkClick={() => setDrawerOpen(false)}
            />
          </aside>
        </>
      )}

      {/* ── Main column ── */}
      <div className="cs-main">
        {/* Topbar */}
        <header className="cs-topbar">
          {/* Hamburger — narrow only, far left */}
          {isNarrow && (
            <button
              className="cs-icon-btn"
              onClick={() => setDrawerOpen((o) => !o)}
              aria-label="開啟選單"
              style={{ marginRight: 4, flexShrink: 0 }}
            >
              <Icon name="filter" w={18} />
            </button>
          )}

          {/* Page title (+ optional subtitle) */}
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="cs-page-title">{title ?? ''}</div>
          </div>

          {/* Light/dark toggle */}
          <ThemeToggle />

          {/* Bell icon — links to /notifications, shows alert dot when unread > 0 */}
          <Link to="/notifications" style={{ textDecoration: 'none' }}>
            <button className="cs-icon-btn" aria-label="通知">
              <Icon name="bell" w={18} />
              {unread > 0 && <span className="cs-dot-alert" />}
            </button>
          </Link>

          {/* User avatar + display name (name hidden on mobile ≤640px) */}
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            {displayName ? (
              <SidebarAvatar name={displayName} size="sm" />
            ) : (
              <span className="cs-av cs-av-sm" style={{ background: avColor('?') }}>?</span>
            )}
            {!isMobile && (
              <span style={{ fontSize: 14, color: 'var(--ink)', fontWeight: 500, whiteSpace: 'nowrap' }}>
                {displayName}
              </span>
            )}
          </div>

          {/* Logout button */}
          <button
            className="cs-btn cs-btn--ghost"
            onClick={() => void logout()}
            style={{ fontSize: 13, padding: '5px 12px' }}
          >
            登出
          </button>
        </header>

        {/* Content area */}
        <main className="cs-content" style={{ overflowY: 'auto' }}>
          {children}
        </main>
      </div>
    </div>
  )
}

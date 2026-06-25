// SPA routing with the combined navigation guard (CRD §8.4, lines 6473-6494):
// title updates, same-path short-circuit, snapshot fast path, pending-session
// wait, guest-only and auth-required redirects, fail-closed auth guard errors.

import { useEffect, useState } from 'react'
import {
  createBrowserRouter,
  Navigate,
  Outlet,
  useLocation,
} from 'react-router-dom'

import { can, type Area } from './auth/permissions'
import { session } from './auth/session'
import { t } from './i18n'
import Login from './pages/Login'
import Dashboard from './pages/Dashboard'
import NotFound from './pages/NotFound'
import Inbox from './pages/Inbox'
import Conversations from './pages/Conversations'
import ConversationDetail from './pages/ConversationDetail'
import Customers from './pages/Customers'
import MessageSearch from './pages/MessageSearch'
import Agents from './pages/Agents'
import Sessions from './pages/Sessions'
import LiffSettings from './pages/LiffSettings'
import Analytics from './pages/Analytics'
import Reminders from './pages/Reminders'
import SystemMonitoring from './pages/SystemMonitoring'
import AlertConfig from './pages/AlertConfig'
import SystemMaintenance from './pages/SystemMaintenance'
import Notifications from './pages/Notifications'
import Tags from './pages/Tags'
import AppShell from './components/AppShell'
import Teams from './pages/Teams'
import Settings from './pages/Settings'
import ProfilePage from './pages/Profile'
import Reports from './pages/Reports'
import ActivityLog from './pages/Activity'
import AutoReply from './pages/AutoReply'
import Channels from './pages/Channels'
import Install from './pages/Install'

interface RouteMeta {
  requiresAuth?: boolean // default true (CRD 6476)
  guestOnly?: boolean
  area?: Area // access area; checked after auth (CRD 6495)
  title?: string
}

export function Guard({ meta, children }: { meta: RouteMeta; children: React.ReactNode }) {
  const location = useLocation()
  const [decision, setDecision] = useState<'pending' | 'allow' | 'toLogin' | 'toDashboard'>(
    'pending',
  )

  useEffect(() => {
    // 1. Title updates immediately (CRD 6478).
    document.title = meta.title ? `${meta.title} - ${t('app.name')}` : t('app.name')

    let cancelled = false
    const run = async () => {
      let requiresAuth = meta.requiresAuth ?? true
      try {
        // 3. Snapshot fast paths (CRD 6480).
        const snap = session.snapshot()
        if (snap !== null) {
          if (!requiresAuth && !meta.guestOnly) return setDecision('allow')
          if (snap && requiresAuth && !meta.guestOnly) {
            return setDecision(
              can(session.position(), meta.area ?? 'daily') ? 'allow' : 'toDashboard',
            )
          }
        }
        // 4. Wait for session initialization when pending (CRD 6481).
        if (session.lifecycle() === 'pending') await session.init()
        const authenticated = session.lifecycle() === 'authenticated'
        session.recordSnapshot(authenticated)
        if (cancelled) return
        // 5-7. Guest-only and auth-required rules.
        if (meta.guestOnly) {
          return setDecision(authenticated ? 'toDashboard' : 'allow')
        }
        if (requiresAuth && !authenticated) {
          return setDecision('toLogin')
        }
        if (requiresAuth && !meta.guestOnly && !can(session.position(), meta.area ?? 'daily')) {
          return setDecision('toDashboard')
        }
        setDecision('allow')
      } catch {
        // Auth-required routes must fail closed when guard evaluation throws.
        session.recordSnapshot(false)
        if (!cancelled) setDecision(requiresAuth ? 'toLogin' : 'allow')
      }
    }
    void run()
    return () => {
      cancelled = true
    }
  }, [location.pathname])

  if (decision === 'pending') return null
  if (decision === 'toLogin') return <Navigate to="/login" replace />
  if (decision === 'toDashboard') return <Navigate to="/dashboard" replace />
  return <>{children}</>
}

const page = (meta: RouteMeta, element: React.ReactNode) => (
  <Guard meta={meta}>
    {(meta.requiresAuth ?? true) && !meta.guestOnly
      ? <AppShell title={meta.title}>{element}</AppShell>
      : element}
  </Guard>
)

// Known navigable destinations and access tiers (CRD 6488-6494).
export const router = createBrowserRouter([
  { path: '/', element: <Navigate to="/dashboard" replace /> },
  {
    path: '/login',
    element: page({ requiresAuth: false, guestOnly: true, title: t('login.title') }, <Login />),
  },
  {
    path: '/dashboard',
    element: page({ title: t('dashboard.title'), area: 'daily' }, <Dashboard />),
  },
  // daily-area destinations (all authenticated positions).
  {
    path: '/conversations',
    element: page({ title: '對話收件匣', area: 'daily' }, <Inbox />),
  },
  {
    path: '/conversations/:id',
    element: page({ title: '對話收件匣', area: 'daily' }, <Inbox />),
  },
  {
    path: '/customers',
    element: page({ title: '客戶管理', area: 'daily' }, <Customers />),
  },
  {
    path: '/messages/search',
    element: page({ title: '訊息搜尋', area: 'daily' }, <MessageSearch />),
  },
  {
    path: '/notifications',
    element: page({ title: '通知中心', area: 'daily' }, <Notifications />),
  },
  {
    path: '/tags',
    element: page({ title: '標籤管理', area: 'daily' }, <Tags />),
  },
  {
    path: '/profile',
    element: page({ title: '個人資料', area: 'daily' }, <ProfilePage />),
  },
  {
    path: '/reminders',
    element: page({ title: '提醒', area: 'daily' }, <Reminders />),
  },
  {
    path: '/auto-reply',
    element: page({ title: '自動回覆', area: 'daily' }, <AutoReply />),
  },
  // ops-area destinations (supervisor and above).
  {
    path: '/agents',
    element: page({ title: '客服人員管理', area: 'ops' }, <Agents />),
  },
  {
    path: '/sessions',
    element: page({ title: '工作階段', area: 'ops' }, <Sessions />),
  },
  {
    path: '/teams',
    element: page({ title: '團隊管理', area: 'ops' }, <Teams />),
  },
  // analytics-area destinations (supervisor and above).
  {
    path: '/analytics',
    element: page({ title: '數據分析', area: 'analytics' }, <Analytics />),
  },
  {
    path: '/reports',
    element: page({ title: '報表', area: 'analytics' }, <Reports />),
  },
  {
    path: '/export',
    element: page({ title: '報表', area: 'analytics' }, <Reports />),
  },
  {
    path: '/activity',
    element: page({ title: '活動日誌', area: 'analytics' }, <ActivityLog />),
  },
  // system-area destinations (system_admin only).
  {
    path: '/settings',
    element: page({ title: '系統設定', area: 'system' }, <Settings />),
  },
  {
    path: '/channels',
    element: page({ title: '頻道管理', area: 'system' }, <Channels />),
  },
  {
    path: '/liff',
    element: page({ title: 'LIFF 設定', area: 'system' }, <LiffSettings />),
  },
  {
    path: '/system/monitoring',
    element: page({ title: '系統監控', area: 'system' }, <SystemMonitoring />),
  },
  {
    path: '/system/alerts',
    element: page({ title: '告警設定', area: 'system' }, <AlertConfig />),
  },
  {
    path: '/system/maintenance',
    element: page({ title: '系統維護', area: 'system' }, <SystemMaintenance />),
  },
  {
    path: '/install',
    element: page({ requiresAuth: false, title: '安裝精靈' }, <Install />),
  },
  {
    path: '*',
    element: page({ requiresAuth: false, title: t('notfound.title') }, <NotFound />),
  },
])

export function Layout() {
  return <Outlet />
}

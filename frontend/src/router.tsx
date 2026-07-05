// SPA routing with the combined navigation guard (CRD §8.4, lines 6473-6494):
// title updates, same-path short-circuit, snapshot fast path, pending-session
// wait, guest-only and auth-required redirects, fail-closed auth guard errors.

import { lazy, Suspense, useEffect, useState } from 'react'
import {
  createBrowserRouter,
  Navigate,
  Outlet,
  useLocation,
} from 'react-router-dom'

import { can, type Area } from './auth/permissions'
import { session } from './auth/session'
import { t } from './i18n'
import AppShell from './components/AppShell'

// Route-based code splitting: each page is its own chunk, fetched on demand behind
// the Suspense fallback below — keeps the initial bundle small.
const Login = lazy(() => import('./pages/Login'))
const Dashboard = lazy(() => import('./pages/Dashboard'))
const NotFound = lazy(() => import('./pages/NotFound'))
const Inbox = lazy(() => import('./pages/Inbox'))
const Customers = lazy(() => import('./pages/Customers'))
const MessageSearch = lazy(() => import('./pages/MessageSearch'))
const Agents = lazy(() => import('./pages/Agents'))
const Sessions = lazy(() => import('./pages/Sessions'))
const LiffSettings = lazy(() => import('./pages/LiffSettings'))
const Analytics = lazy(() => import('./pages/Analytics'))
const Reminders = lazy(() => import('./pages/Reminders'))
const SystemMonitoring = lazy(() => import('./pages/SystemMonitoring'))
const AlertConfig = lazy(() => import('./pages/AlertConfig'))
const SystemMaintenance = lazy(() => import('./pages/SystemMaintenance'))
const Notifications = lazy(() => import('./pages/Notifications'))
const Tags = lazy(() => import('./pages/Tags'))
const Teams = lazy(() => import('./pages/Teams'))
const Settings = lazy(() => import('./pages/Settings'))
const ProfilePage = lazy(() => import('./pages/Profile'))
const Reports = lazy(() => import('./pages/Reports'))
const ActivityLog = lazy(() => import('./pages/Activity'))
const AutoReply = lazy(() => import('./pages/AutoReply'))
const Channels = lazy(() => import('./pages/Channels'))
const Install = lazy(() => import('./pages/Install'))

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

// Fallback shown while a route's lazy chunk loads (matches the app's muted
// "載入中…" loading style, e.g. DataTable).
function PageLoading() {
  return (
    <div style={{ padding: '40px 20px', textAlign: 'center', color: 'var(--muted)', fontSize: 13 }}>
      載入中…
    </div>
  )
}

const page = (meta: RouteMeta, element: React.ReactNode) => (
  <Guard meta={meta}>
    {(meta.requiresAuth ?? true) && !meta.guestOnly ? (
      <AppShell title={meta.title}>
        <Suspense fallback={<PageLoading />}>{element}</Suspense>
      </AppShell>
    ) : (
      <Suspense fallback={<PageLoading />}>{element}</Suspense>
    )}
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
    // Everyone can READ the Teams page (daily area, all authenticated positions);
    // modify controls are gated inside the page (canModify / isAdmin).
    path: '/teams',
    element: page({ title: '團隊管理', area: 'daily' }, <Teams />),
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

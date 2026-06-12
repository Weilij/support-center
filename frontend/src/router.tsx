// SPA routing with the combined navigation guard (CRD §8.4, lines 6473-6494):
// title updates, same-path short-circuit, snapshot fast path, pending-session
// wait, guest-only and auth-required redirects, fail-open on guard errors.

import { useEffect, useState } from 'react'
import {
  createBrowserRouter,
  Navigate,
  Outlet,
  useLocation,
} from 'react-router-dom'

import { session } from './auth/session'
import { t } from './i18n'
import Login from './pages/Login'
import Dashboard from './pages/Dashboard'
import NotFound from './pages/NotFound'

interface RouteMeta {
  requiresAuth?: boolean // default true (CRD 6476)
  guestOnly?: boolean
  adminOnly?: boolean // metadata only; enforced by screens + backend (CRD 6495)
  title?: string
}

function Guard({ meta, children }: { meta: RouteMeta; children: React.ReactNode }) {
  const location = useLocation()
  const [decision, setDecision] = useState<'pending' | 'allow' | 'toLogin' | 'toDashboard'>(
    'pending',
  )

  useEffect(() => {
    // 1. Title updates immediately (CRD 6478).
    document.title = meta.title ? `${meta.title} - ${t('app.name')}` : t('app.name')

    let cancelled = false
    const run = async () => {
      try {
        const requiresAuth = meta.requiresAuth ?? true
        // 3. Snapshot fast paths (CRD 6480).
        const snap = session.snapshot()
        if (snap !== null) {
          if (!requiresAuth && !meta.guestOnly) return setDecision('allow')
          if (snap && requiresAuth && !meta.guestOnly) return setDecision('allow')
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
        setDecision('allow')
      } catch {
        // 8. Fail-open at the UX layer (CRD 6485).
        session.recordSnapshot(false)
        if (!cancelled) setDecision('allow')
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
  <Guard meta={meta}>{element}</Guard>
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
    element: page({ title: t('dashboard.title') }, <Dashboard />),
  },
  // Authenticated-only destinations (placeholder screens render the shared shell).
  ...['/profile', '/conversations', '/conversations/:id', '/tags', '/notifications',
      '/export', '/reports'].map((path) => ({
    path,
    element: page({ title: t('dashboard.title') }, <Dashboard />),
  })),
  // Admin-flagged destinations (admin gating happens in-screen + backend).
  ...['/teams', '/channels', '/activity', '/settings', '/auto-reply'].map((path) => ({
    path,
    element: page({ title: t('dashboard.title'), adminOnly: true }, <Dashboard />),
  })),
  {
    path: '*',
    element: page({ requiresAuth: false, title: t('notfound.title') }, <NotFound />),
  },
])

export function Layout() {
  return <Outlet />
}

// Shared authenticated shell: navigation, unread badge, logout (CRD §8.2).

import { Link, useNavigate } from 'react-router-dom'

import { post } from './api/client'
import { session, authChanged } from './auth/session'
import { notificationsStore } from './stores/notifications'
import { useStore } from './stores/store'

export default function Shell({ children }: { children: React.ReactNode }) {
  const navigate = useNavigate()
  const notifications = useStore(notificationsStore)
  const who = session.identity()

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
  void post // keep import shape stable for future use

  return (
    <div style={{ fontFamily: 'sans-serif' }}>
      <nav style={{
        display: 'flex', gap: 16, alignItems: 'center', padding: '8px 16px',
        borderBottom: '1px solid #ddd',
      }}>
        <Link to="/dashboard">儀表板</Link>
        <Link to="/conversations">對話</Link>
        <Link to="/notifications">
          通知{notifications.unread > 0 ? ` (${notifications.unread})` : ''}
        </Link>
        <Link to="/tags">標籤</Link>
        {session.isAdmin() && <Link to="/teams">團隊</Link>}
        {session.isAdmin() && <Link to="/settings">設定</Link>}
        <span style={{ marginLeft: 'auto' }}>{who?.displayName}</span>
        <button onClick={() => void logout()}>登出</button>
      </nav>
      {children}
    </div>
  )
}

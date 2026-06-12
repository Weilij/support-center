// Login screen (CRD §8.2 login flow): email/password, generic failure
// message passthrough, forced password-change branch.

import { useState } from 'react'
import { useNavigate } from 'react-router-dom'

import { post } from '../api/client'
import { session } from '../auth/session'
import { t } from '../i18n'

interface LoginData {
  token: string
  refreshToken: string
  sessionId: string
  agent: { id: string; email: string; displayName: string; role: string }
  mustChangePassword?: boolean
  tempToken?: string
}

export default function Login() {
  const navigate = useNavigate()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    const resp = await post<LoginData>(
      '/api/auth/login',
      { email, password },
      { redirectOnUnauthorized: false },
    )
    setBusy(false)
    if (!resp.success || !resp.data) {
      setError(resp.message ?? t('error.server'))
      return
    }
    if (resp.data.mustChangePassword) {
      setError(t('login.mustChange'))
      return
    }
    session.storeLogin(resp.data.token, resp.data.refreshToken, resp.data.sessionId, resp.data.agent)
    navigate('/dashboard', { replace: true })
  }

  return (
    <main style={{ maxWidth: 360, margin: '10vh auto', fontFamily: 'sans-serif' }}>
      <h1>{t('login.title')}</h1>
      <form onSubmit={submit}>
        <label style={{ display: 'block', marginBottom: 8 }}>
          {t('login.email')}
          <input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            required
            style={{ width: '100%' }}
          />
        </label>
        <label style={{ display: 'block', marginBottom: 8 }}>
          {t('login.password')}
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
            style={{ width: '100%' }}
          />
        </label>
        {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
        <button type="submit" disabled={busy}>{t('login.submit')}</button>
      </form>
    </main>
  )
}

// Login screen (CRD §8.2 login flow): email/password, generic failure
// message passthrough, forced password-change branch.

import { useState } from 'react'
import { useNavigate } from 'react-router-dom'

import { post } from '../api/client'
import { session } from '../auth/session'
import { t } from '../i18n'
import { Icon } from '../components/Icon'

interface LoginData {
  // token / refreshToken are set as HttpOnly cookies by the backend;
  // we ignore them here and let the browser handle them automatically.
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
  const [mustChange, setMustChange] = useState(false)
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
      setError(null)
      setMustChange(true)
      return
    }
    session.storeLogin(resp.data.sessionId, resp.data.agent)
    navigate('/dashboard', { replace: true })
  }

  const outerStyle: React.CSSProperties = {
    minHeight: '100vh',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    background: 'var(--bg)',
    padding: '24px',
  }

  const cardStyle: React.CSSProperties = {
    width: '100%',
    maxWidth: 380,
    background: 'var(--surface)',
    border: '1px solid var(--line)',
    borderRadius: 'var(--radius-lg)',
    boxShadow: 'var(--shadow-lg)',
    padding: '36px 32px',
  }

  const logoStyle: React.CSSProperties = {
    width: 44,
    height: 44,
    borderRadius: 12,
    background: 'linear-gradient(150deg, var(--blue-400), var(--blue-600))',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    color: '#fff',
    boxShadow: '0 4px 16px rgba(2,132,199,0.30)',
    margin: '0 auto var(--sp-3)',
  }

  const brandBlockStyle: React.CSSProperties = {
    textAlign: 'center',
    marginBottom: 'var(--sp-5)',
  }

  const titleStyle: React.CSSProperties = {
    margin: '0 0 4px',
    fontSize: 20,
    fontWeight: 700,
    color: 'var(--ink)',
  }

  const subtitleStyle: React.CSSProperties = {
    margin: 0,
    fontSize: 13,
    color: 'var(--muted)',
  }

  const labelStyle: React.CSSProperties = {
    display: 'block',
    marginBottom: 'var(--sp-4)',
    fontSize: 13,
    color: 'var(--muted)',
    fontWeight: 500,
  }

  const inputStyle: React.CSSProperties = {
    width: '100%',
    marginTop: 4,
    boxSizing: 'border-box',
  }

  return (
    <div style={outerStyle}>
      <div style={cardStyle}>
        <div style={brandBlockStyle}>
          <div style={logoStyle}>
            <Icon name="chat" w={22} />
          </div>
          <h1 style={titleStyle}>{t('login.title')}</h1>
          <p style={subtitleStyle}>登入以繼續</p>
        </div>

        {mustChange ? (
          <div>
            <div
              style={{
                background: 'var(--amber-50, #fffbeb)',
                border: '1px solid var(--amber-300, #fcd34d)',
                borderRadius: 'var(--radius-md, 10px)',
                padding: 'var(--sp-4) var(--sp-4)',
                marginBottom: 'var(--sp-5)',
              }}
            >
              <h2 style={{ margin: '0 0 var(--sp-3)', fontSize: 16, fontWeight: 700, color: 'var(--ink)' }}>
                {t('login.mustChangeTitle')}
              </h2>
              <p style={{ margin: '0 0 var(--sp-3)', fontSize: 13, color: 'var(--ink)' }}>{t('login.mustChange')}</p>
              <p style={{ margin: 0, fontSize: 13, color: 'var(--muted)' }}>{t('login.mustChangeHint')}</p>
            </div>
            <button
              type="button"
              onClick={() => { setMustChange(false); setPassword('') }}
              className="cs-btn cs-btn--primary"
              style={{ width: '100%', justifyContent: 'center' }}
            >
              {t('login.backToLogin')}
            </button>
          </div>
        ) : (
          <form onSubmit={submit}>
            <label style={labelStyle}>
              {t('login.email')}
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                style={inputStyle}
              />
            </label>
            <label style={labelStyle}>
              {t('login.password')}
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
                style={inputStyle}
              />
            </label>
            {error && <p role="alert" style={{ color: 'crimson', margin: '0 0 var(--sp-4)', fontSize: 13 }}>{error}</p>}
            <button type="submit" disabled={busy} className="cs-btn cs-btn--primary" style={{ width: '100%', justifyContent: 'center' }}>
              {t('login.submit')}
            </button>
          </form>
        )}
      </div>
    </div>
  )
}

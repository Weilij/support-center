// Personal profile screen (CRD §8.2): view profile, self-service display
// name edit (strict allowlist), change password with current-password proof.

import { useEffect, useState } from 'react'

import { get, put, post } from '../api/client'
import { PageHeader } from '../components/PageHeader'
import { Card } from '../components/Card'

interface Profile {
  id?: string
  email?: string
  displayName?: string
  role?: string
  teamName?: string
}

export default function ProfilePage() {
  const [profile, setProfile] = useState<Profile>({})
  const [displayName, setDisplayName] = useState('')
  const [currentPassword, setCurrentPassword] = useState('')
  const [newPassword, setNewPassword] = useState('')
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    void get<{ user?: Profile }>('/api/auth/profile').then((resp) => {
      if (resp.success && resp.data?.user) {
        setProfile(resp.data.user)
        setDisplayName(resp.data.user.displayName ?? '')
      }
    })
  }, [])

  const saveName = async (e: React.FormEvent) => {
    e.preventDefault()
    setMessage(null); setError(null)
    const resp = await put('/api/auth/me', { displayName })
    if (resp.success) setMessage(resp.message ?? '已更新')
    else setError(resp.message ?? null)
  }

  const changePassword = async (e: React.FormEvent) => {
    e.preventDefault()
    setMessage(null); setError(null)
    const resp = await post('/api/auth/change-password', { currentPassword, newPassword })
    if (resp.success) {
      setMessage(resp.message ?? '密碼已變更')
      setCurrentPassword(''); setNewPassword('')
    } else {
      setError(resp.message ?? null)
    }
  }

  return (
    <div style={{ maxWidth: 480, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader
        title="個人資料"
        subtitle={profile.email ? `${profile.email}${profile.role ? ` · ${profile.role}` : ''}${profile.teamName ? ` · ${profile.teamName}` : ''}` : undefined}
      />
      {message && <p style={{ color: 'seagreen' }}>{message}</p>}
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}

      <Card title="顯示名稱" style={{ marginBottom: 'var(--sp-4)' }}>
        <form onSubmit={saveName}>
          <label style={{ display: 'block', marginBottom: 8 }}>
            顯示名稱
            <input value={displayName} onChange={(e) => setDisplayName(e.target.value)}
                   maxLength={50} style={{ width: '100%' }} />
          </label>
          <button type="submit">更新名稱</button>
        </form>
      </Card>

      <Card title="變更密碼">
        <form onSubmit={changePassword}>
          <label style={{ display: 'block', marginBottom: 8 }}>
            目前密碼
            <input type="password" value={currentPassword}
                   onChange={(e) => setCurrentPassword(e.target.value)} required style={{ width: '100%' }} />
          </label>
          <label style={{ display: 'block', marginBottom: 8 }}>
            新密碼
            <input type="password" value={newPassword}
                   onChange={(e) => setNewPassword(e.target.value)} required style={{ width: '100%' }} />
          </label>
          <button type="submit">變更密碼</button>
        </form>
      </Card>
    </div>
  )
}

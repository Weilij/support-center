// Team management screen (CRD §8.2, admin-flagged): team list with member
// counts, create team, member listing per team.

import { useEffect, useState } from 'react'

import { get, post } from '../api/client'
import { session } from '../auth/session'

interface Team {
  id: number
  name: string
  description?: string
  memberCount?: number
  isActive?: boolean
}

interface Member {
  id: string
  displayName?: string
  email?: string
  role?: string
  teamRole?: string
}

export default function Teams() {
  const [teams, setTeams] = useState<Team[]>([])
  const [selected, setSelected] = useState<number | null>(null)
  const [members, setMembers] = useState<Member[]>([])
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)

  const load = async () => {
    const resp = await get<{ items?: Team[]; teams?: Team[] } | Team[]>('/api/teams')
    if (resp.success && resp.data) {
      const data = resp.data as { items?: Team[]; teams?: Team[] } | Team[]
      setTeams(Array.isArray(data) ? data : data.items ?? data.teams ?? [])
    } else {
      setError(resp.message ?? null)
    }
  }
  useEffect(() => {
    void load()
  }, [])

  const openTeam = async (id: number) => {
    setSelected(id)
    const resp = await get<{ members?: Member[]; items?: Member[] }>(`/api/teams/${id}/members`)
    if (resp.success && resp.data) {
      setMembers(resp.data.members ?? resp.data.items ?? [])
    }
  }

  const create = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!name.trim()) return
    const resp = await post('/api/teams', { name: name.trim() })
    if (resp.success) {
      setName('')
      void load()
    } else {
      setError(resp.message ?? null)
    }
  }

  // Admin gate AFTER all hooks (Rules of Hooks: stable hook order).
  if (!session.isAdmin()) {
    return <main style={{ margin: '10vh auto', maxWidth: 480 }}><p>權限不足</p></main>
  }
  return (
    <main style={{ maxWidth: 720, margin: '5vh auto' }}>
      <h1>團隊管理</h1>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      <form onSubmit={create} style={{ display: 'flex', gap: 8 }}>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="新團隊名稱" />
        <button type="submit">建立</button>
      </form>
      <div style={{ display: 'flex', gap: 24, marginTop: 16 }}>
        <ul style={{ listStyle: 'none', padding: 0, flex: 1 }}>
          {teams.map((team) => (
            <li
              key={team.id}
              onClick={() => void openTeam(team.id)}
              style={{
                padding: 8, cursor: 'pointer',
                background: selected === team.id ? '#eef5ff' : undefined,
              }}
            >
              <strong>{team.name}</strong>
              {team.memberCount !== undefined && <span>（{team.memberCount} 人）</span>}
            </li>
          ))}
        </ul>
        <div style={{ flex: 1 }}>
          {selected !== null && (
            <>
              <h2>成員</h2>
              <ul style={{ listStyle: 'none', padding: 0 }}>
                {members.map((m) => (
                  <li key={m.id} style={{ padding: 4 }}>
                    {m.displayName ?? m.email}
                    <small style={{ color: '#666' }}> {m.teamRole ?? m.role}</small>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      </div>
    </main>
  )
}

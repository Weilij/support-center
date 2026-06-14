// Team management (admin): team list + create, a per-team QR-code panel
// (generate / show latest join QR), and member management with inline role and
// active-status changes plus bulk removal (Phase 2.2).

import { useEffect, useState } from 'react'

import { get, post, put } from '../api/client'
import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { DataTable } from '../components/DataTable'
import { Modal, ConfirmDialog } from '../components/Modal'
import { StatusPill, Toast } from '../components/ui'
import { PageHeader } from '../components/PageHeader'
import { Card } from '../components/Card'
import type { Column } from '../components/DataTable'

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
  isActive?: boolean
}

interface LatestQr {
  qrCodeImage?: string
  joinUrl?: string
}

const ROLE_OPTIONS = [
  { value: 'agent', label: '客服' },
  { value: 'admin', label: '管理員' },
]

function qrSrc(image?: string): string | undefined {
  if (!image) return undefined
  if (image.startsWith('data:') || image.startsWith('http')) return image
  return `data:image/png;base64,${image}`
}

export default function Teams() {
  const [teams, setTeams] = useState<Team[]>([])
  const [selected, setSelected] = useState<number | null>(null)
  const [members, setMembers] = useState<Member[]>([])
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const [picked, setPicked] = useState<Set<string>>(new Set())
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [qr, setQr] = useState<LatestQr | null>(null)
  const [qrOpen, setQrOpen] = useState(false)

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
    setPicked(new Set())
    const resp = await get<{ members?: Member[]; items?: Member[] }>(`/api/teams/${id}/members`)
    if (resp.success && resp.data) setMembers(resp.data.members ?? resp.data.items ?? [])
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

  const changeRole = async (memberId: string, role: string) => {
    const resp = await put(`/api/teams/members/${memberId}/role`, { role })
    setToast(resp.success ? '角色已更新' : resp.message ?? '更新失敗')
    if (resp.success) setMembers((ms) => ms.map((m) => (m.id === memberId ? { ...m, role } : m)))
  }

  const toggleActive = async (m: Member) => {
    const resp = await put(`/api/teams/members/${m.id}/status`, { isActive: !m.isActive })
    setToast(resp.success ? '狀態已更新' : resp.message ?? '更新失敗')
    if (resp.success) setMembers((ms) => ms.map((x) => (x.id === m.id ? { ...x, isActive: !m.isActive } : x)))
  }

  const bulkDelete = async () => {
    const memberIds = [...picked]
    const resp = await post('/api/teams/members/bulk-delete', { memberIds })
    setConfirmDelete(false)
    setToast(resp.success ? `已移除 ${memberIds.length} 位成員` : resp.message ?? '刪除失敗')
    if (resp.success && selected != null) {
      setPicked(new Set())
      void openTeam(selected)
    }
  }

  const showQr = async (teamId: number) => {
    const resp = await get<LatestQr>(`/api/teams/${teamId}/qr-code/latest`)
    setQr(resp.success && resp.data ? resp.data : {})
    setQrOpen(true)
  }

  const regenerateQr = async (teamId: number) => {
    const resp = await post(`/api/teams/${teamId}/qr-code`, {})
    if (resp.success) {
      setToast('已重新產生 QR code')
      await showQr(teamId)
    } else {
      setToast(resp.message ?? '產生失敗')
    }
  }

  const togglePick = (id: string) =>
    setPicked((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  if (!can(session.position(), 'ops')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  const memberColumns: Column<Member>[] = [
    {
      key: 'sel',
      header: '',
      width: 28,
      render: (m) => <input type="checkbox" checked={picked.has(m.id)} onChange={() => togglePick(m.id)} />,
    },
    { key: 'displayName', header: '名稱', render: (m) => m.displayName || m.email || m.id },
    {
      key: 'role',
      header: '角色',
      width: 120,
      render: (m) => (
        <select
          value={m.role ?? 'agent'}
          onChange={(e) => void changeRole(m.id, e.target.value)}
          style={{ padding: '3px 6px', borderRadius: 6, border: '1px solid #ccc' }}
        >
          {ROLE_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      ),
    },
    {
      key: 'isActive',
      header: '狀態',
      width: 110,
      render: (m) => (
        <button onClick={() => void toggleActive(m)}>
          <StatusPill status={m.isActive ? 'active' : 'inactive'} label={m.isActive ? '啟用' : '停用'} />
        </button>
      ),
    },
  ]

  return (
    <div style={{ maxWidth: 920, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="團隊管理" />

      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}

      <Card style={{ marginBottom: 'var(--sp-4)' }}>
        <form onSubmit={create} style={{ display: 'flex', gap: 8 }}>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="新團隊名稱" />
          <button type="submit">建立</button>
        </form>
      </Card>

      <div style={{ display: 'flex', gap: 24, alignItems: 'flex-start' }}>
        <Card style={{ flex: '0 0 240px' }}>
          <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
            {teams.map((team) => (
              <li
                key={team.id}
                style={{
                  padding: 8,
                  borderRadius: 6,
                  cursor: 'pointer',
                  background: selected === team.id ? 'var(--hairline)' : undefined,
                  display: 'flex',
                  alignItems: 'center',
                  gap: 6,
                }}
              >
                <span style={{ flex: 1 }} onClick={() => void openTeam(team.id)}>
                  <strong>{team.name}</strong>
                  {team.memberCount !== undefined && <span>（{team.memberCount}）</span>}
                </span>
                <button onClick={() => void showQr(team.id)} title="加入 QR code">
                  QR
                </button>
              </li>
            ))}
          </ul>
        </Card>

        <div style={{ flex: 1 }}>
          {selected !== null && (
            <Card>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 'var(--sp-3)' }}>
                <h3 style={{ margin: 0 }}>成員</h3>
                {picked.size > 0 && (
                  <button onClick={() => setConfirmDelete(true)} style={{ color: 'crimson', marginLeft: 'auto' }}>
                    移除所選（{picked.size}）
                  </button>
                )}
              </div>
              <DataTable columns={memberColumns} rows={members} rowKey={(m) => m.id} empty="此團隊沒有成員" />
            </Card>
          )}
        </div>
      </div>

      <Modal open={qrOpen} title="團隊加入 QR code" onClose={() => setQrOpen(false)} width={360}>
        {qr?.qrCodeImage ? (
          <img src={qrSrc(qr.qrCodeImage)} alt="QR code" style={{ width: '100%', maxWidth: 280, display: 'block', margin: '0 auto' }} />
        ) : (
          <p style={{ color: '#888' }}>尚無 QR code，請重新產生。</p>
        )}
        {qr?.joinUrl && (
          <p style={{ fontSize: 13, wordBreak: 'break-all' }}>
            <a href={qr.joinUrl} target="_blank" rel="noreferrer">
              {qr.joinUrl}
            </a>
          </p>
        )}
        <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 12 }}>
          <button onClick={() => selected != null && void regenerateQr(selected)}>重新產生</button>
        </div>
      </Modal>

      <ConfirmDialog
        open={confirmDelete}
        message={`確定要移除所選的 ${picked.size} 位成員嗎？`}
        confirmLabel="移除"
        danger
        onConfirm={() => void bulkDelete()}
        onCancel={() => setConfirmDelete(false)}
      />

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </div>
  )
}

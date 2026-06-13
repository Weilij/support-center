// Assign / transfer / unassign dialog for a single conversation (Phase 1.2).
// Shared by the conversation detail header and reusable anywhere a routing
// action is needed. Teams come from the shared teamsStore.

import { useEffect, useState } from 'react'

import { Modal } from './Modal'
import { Select, Textarea } from './Form'
import { useStore } from '../stores/store'
import { teamsStore, loadTeams } from '../stores/teams'
import {
  assignConversation,
  transferConversation,
  unassignConversation,
} from '../stores/conversations'

export type AssignMode = 'assign' | 'transfer' | 'unassign'

const TITLES: Record<AssignMode, string> = {
  assign: '指派對話',
  transfer: '轉接對話',
  unassign: '取消指派',
}

export function AssignDialog({
  open,
  mode,
  conversationId,
  currentTeamId,
  onClose,
  onDone,
}: {
  open: boolean
  mode: AssignMode
  conversationId: string
  currentTeamId?: number | null
  onClose: () => void
  onDone?: (ok: boolean) => void
}) {
  const { items: teams } = useStore(teamsStore)
  const [teamId, setTeamId] = useState('')
  const [reason, setReason] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (open) {
      void loadTeams()
      setTeamId('')
      setReason('')
      setError(null)
    }
  }, [open])

  const submit = async () => {
    if (mode !== 'unassign' && !teamId) {
      setError('請選擇團隊')
      return
    }
    setBusy(true)
    let ok = false
    if (mode === 'assign') ok = await assignConversation(conversationId, Number(teamId), reason || undefined)
    else if (mode === 'transfer')
      ok = await transferConversation(conversationId, Number(teamId), currentTeamId, reason || undefined)
    else ok = await unassignConversation(conversationId, reason || undefined)
    setBusy(false)
    if (ok) {
      onDone?.(true)
      onClose()
    } else {
      setError('操作失敗，請重試')
    }
  }

  const teamOptions = teams
    .filter((t) => mode !== 'transfer' || t.id !== currentTeamId)
    .map((t) => ({ value: t.id, label: t.name }))

  return (
    <Modal open={open} title={TITLES[mode]} onClose={onClose} width={420}>
      {mode !== 'unassign' && (
        <Select
          label={mode === 'transfer' ? '轉接至團隊' : '指派團隊'}
          options={teamOptions}
          placeholder="選擇團隊"
          value={teamId}
          onChange={(e) => setTeamId(e.target.value)}
        />
      )}
      <Textarea
        label="原因（選填，提供後會寫入路由紀錄）"
        value={reason}
        onChange={(e) => setReason(e.target.value)}
        placeholder="例如：客戶為 VIP，轉至專責團隊"
      />
      {error && <p role="alert" style={{ color: 'crimson', fontSize: 13 }}>{error}</p>}
      <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', marginTop: 8 }}>
        <button onClick={onClose} disabled={busy}>
          取消
        </button>
        <button onClick={() => void submit()} disabled={busy}>
          {busy ? '處理中…' : '確認'}
        </button>
      </div>
    </Modal>
  )
}

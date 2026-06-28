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
  mode?: AssignMode
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

  // When no explicit mode is provided, the dialog self-routes: it assigns the
  // conversation if it has no team yet, otherwise transfers it. Callers passing
  // an explicit mode keep the legacy single-action behavior.
  const unified = mode === undefined

  useEffect(() => {
    if (open) {
      void loadTeams()
      setTeamId('')
      setReason('')
      setError(null)
    }
  }, [open])

  const submit = async () => {
    const effectiveUnassign = !unified && mode === 'unassign'
    if (!effectiveUnassign && !teamId) {
      setError('請選擇團隊')
      return
    }
    setBusy(true)
    let ok = false
    if (effectiveUnassign) {
      ok = await unassignConversation(conversationId, reason || undefined)
    } else if (unified) {
      ok = currentTeamId == null
        ? await assignConversation(conversationId, Number(teamId), reason || undefined)
        : await transferConversation(conversationId, Number(teamId), currentTeamId, reason || undefined)
    } else if (mode === 'assign') {
      ok = await assignConversation(conversationId, Number(teamId), reason || undefined)
    } else {
      ok = await transferConversation(conversationId, Number(teamId), currentTeamId, reason || undefined)
    }
    setBusy(false)
    if (ok) {
      onDone?.(true)
      onClose()
    } else {
      setError('操作失敗，請重試')
    }
  }

  const doUnassign = async () => {
    setBusy(true)
    const ok = await unassignConversation(conversationId, reason || undefined)
    setBusy(false)
    if (ok) {
      onDone?.(true)
      onClose()
    } else {
      setError('操作失敗，請重試')
    }
  }

  const excludeCurrent = unified ? currentTeamId != null : mode === 'transfer'
  const teamOptions = teams
    .filter((t) => !excludeCurrent || t.id !== currentTeamId)
    .map((t) => ({ value: t.id, label: t.name }))

  const showSelect = unified || mode !== 'unassign'
  const selectLabel = unified
    ? '指派團隊'
    : mode === 'transfer'
      ? '轉接至團隊'
      : '指派團隊'
  const title = unified ? '指派團隊' : TITLES[mode]
  const currentTeamName =
    currentTeamId != null ? teams.find((t) => t.id === currentTeamId)?.name : undefined

  return (
    <Modal open={open} title={title} onClose={onClose} width={420}>
      {unified && (
        <p style={{ fontSize: 13, color: 'var(--muted)', margin: '0 0 8px' }}>
          {currentTeamId != null
            ? `目前團隊：${currentTeamName ?? currentTeamId}`
            : '目前：未指派'}
        </p>
      )}
      {showSelect && (
        <Select
          label={selectLabel}
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
        {unified && currentTeamId != null && (
          <button onClick={() => void doUnassign()} disabled={busy} style={{ marginRight: 'auto', color: 'crimson' }}>
            取消指派
          </button>
        )}
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

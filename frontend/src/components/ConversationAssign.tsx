// Inline team-assign menu (Phase 2.1). Replaces the multi-step modal: the
// thread-header button opens a dropdown listing teams (current one marked, plus
// an unassign action). Picking a team submits immediately with toast feedback.
// The routing reason is an optional secondary input that never blocks the pick.

import { useEffect, useRef, useState } from 'react'

import { Icon } from './Icon'
import { useStore } from '../stores/store'
import { teamsStore, loadTeams } from '../stores/teams'
import {
  assignConversation,
  transferConversation,
  unassignConversation,
} from '../stores/conversations'

export function AssignMenu({
  conversationId,
  currentTeamId,
  onResult,
  onChanged,
}: {
  conversationId: string
  currentTeamId?: number | null
  onResult?: (message: string) => void
  /// Called after a successful assign/transfer/unassign so callers can refresh
  /// derived UI (e.g. the customer panel's team label) without a page reload.
  onChanged?: () => void
}) {
  const { items: teams } = useStore(teamsStore)
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [showReason, setShowReason] = useState(false)
  const [reason, setReason] = useState('')
  const wrap = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (open) void loadTeams()
  }, [open])

  // Close on outside click / Escape.
  useEffect(() => {
    if (!open) return
    const onDown = (event: MouseEvent) => {
      if (wrap.current && !wrap.current.contains(event.target as Node)) setOpen(false)
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const close = () => {
    setOpen(false)
    setShowReason(false)
    setReason('')
  }

  const pick = async (teamId: number, teamName: string) => {
    if (busy) return
    if (teamId === currentTeamId) { close(); return } // already there
    setBusy(true)
    const trimmed = reason.trim() || undefined
    const ok = currentTeamId == null
      ? await assignConversation(conversationId, teamId, trimmed)
      : await transferConversation(conversationId, teamId, currentTeamId, trimmed)
    setBusy(false)
    onResult?.(ok ? `已指派給「${teamName}」` : '指派失敗，請重試')
    if (ok) { close(); onChanged?.() }
  }

  const unassign = async () => {
    if (busy) return
    setBusy(true)
    const ok = await unassignConversation(conversationId, reason.trim() || undefined)
    setBusy(false)
    onResult?.(ok ? '已取消指派' : '操作失敗，請重試')
    if (ok) { close(); onChanged?.() }
  }

  return (
    <div ref={wrap} style={{ position: 'relative' }}>
      <button
        className="cs-icon-btn"
        aria-label="指派團隊"
        title="指派團隊"
        aria-haspopup="menu"
        aria-expanded={open}
        style={{ width: 38, height: 38 }}
        onClick={() => setOpen((value) => !value)}
      >
        <Icon name="users" w={19} />
      </button>

      {open && (
        <div className="cs-assign-menu" role="menu">
          <div className="cs-assign-menu-label">指派給團隊</div>
          {teams.length === 0 && (
            <div style={{ padding: '8px 12px', fontSize: 13, color: 'var(--muted)' }}>沒有可用團隊</div>
          )}
          {teams.map((team) => {
            const isCurrent = team.id === currentTeamId
            return (
              <button
                key={team.id}
                role="menuitem"
                className="cs-assign-item"
                disabled={busy}
                onClick={() => void pick(team.id, team.name)}
              >
                <span style={{ width: 16, flexShrink: 0, color: 'var(--blue-600)' }}>{isCurrent ? '✓' : ''}</span>
                <span style={{ flex: 1, textAlign: 'left' }}>{team.name}</span>
                {isCurrent && <span style={{ fontSize: 12, color: 'var(--muted)' }}>目前</span>}
              </button>
            )
          })}

          {currentTeamId != null && (
            <button
              role="menuitem"
              className="cs-assign-item cs-assign-item--danger"
              disabled={busy}
              onClick={() => void unassign()}
            >
              <span style={{ width: 16, flexShrink: 0 }} />
              <span style={{ flex: 1, textAlign: 'left' }}>取消指派</span>
            </button>
          )}

          <div className="cs-assign-menu-foot">
            {showReason ? (
              <textarea
                aria-label="指派原因"
                value={reason}
                onChange={(event) => setReason(event.target.value)}
                placeholder="原因（選填，會寫入路由紀錄）"
                rows={2}
                style={{ width: '100%', boxSizing: 'border-box', resize: 'none', fontSize: 12 }}
              />
            ) : (
              <button
                type="button"
                className="cs-assign-reason-toggle"
                onClick={() => setShowReason(true)}
              >
                填寫原因…
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  )
}

import { Drawer } from '../../components/Modal'
import { cancelDelayed, type PendingDelayed } from '../../stores/delayedMessages'
import type { ConvMeta } from './types'

export function ScheduleDrawer({
  open,
  convId,
  meta,
  pending,
  draft,
  delayMin,
  message,
  onClose,
  onDraftChange,
  onDelayMinChange,
  onSubmit,
  onRefresh,
}: {
  open: boolean
  convId: string | undefined
  meta: ConvMeta
  pending: PendingDelayed[]
  draft: string
  delayMin: number
  message: string | null
  onClose: () => void
  onDraftChange: (value: string) => void
  onDelayMinChange: (value: number) => void
  onSubmit: () => Promise<void>
  onRefresh: () => Promise<void>
}) {
  return (
    <Drawer
      open={open}
      title="排程訊息"
      onClose={onClose}
      width={420}
    >
      {convId && (
        <>
          {(!meta.platform || !meta.platformUserId) ? (
            <p style={{ color: 'var(--muted)', fontSize: 13, margin: '0 0 12px' }}>
              缺少客戶平台資訊，無法排程
            </p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginBottom: 12 }}>
              <textarea
                value={draft}
                onChange={(event) => onDraftChange(event.target.value)}
                placeholder="排程訊息內容"
                rows={3}
                style={{
                  width: '100%',
                  padding: '6px 8px',
                  fontSize: 14,
                  border: '1px solid var(--line)',
                  borderRadius: 8,
                  resize: 'vertical',
                  fontFamily: 'inherit',
                  boxSizing: 'border-box',
                }}
              />
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <label style={{ fontSize: 13, color: 'var(--muted)', display: 'flex', alignItems: 'center', gap: 4 }}>
                  延遲
                  <input
                    type="number"
                    min={1}
                    value={delayMin}
                    onChange={(event) => onDelayMinChange(Number(event.target.value))}
                    style={{ width: 60, padding: '4px 6px', border: '1px solid var(--line)', borderRadius: 6, textAlign: 'center' }}
                  />
                  分鐘
                </label>
                <button
                  className="cs-btn cs-btn--primary"
                  disabled={!draft.trim()}
                  style={{ marginLeft: 'auto' }}
                  onClick={() => void onSubmit()}
                >
                  排程送出
                </button>
              </div>
              {message && (
                <p style={{ fontSize: 13, color: 'var(--muted)', margin: 0 }}>{message}</p>
              )}
            </div>
          )}
          <hr style={{ border: 'none', borderTop: '1px solid var(--line)', margin: '0 0 12px' }} />
          {pending.length === 0 ? (
            <p style={{ color: 'var(--muted)', fontSize: 13, margin: 0 }}>無待送訊息</p>
          ) : (
            <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
              {pending.map((item) => (
                <li
                  key={item.messageId}
                  style={{
                    display: 'flex',
                    gap: 8,
                    alignItems: 'center',
                    padding: '6px 0',
                    fontSize: 14,
                    borderBottom: '1px solid var(--line)',
                  }}
                >
                  <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {item.preview || '(無內容)'}
                  </span>
                  <span style={{ color: 'var(--muted)', fontSize: 12, flexShrink: 0 }}>
                    {item.remainingMs != null ? `${Math.ceil(item.remainingMs / 1000)}s` : ''}
                  </span>
                  <button
                    className="cs-btn"
                    style={{ flexShrink: 0, fontSize: 12, padding: '3px 10px' }}
                    onClick={async () => {
                      if (await cancelDelayed(item.messageId)) await onRefresh()
                    }}
                  >
                    取消
                  </button>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </Drawer>
  )
}

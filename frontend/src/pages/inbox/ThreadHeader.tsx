import { Avatar } from '../../components/Avatar'
import { ChanGlyph } from '../../components/ChanGlyph'
import { Icon } from '../../components/Icon'
import { CHANNELS, channelOf } from '../../components/channels'
import type { ConvMeta } from './types'

export function ThreadHeader({
  convId,
  meta,
  filesCount,
  pendingCount,
  onBack,
  onToggleFiles,
  onToggleSchedule,
  onAssign,
  onTransfer,
  onToggleCustomerPanel,
  showCustomerPanelToggle,
}: {
  convId: string
  meta: ConvMeta
  filesCount: number
  pendingCount: number
  onBack?: () => void
  onToggleFiles: () => void
  onToggleSchedule: () => void
  onAssign: () => void
  onTransfer: () => void
  onToggleCustomerPanel?: () => void
  showCustomerPanelToggle?: boolean
}) {
  const chanKey = channelOf(meta.platform ?? 'chat')
  const chanDef = CHANNELS[chanKey]
  const customerName = meta.customerName ?? ''
  const customerAvatarUrl = meta.avatarUrl ?? undefined

  return (
    <div className="cs-thread-head">
      {onBack && (
        <button
          className="cs-icon-btn"
          aria-label="返回列表"
          title="返回列表"
          onClick={onBack}
          style={{ width: 38, height: 38, marginRight: 4, transform: 'scaleX(-1)' }}
        >
          <Icon name="arrowRight" w={19} />
        </button>
      )}
      <div style={{ position: 'relative', flexShrink: 0 }}>
        <Avatar name={customerName || '?'} src={customerAvatarUrl} size="md" />
        <span style={{ position: 'absolute', bottom: -2, right: -4 }}>
          <ChanGlyph type={chanKey as 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'} size={17} />
        </span>
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span style={{ fontSize: 15.5, fontWeight: 700, color: 'var(--ink)' }}>
            {customerName || convId}
          </span>
        </div>
        <div style={{ fontSize: 12, color: 'var(--muted)', marginTop: 1 }}>
          透過 {chanDef?.name ?? chanKey}
        </div>
      </div>
      <div style={{ display: 'flex', gap: 6 }}>
        <CounterButton
          ariaLabel="檔案"
          title="檔案"
          icon="paperclip"
          count={filesCount}
          onClick={onToggleFiles}
        />
        <CounterButton
          ariaLabel="排程"
          title="排程"
          icon="clock"
          count={pendingCount}
          onClick={onToggleSchedule}
        />
        <button
          className="cs-icon-btn"
          aria-label="指派"
          title="指派"
          style={{ width: 38, height: 38 }}
          onClick={onAssign}
        >
          <Icon name="plus" w={19} />
        </button>
        <button
          className="cs-icon-btn"
          aria-label="轉接"
          title="轉接"
          style={{ width: 38, height: 38 }}
          onClick={onTransfer}
        >
          <Icon name="users" w={19} />
        </button>
        {showCustomerPanelToggle && (
          <button
            className="cs-icon-btn"
            aria-label="客戶資訊"
            title="客戶資訊"
            style={{ width: 38, height: 38 }}
            onClick={onToggleCustomerPanel}
          >
            <Icon name="arrowRight" w={19} />
          </button>
        )}
      </div>
    </div>
  )
}

function CounterButton({
  ariaLabel,
  title,
  icon,
  count,
  onClick,
}: {
  ariaLabel: string
  title: string
  icon: 'paperclip' | 'clock'
  count: number
  onClick: () => void
}) {
  return (
    <button
      className="cs-icon-btn"
      aria-label={ariaLabel}
      title={title}
      style={{ width: 38, height: 38, position: 'relative' }}
      onClick={onClick}
    >
      <Icon name={icon} w={19} />
      {count > 0 && (
        <span style={{
          position: 'absolute',
          top: 4,
          right: 4,
          background: 'var(--brand, var(--blue-600))',
          color: '#fff',
          fontSize: 10,
          fontWeight: 700,
          borderRadius: 8,
          minWidth: 14,
          height: 14,
          lineHeight: '14px',
          textAlign: 'center',
          padding: '0 3px',
        }}>
          {count}
        </span>
      )}
    </button>
  )
}

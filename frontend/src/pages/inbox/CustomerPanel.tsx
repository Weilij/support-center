import { useEffect, useState, type CSSProperties, type ReactNode } from 'react'

import { Avatar } from '../../components/Avatar'
import { ChanGlyph } from '../../components/ChanGlyph'
import { Tag } from '../../components/Chip'
import { Icon } from '../../components/Icon'
import { channelOf, CHANNELS } from '../../components/channels'
import { useCollapsed } from '../../hooks/useCollapsed'
import {
  loadCustomerDetail,
  loadCustomerTags,
  type Customer,
  type CustomerTag,
} from '../../stores/customers'
import type { ConvMeta } from './types'

function CollapsibleSection({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  const [collapsed, toggle] = useCollapsed(id, false)
  return (
    <div>
      <button
        className="cs-cust-block-label"
        onClick={toggle}
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          width: '100%',
          background: 'none',
          border: 'none',
          padding: 0,
          cursor: 'pointer',
        }}
      >
        {title}
        <Icon name={collapsed ? 'down' : 'up'} w={14} />
      </button>
      {!collapsed && <div>{children}</div>}
    </div>
  )
}

const overlayPanelStyle: CSSProperties = {
  position: 'absolute',
  top: 0,
  right: 0,
  bottom: 0,
  zIndex: 100,
  boxShadow: 'var(--shadow-lg)',
  borderLeft: '1px solid var(--line)',
}

export function CustomerPanel({
  meta,
  overlay,
  onClose,
}: {
  meta: ConvMeta
  overlay?: boolean
  onClose?: () => void
}) {
  const [customer, setCustomer] = useState<Customer | null>(null)
  const [tags, setTags] = useState<CustomerTag[]>([])
  const [convCount, setConvCount] = useState<number | null>(null)
  const customerId = meta.customerId

  useEffect(() => {
    if (!customerId) {
      setCustomer(null)
      setTags([])
      setConvCount(null)
      return
    }
    void loadCustomerDetail(customerId).then((detail) => {
      if (!detail) return
      setCustomer(detail.customer)
      setConvCount(detail.conversationCount)
    })
    void loadCustomerTags(customerId).then(setTags)
  }, [customerId])

  if (!meta.platform && !meta.customerName) {
    return (
      <div
        className="cs-cust"
        style={{
          alignItems: 'center',
          justifyContent: 'center',
          ...(overlay ? overlayPanelStyle : {}),
        }}
      >
        {overlay && onClose && (
          <button
            className="cs-icon-btn"
            onClick={onClose}
            style={{ position: 'absolute', top: 12, right: 12, width: 32, height: 32 }}
            title="關閉"
          >
            <Icon name="plus" w={16} style={{ transform: 'rotate(45deg)' }} />
          </button>
        )}
        <span style={{ color: 'var(--muted)', fontSize: 13 }}>選擇對話以查看客戶資訊</span>
      </div>
    )
  }

  const name = customer?.display_name ?? meta.customerName ?? ''
  const email = customer?.email
  const phone = customer?.phone
  const platform = customer?.platform ?? meta.platform ?? ''
  const platformUserId = customer?.platform_user_id ?? meta.platformUserId ?? ''
  const chanKey = channelOf(platform)
  const chanDef = CHANNELS[chanKey]

  return (
    <div
      className="cs-cust"
      style={{
        overflowY: 'auto',
        ...(overlay ? overlayPanelStyle : {}),
      }}
    >
      {overlay && onClose && (
        <button
          className="cs-icon-btn"
          onClick={onClose}
          style={{ position: 'absolute', top: 12, right: 12, width: 32, height: 32 }}
          title="關閉"
        >
          <Icon name="plus" w={16} style={{ transform: 'rotate(45deg)' }} />
        </button>
      )}

      <div style={{ textAlign: 'center', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 8 }}>
        <Avatar name={name || '?'} src={customer?.avatar_url ?? meta.avatarUrl ?? undefined} size="lg" ring />
        <div style={{ fontSize: 16, fontWeight: 700, color: 'var(--ink)' }}>{name}</div>
        {(customer?.id || platformUserId) && (
          <div style={{ fontSize: 12.5, color: 'var(--muted)' }}>
            {customer?.id ? `會員編號 #C-${customer.id}` : platformUserId}
          </div>
        )}
        {tags.length > 0 && (
          <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap', justifyContent: 'center' }}>
            {tags.map((tag) => (
              <Tag key={tag.id} label={tag.name} />
            ))}
          </div>
        )}
      </div>

      <hr style={{ border: 'none', borderTop: '1px solid var(--line)', margin: 0 }} />

      <CollapsibleSection id="inbox.cust.contact" title="聯絡資訊">
        {email && (
          <div className="cs-kv">
            <span className="cs-kv-k">電子郵件</span>
            <span className="cs-kv-v cs-mono" style={{ fontSize: 12 }}>{email}</span>
          </div>
        )}
        {phone && (
          <div className="cs-kv">
            <span className="cs-kv-k">電話</span>
            <span className="cs-kv-v cs-mono">{phone}</span>
          </div>
        )}
        {chanDef && (
          <div className="cs-kv">
            <span className="cs-kv-k">偏好渠道</span>
            <span className="cs-kv-v" style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <ChanGlyph type={chanKey as 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'} size={14} />
              {chanDef.name}
            </span>
          </div>
        )}
        {!email && !phone && !chanDef && (
          <p style={{ fontSize: 13, color: 'var(--muted)' }}>無聯絡資料</p>
        )}
      </CollapsibleSection>

      {convCount !== null && (
        <>
          <hr style={{ border: 'none', borderTop: '1px solid var(--line)', margin: 0 }} />
          <CollapsibleSection id="inbox.cust.stats" title="統計">
            <div style={{ display: 'flex', gap: 0 }}>
              <div style={{ flex: 1, textAlign: 'center', borderRight: '1px solid var(--line)', paddingRight: 8 }}>
                <div style={{ fontSize: 20, fontWeight: 700, color: 'var(--ink)' }}>{convCount}</div>
                <div style={{ fontSize: 11.5, color: 'var(--muted)', marginTop: 2 }}>歷史對話</div>
              </div>
            </div>
          </CollapsibleSection>
        </>
      )}
    </div>
  )
}

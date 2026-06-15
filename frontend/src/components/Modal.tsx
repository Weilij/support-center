// Overlay primitives (Epic 0 foundation): a centred Modal for forms, a
// right-side Drawer for detail panels, and a ConfirmDialog built on Modal for
// destructive/bulk actions. All dismiss on backdrop click and Escape.

import { useEffect } from 'react'
import type { ReactNode } from 'react'

const backdrop: React.CSSProperties = {
  position: 'fixed',
  inset: 0,
  background: 'rgba(15,23,42,0.4)',
  display: 'flex',
  zIndex: 1000,
}

function useEscape(onClose: () => void) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
}

export interface ModalProps {
  open: boolean
  title?: ReactNode
  onClose: () => void
  children: ReactNode
  width?: number
}

export function Modal({ open, title, onClose, children, width = 480 }: ModalProps) {
  useEscape(onClose)
  if (!open) return null
  return (
    <div style={{ ...backdrop, alignItems: 'center', justifyContent: 'center' }} onClick={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        onClick={(e) => e.stopPropagation()}
        style={{
          background: 'var(--surface)',
          border: '1px solid var(--line)',
          borderRadius: 'var(--radius-lg)',
          padding: 20,
          width,
          maxWidth: '92vw',
          maxHeight: '88vh',
          overflowY: 'auto',
          boxShadow: 'var(--shadow-lg)',
        }}
      >
        {title && <h2 style={{ marginTop: 0, fontSize: 18 }}>{title}</h2>}
        {children}
      </div>
    </div>
  )
}

export interface DrawerProps {
  open: boolean
  title?: ReactNode
  onClose: () => void
  children: ReactNode
  width?: number
}

export function Drawer({ open, title, onClose, children, width = 420 }: DrawerProps) {
  useEscape(onClose)
  if (!open) return null
  return (
    <div style={{ ...backdrop, justifyContent: 'flex-end' }} onClick={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        onClick={(e) => e.stopPropagation()}
        style={{
          background: 'var(--surface)',
          border: '1px solid var(--line)',
          width,
          maxWidth: '92vw',
          height: '100%',
          overflowY: 'auto',
          padding: 20,
          boxShadow: 'var(--shadow-lg)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center' }}>
          {title && <h2 style={{ margin: 0, fontSize: 18 }}>{title}</h2>}
          <button onClick={onClose} style={{ marginLeft: 'auto' }} aria-label="關閉">
            ✕
          </button>
        </div>
        <div style={{ marginTop: 12 }}>{children}</div>
      </div>
    </div>
  )
}

export interface ConfirmDialogProps {
  open: boolean
  title?: ReactNode
  message: ReactNode
  confirmLabel?: string
  danger?: boolean
  onConfirm: () => void
  onCancel: () => void
}

export function ConfirmDialog({
  open,
  title = '請確認',
  message,
  confirmLabel = '確認',
  danger,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  return (
    <Modal open={open} title={title} onClose={onCancel} width={400}>
      <p style={{ margin: '4px 0 20px' }}>{message}</p>
      <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
        <button onClick={onCancel}>取消</button>
        <button
          onClick={onConfirm}
          style={danger ? { background: 'crimson', color: 'white', border: 'none', padding: '6px 14px', borderRadius: 6 } : undefined}
        >
          {confirmLabel}
        </button>
      </div>
    </Modal>
  )
}

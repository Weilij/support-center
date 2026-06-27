// Inbox — 3-column workspace (N5): conversation list + thread + customer panel.
// Replaces the separate Conversations and ConversationDetail pages.
// Columns: .cs-conv-list (340px) | .cs-thread (flex 1) | .cs-cust (300px)
// The .cs-inbox flex container lives inside .cs-content which already has
// overflow:hidden and flex:1 — so height:100% fills the available space.

import { useCallback, useEffect, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'

import {
  conversationsStore,
  loadConversations,
  markConversationRead,
} from '../stores/conversations'
import { useStore } from '../stores/store'
import { useCollapsed } from '../hooks/useCollapsed'
import { useHotkeys } from '../hooks/useHotkeys'
import { ConversationList } from './inbox/ConversationList'
import { CustomerPanel } from './inbox/CustomerPanel'
import { Thread } from './inbox/Thread'
import type { ConvMeta } from './inbox/types'

// ── Main Inbox page ───────────────────────────────────────────────────────────

export default function Inbox() {
  const { id: paramId } = useParams<{ id?: string }>()
  const navigate = useNavigate()
  const { items, busy } = useStore(conversationsStore)
  const [selectedId, setSelectedId] = useState<string | undefined>(paramId)
  const [meta, setMeta] = useState<ConvMeta>({})

  // ── RWD breakpoints ─────────────────────────────────────────────────────────
  const [isWide, setIsWide] = useState(() => window.matchMedia('(min-width: 1101px)').matches)
  const [isMedium, setIsMedium] = useState(() => window.matchMedia('(max-width: 1100px) and (min-width: 769px)').matches)
  const [isNarrow, setIsNarrow] = useState(() => window.matchMedia('(max-width: 768px)').matches)
  const [custPanelOpen, setCustPanelOpen] = useState(false)
  const [custCollapsed, toggleCustCollapsed] = useCollapsed('inbox.custPanel', false)

  useHotkeys({ 'escape': () => setCustPanelOpen(false) })

  useEffect(() => {
    const wide   = window.matchMedia('(min-width: 1101px)')
    const medium = window.matchMedia('(max-width: 1100px) and (min-width: 769px)')
    const narrow = window.matchMedia('(max-width: 768px)')

    const onWide   = (e: MediaQueryListEvent) => { setIsWide(e.matches);   if (e.matches) { setIsMedium(false); setIsNarrow(false) } }
    const onMedium = (e: MediaQueryListEvent) => { setIsMedium(e.matches); if (e.matches) { setIsWide(false); setIsNarrow(false) } }
    const onNarrow = (e: MediaQueryListEvent) => { setIsNarrow(e.matches); if (e.matches) { setIsWide(false); setIsMedium(false) } }

    wide.addEventListener('change', onWide)
    medium.addEventListener('change', onMedium)
    narrow.addEventListener('change', onNarrow)
    return () => {
      wide.removeEventListener('change', onWide)
      medium.removeEventListener('change', onMedium)
      narrow.removeEventListener('change', onNarrow)
    }
  }, [])

  // Load conversation list on mount
  useEffect(() => {
    void loadConversations()
  }, [])

  // Keep selectedId in sync with route param changes
  useEffect(() => {
    if (paramId && paramId !== selectedId) setSelectedId(paramId)
  }, [paramId])

  const handleSelect = useCallback((id: string) => {
    setSelectedId(id)
    setMeta({}) // clear stale meta while new one loads
    void markConversationRead(id)
    navigate(`/conversations/${id}`, { replace: true })
  }, [navigate])

  const handleMetaLoaded = useCallback((m: ConvMeta) => {
    setMeta(m)
  }, [])

  const handleBack = useCallback(() => {
    setSelectedId(undefined)
    navigate('/conversations', { replace: true })
  }, [navigate])

  // ── Layout helpers ──────────────────────────────────────────────────────────

  // Narrow: show list or thread, never both
  if (isNarrow) {
    const showList = !selectedId
    return (
      <div
        className="cs-inbox"
        style={{ margin: '-28px -32px', height: 'calc(100% + 56px)', position: 'relative' }}
      >
        {showList ? (
          <ConversationList
            items={items}
            busy={busy}
            selectedId={selectedId}
            onSelect={handleSelect}
            fullWidth
          />
        ) : (
          <Thread
            convId={selectedId}
            meta={meta}
            onMetaLoaded={handleMetaLoaded}
            onBack={handleBack}
            onToggleCustPanel={() => setCustPanelOpen((v) => !v)}
            showCustToggle
          />
        )}
        {/* Customer panel as overlay drawer */}
        {custPanelOpen && selectedId && (
          <>
            {/* Dim backdrop */}
            <div
              onClick={() => setCustPanelOpen(false)}
              style={{
                position: 'absolute',
                inset: 0,
                background: 'rgba(15,23,42,.32)',
                zIndex: 99,
              }}
            />
            <CustomerPanel
              meta={meta}
              overlay
              onClose={() => setCustPanelOpen(false)}
            />
          </>
        )}
      </div>
    )
  }

  // Medium (≤ 1100px): conv list + thread side by side; customer panel as overlay
  if (isMedium) {
    return (
      <div
        className="cs-inbox"
        style={{ margin: '-28px -32px', height: 'calc(100% + 56px)', position: 'relative' }}
      >
        <ConversationList
          items={items}
          busy={busy}
          selectedId={selectedId}
          onSelect={handleSelect}
        />
        <Thread
          convId={selectedId}
          meta={meta}
          onMetaLoaded={handleMetaLoaded}
          onToggleCustPanel={() => setCustPanelOpen((v) => !v)}
          showCustToggle
        />
        {/* Customer panel overlay drawer */}
        {custPanelOpen && (
          <>
            <div
              onClick={() => setCustPanelOpen(false)}
              style={{
                position: 'absolute',
                inset: 0,
                background: 'rgba(15,23,42,.32)',
                zIndex: 99,
              }}
            />
            <CustomerPanel
              meta={meta}
              overlay
              onClose={() => setCustPanelOpen(false)}
            />
          </>
        )}
      </div>
    )
  }

  // Wide (> 1100px, including default): 3-column layout
  // isWide may be true OR both isMedium and isNarrow are false (SSR/init safety)
  void isWide // suppress unused-var lint in some setups
  return (
    <div
      className="cs-inbox"
      style={{
        margin: '-28px -32px',
        height: 'calc(100% + 56px)',
      }}
    >
      <ConversationList
        items={items}
        busy={busy}
        selectedId={selectedId}
        onSelect={handleSelect}
      />
      <Thread
        convId={selectedId}
        meta={meta}
        onMetaLoaded={handleMetaLoaded}
        onToggleCustPanel={toggleCustCollapsed}
        showCustToggle
      />
      {!custCollapsed && <CustomerPanel meta={meta} />}
    </div>
  )
}

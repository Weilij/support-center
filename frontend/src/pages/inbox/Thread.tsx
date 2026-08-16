import { useCallback, useEffect, useRef, useState, type DragEvent, type FormEvent } from 'react'

import { get, post } from '../../api/client'
import { session } from '../../auth/session'
import { Toast } from '../../components/ui'
import { onEvent, readMessageEvent, subscribeConversation, unsubscribeConversation } from '../../realtime/client'
import { useConnectionState } from '../../hooks/useConnectionState'
import { loadPendingDelayed, scheduleDelayed, type PendingDelayed } from '../../stores/delayedMessages'
import {
  loadConversationFiles,
  uploadConversationFile,
  type Attachment,
} from '../../stores/files'
import { assignConversation, conversationsStore } from '../../stores/conversations'
import { FilesDrawer } from './FilesDrawer'
import { MessageList } from './MessageList'
import { MessageComposer } from './MessageComposer'
import { ScheduleDrawer } from './ScheduleDrawer'
import { ThreadHeader } from './ThreadHeader'
import type { ConvMeta, InboxMessage, PendingAttachment } from './types'

/// Normalize a `message_updated` payload into message fields. The event names
/// the failure text `error` (CRD 828) while the REST message view calls it
/// `rejectMessage`; both land on `rejectMessage` so the UI reads one shape.
function deliveryPatch(update: Record<string, unknown>): Partial<InboxMessage> {
  const patch: Partial<InboxMessage> = {}
  if (typeof update.deliveryStatus === 'string') patch.deliveryStatus = update.deliveryStatus
  if (typeof update.isSent === 'boolean') patch.isSent = update.isSent
  if (typeof update.rejectCode === 'string') patch.rejectCode = update.rejectCode
  if (typeof update.error === 'string') patch.rejectMessage = update.error
  if (typeof update.readAt === 'string') patch.readAt = update.readAt
  return patch
}

/// Upper bound on parked `message_updated` patches (see the buffering site).
const BUFFERED_UPDATE_CAP = 200

/// Apply the `message_updated` patches parked for messages that were not in the
/// list yet. EVERY path that puts messages into the list applies them, not just
/// `send`: the history load (initial and on reconnect) races the delivery
/// outcome, and a stale history response would otherwise overwrite an
/// already-resolved message back to 'pending'.
///
/// Pure — it does not consume what it applies. Entries are dropped after the
/// commit that used them (see the cleanup effect), because a state updater may
/// run twice under StrictMode and the second pass would find the buffer already
/// emptied and rebuild the list without the patch.
function applyBuffered(
  buffer: Map<string, Partial<InboxMessage>>,
  list: InboxMessage[],
): InboxMessage[] {
  if (buffer.size === 0) return list
  let changed = false
  const next = list.map((message) => {
    const patch = buffer.get(message.id)
    if (!patch) return message
    changed = true
    return { ...message, ...patch }
  })
  return changed ? next : list
}

/// Park a patch, evicting the oldest entries past the cap. A read receipt can
/// name a message that scrolled out of the loaded history and will never enter
/// the list, so the buffer cannot be left to grow for the lifetime of the
/// thread. Map iteration is insertion-ordered.
function parkUpdate(
  buffer: Map<string, Partial<InboxMessage>>,
  messageId: string,
  patch: Partial<InboxMessage>,
) {
  buffer.set(messageId, patch)
  while (buffer.size > BUFFERED_UPDATE_CAP) {
    const oldest = buffer.keys().next().value
    if (oldest === undefined) break
    buffer.delete(oldest)
  }
}

export function Thread({
  convId,
  meta,
  onMetaLoaded,
  onBack,
  onToggleCustPanel,
  showCustToggle,
}: {
  convId: string | undefined
  meta: ConvMeta
  onMetaLoaded: (m: ConvMeta) => void
  onBack?: () => void
  onToggleCustPanel?: () => void
  showCustToggle?: boolean
}) {
  const [messages, setMessages] = useState<InboxMessage[]>([])
  const [draft, setDraft] = useState('')
  // Per-conversation draft cache (in-memory): switching away parks the current
  // draft under its conversation id and restores the target's, so an unfinished
  // reply survives a detour to another thread. Attachments are not preserved.
  const draftRef = useRef(draft)
  draftRef.current = draft
  const drafts = useRef<Map<string, string>>(new Map())
  const prevConvId = useRef(convId)
  const [error, setError] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const [reloadKey, setReloadKey] = useState(0)
  const bottom = useRef<HTMLDivElement>(null)

  const [showFiles, setShowFiles] = useState(false)
  const [files, setFiles] = useState<Attachment[]>([])

  const refreshFiles = useCallback(async () => {
    if (!convId) return
    setFiles(await loadConversationFiles(convId))
  }, [convId])

  useEffect(() => {
    if (showFiles) void refreshFiles()
  }, [showFiles, convId, refreshFiles])

  const [showSchedule, setShowSchedule] = useState(false)
  const [pending, setPending] = useState<PendingDelayed[]>([])
  const [schedDraft, setSchedDraft] = useState('')
  const [delayMin, setDelayMin] = useState(5)
  const [schedMsg, setSchedMsg] = useState<string | null>(null)

  const refreshPending = useCallback(async () => {
    if (!convId) return
    setPending(await loadPendingDelayed(convId))
  }, [convId])

  useEffect(() => {
    if (showSchedule) void refreshPending()
  }, [showSchedule, convId, refreshPending])

  const submitSchedule = async () => {
    if (!convId || !schedDraft.trim()) return
    if (!meta.platform || !meta.platformUserId) {
      setSchedMsg('缺少客戶平台資訊，無法排程')
      return
    }
    const res = await scheduleDelayed({
      conversationId: convId,
      content: schedDraft.trim(),
      platform: meta.platform,
      userId: meta.platformUserId,
      delaySeconds: Math.max(1, Math.round(delayMin * 60)),
    })
    setSchedMsg(res.ok ? '已排程' : res.message ?? '排程失敗')
    if (res.ok) {
      setSchedDraft('')
      await refreshPending()
    }
  }

  const offline = useConnectionState() === 'reconnecting'

  const [dragOver, setDragOver] = useState(false)
  const fileInput = useRef<HTMLInputElement>(null)
  const objectUrls = useRef<string[]>([])
  const [attachPending, setAttachPending] = useState<PendingAttachment[]>([])

  const addFiles = useCallback(async (files: FileList | File[]) => {
    if (!convId) return
    for (const file of Array.from(files)) {
      const { attachment, error } = await uploadConversationFile(convId, file)
      if (error || !attachment) { setToast(`上傳失敗：${error ?? file.name}`); continue }
      const previewUrl = file.type.startsWith('image/') ? URL.createObjectURL(file) : undefined
      if (previewUrl) objectUrls.current.push(previewUrl)
      setAttachPending((pendingAttachments) => [...pendingAttachments, {
        id: attachment.id,
        name: attachment.filename ?? file.name,
        mime: attachment.contentType ?? file.type,
        previewUrl,
      }])
    }
  }, [convId])

  useEffect(() => () => {
    objectUrls.current.forEach((url) => URL.revokeObjectURL(url))
    objectUrls.current = []
  }, [])

  const handleDragOver = (event: DragEvent) => {
    event.preventDefault()
    setDragOver(true)
  }
  const handleDragLeave = () => setDragOver(false)
  const handleDrop = async (event: DragEvent) => {
    event.preventDefault()
    setDragOver(false)
    const dropped = event.dataTransfer.files
    if (dropped && dropped.length) await addFiles(dropped)
  }

  // Park/restore drafts on conversation switch (runs before the message-load
  // effect below; both key off convId).
  useEffect(() => {
    const prev = prevConvId.current
    if (prev && prev !== convId) drafts.current.set(prev, draftRef.current)
    setDraft(convId ? (drafts.current.get(convId) ?? '') : '')
    prevConvId.current = convId
  }, [convId])

  // `message_updated` patches keyed by real message id, applied by every path
  // that inserts messages (`send`, the history load, and the `new_message`
  // insert). Parking happens eagerly in the event handler rather than inside a
  // state updater: a history load resolving in between computes its apply
  // eagerly too, so a park deferred until React flushed the updater would be
  // missed and the patch stranded — the exact race this buffer exists to close.
  const bufferedUpdates = useRef(new Map<string, Partial<InboxMessage>>())

  // Fetch conversation metadata (platform, team, customer) and push it up. Used
  // on conversation switch AND after an assign/transfer so the customer panel's
  // team label stays in sync without a page reload.
  const refreshMeta = useCallback(async () => {
    if (!convId) return
    const resp = await get<{
      platform?: string
      platformUserId?: string
      teamId?: number | null
      assignedTeam?: { id?: number; name?: string } | null
      customerId?: number | null
      customerName?: string
      customerAvatarUrl?: string
    }>(`/api/conversations/${convId}`)
    if (resp.success && resp.data) {
      onMetaLoaded({
        platform: resp.data.platform,
        platformUserId: resp.data.platformUserId,
        teamId: resp.data.teamId ?? null,
        teamName: resp.data.assignedTeam?.name ?? null,
        customerId: resp.data.customerId ?? null,
        customerName: resp.data.customerName,
        avatarUrl: resp.data.customerAvatarUrl ?? null,
      })
    }
  }, [convId, onMetaLoaded])

  useEffect(() => {
    setAttachPending((prev) => {
      prev.forEach((attachment) => { if (attachment.previewUrl) URL.revokeObjectURL(attachment.previewUrl) })
      return []
    })
    if (!convId) { setMessages([]); return }
    void refreshMeta()
    const loadMessages = async () => {
      setError(null)
      const resp = await get<{ items?: InboxMessage[]; messages?: InboxMessage[] }>(
        `/api/conversations/${convId}/messages`,
      )
      if (resp.success && resp.data) {
        const items = (resp.data.items ?? resp.data.messages ?? []) as Array<
          InboxMessage & { metadata?: { media?: Record<string, unknown> } }
        >
        const mapped = items.map((message) => ({ ...message, media: message.media ?? message.metadata?.media }))
        setMessages(applyBuffered(bufferedUpdates.current, [...mapped].reverse()))
      } else {
        setError(resp.message ?? null)
      }
    }
    void loadMessages()
    subscribeConversation(convId)
    const off = onEvent('new_message', (payload) => {
      const message = readMessageEvent(payload)
      if (message.conversationId !== convId || message.isOwn) return
      // A teammate's message enters the list here, so its delivery outcome can
      // also have landed first. Read outside the updater; the cleanup effect
      // drops the entry once the commit that used it lands.
      const buffered = bufferedUpdates.current.get(message.id)
      setMessages((prev) =>
        prev.some((item) => item.id === message.id)
          ? prev
          : [...prev, {
              id: message.id || crypto.randomUUID(),
              content: message.content,
              senderType: message.senderType,
              createdAt: message.timestamp,
              messageType: message.messageType,
              media: message.media,
              ...buffered,
            }],
      )
    })
    const offReconnect = onEvent('realtime_reconnected', () => {
      void loadMessages()
    })
    const offMessageUpdated = onEvent('message_updated', (payload) => {
      const update = (payload.data ?? payload) as Record<string, unknown>
      if (String(update.conversationId ?? payload.conversationId ?? '') !== convId) return
      const messageId = String(update.messageId ?? '')
      if (!messageId) return
      // Park unconditionally and synchronously, then let the updater apply
      // whatever is parked. Delivery can resolve before the POST response swaps
      // the optimistic temp id for the real one (a credential-less send fails
      // with no network round-trip at all), and a history load can resolve
      // before React flushes this update — both are covered because the entry
      // exists the moment the event is handled.
      parkUpdate(bufferedUpdates.current, messageId, deliveryPatch(update))
      setMessages((prev) => applyBuffered(bufferedUpdates.current, prev))
    })
    return () => {
      off()
      offReconnect()
      offMessageUpdated()
      bufferedUpdates.current.clear()
      unsubscribeConversation(convId)
    }
  }, [convId, reloadKey, refreshMeta])

  // Drop parked patches once a commit carrying their message has landed. Every
  // path that writes `messages` applies the buffer first, so a message present
  // here has already taken its patch; doing the delete after the commit instead
  // of inside the updater keeps the updaters pure for StrictMode.
  useEffect(() => {
    if (bufferedUpdates.current.size === 0) return
    for (const message of messages) bufferedUpdates.current.delete(message.id)
  }, [messages])

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages.length])

  const send = async (event: FormEvent) => {
    event.preventDefault()
    if (!convId || (!draft.trim() && attachPending.length === 0)) return
    const text = draft.trim()
    const attachments = attachPending
    setDraft('')
    setAttachPending([])
    const tempId = `pending-${Date.now()}`
    const who = session.identity()
    setMessages((prev) => [...prev, {
      id: tempId,
      content: text,
      senderType: 'agent',
      senderName: who?.displayName,
      pending: true,
      attachments: attachments.map((attachment) => ({
        id: attachment.id,
        filename: attachment.name,
        mimeType: attachment.mime,
        url: attachment.previewUrl,
      })),
    }])
    const resp = await post<{ message?: InboxMessage; id?: string }>(
      `/api/conversations/${convId}/messages`,
      { content: text, senderId: who?.id, attachmentIds: attachments.map((attachment) => attachment.id) },
    )
    if (resp.success) {
      const confirmed = resp.data?.message ?? { id: resp.data?.id ?? tempId, content: text }
      // The send only queues delivery (CRD 773) — the outcome arrives later on
      // `message_updated`, so 'pending' is the honest state until it does. Any
      // update that beat this swap was buffered under the real id.
      const buffered = bufferedUpdates.current.get(confirmed.id)
      setMessages((prev) => prev.map((message) => (
        message.id === tempId
          ? { ...message, ...confirmed, pending: false, deliveryStatus: 'pending', ...buffered }
          : message
      )))
    } else {
      setMessages((prev) => prev.filter((message) => message.id !== tempId))
      setError(resp.message ?? null)
      setDraft(text)
      setAttachPending(attachments)
    }
  }

  const quickAssignToMyTeam = async () => {
    if (!convId) return
    const myTeam = session.currentTeam() ?? session.teamOptions().find((t) => t.isPrimary) ?? session.teamOptions()[0]
    const teamNum = myTeam ? Number(myTeam.id) : NaN
    if (!Number.isFinite(teamNum)) { setToast('你沒有所屬團隊，無法指派'); return }
    const ok = await assignConversation(convId, teamNum)
    setToast(ok ? `已指派給「${myTeam!.name}」` : '指派失敗')
  }

  const currentTeamId =
    meta.teamId ??
    ((conversationsStore.get().items.find((conversation) => conversation.id === convId)?.teamId ?? null) as number | null)

  if (!convId) {
    return (
      <div className="cs-thread" style={{ alignItems: 'center', justifyContent: 'center' }}>
        <div style={{ textAlign: 'center', color: 'var(--muted)', fontSize: 15 }}>
          選擇一則對話開始
        </div>
      </div>
    )
  }

  const customerName = meta.customerName ?? ''
  const customerAvatarUrl = meta.avatarUrl ?? undefined

  return (
    <div className="cs-thread">
      <ThreadHeader
        convId={convId}
        meta={meta}
        filesCount={files.length}
        pendingCount={pending.length}
        currentTeamId={currentTeamId}
        onBack={onBack}
        onToggleFiles={() => setShowFiles((value) => !value)}
        onToggleSchedule={() => setShowSchedule((value) => !value)}
        onAssignResult={setToast}
        onAssignChanged={() => void refreshMeta()}
        onToggleCustomerPanel={onToggleCustPanel}
        showCustomerPanelToggle={showCustToggle}
      />

      <MessageList
        convId={convId}
        messages={messages}
        error={error}
        onRetry={() => setReloadKey((key) => key + 1)}
        customerName={customerName}
        customerAvatarUrl={customerAvatarUrl}
        bottomRef={bottom}
      />

      <MessageComposer
        draft={draft}
        setDraft={setDraft}
        attachments={attachPending}
        setAttachments={setAttachPending}
        dragOver={dragOver}
        fileInput={fileInput}
        onAddFiles={addFiles}
        onAssign={() => void quickAssignToMyTeam()}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
        onSubmit={send}
        offline={offline}
      />

      <FilesDrawer
        open={showFiles}
        convId={convId}
        files={files}
        onClose={() => setShowFiles(false)}
        onRefresh={refreshFiles}
      />

      <ScheduleDrawer
        open={showSchedule}
        convId={convId}
        meta={meta}
        pending={pending}
        draft={schedDraft}
        delayMin={delayMin}
        message={schedMsg}
        onClose={() => setShowSchedule(false)}
        onDraftChange={setSchedDraft}
        onDelayMinChange={setDelayMin}
        onSubmit={submitSchedule}
        onRefresh={refreshPending}
      />

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </div>
  )
}

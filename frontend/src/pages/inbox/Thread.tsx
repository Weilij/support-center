import { useCallback, useEffect, useRef, useState, type DragEvent, type FormEvent } from 'react'

import { get, post } from '../../api/client'
import { session } from '../../auth/session'
import { AssignDialog } from '../../components/ConversationAssign'
import { Toast } from '../../components/ui'
import { onEvent, readMessageEvent, subscribeConversation, unsubscribeConversation } from '../../realtime/client'
import { loadPendingDelayed, scheduleDelayed, type PendingDelayed } from '../../stores/delayedMessages'
import {
  loadConversationFiles,
  uploadConversationFile,
  type Attachment,
} from '../../stores/files'
import { conversationsStore } from '../../stores/conversations'
import { FilesDrawer } from './FilesDrawer'
import { MessageList } from './MessageList'
import { MessageComposer } from './MessageComposer'
import { ScheduleDrawer } from './ScheduleDrawer'
import { ThreadHeader } from './ThreadHeader'
import type { ConvMeta, InboxMessage, PendingAttachment } from './types'

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
  const [error, setError] = useState<string | null>(null)
  const [assignOpen, setAssignOpen] = useState(false)
  const [toast, setToast] = useState<string | null>(null)
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

  useEffect(() => {
    setAttachPending((prev) => {
      prev.forEach((attachment) => { if (attachment.previewUrl) URL.revokeObjectURL(attachment.previewUrl) })
      return []
    })
    if (!convId) { setMessages([]); return }
    void get<{
      platform?: string
      platformUserId?: string
      teamId?: number | null
      customerId?: number | null
      customerName?: string
      customerAvatarUrl?: string
    }>(`/api/conversations/${convId}`).then((resp) => {
      if (resp.success && resp.data) {
        onMetaLoaded({
          platform: resp.data.platform,
          platformUserId: resp.data.platformUserId,
          teamId: resp.data.teamId ?? null,
          customerId: resp.data.customerId ?? null,
          customerName: resp.data.customerName,
          avatarUrl: resp.data.customerAvatarUrl ?? null,
        })
      }
    })
    const loadMessages = async () => {
      const resp = await get<{ items?: InboxMessage[]; messages?: InboxMessage[] }>(
        `/api/conversations/${convId}/messages`,
      )
      if (resp.success && resp.data) {
        const items = (resp.data.items ?? resp.data.messages ?? []) as Array<
          InboxMessage & { metadata?: { media?: Record<string, unknown> } }
        >
        const mapped = items.map((message) => ({ ...message, media: message.media ?? message.metadata?.media }))
        setMessages([...mapped].reverse())
      } else {
        setError(resp.message ?? null)
      }
    }
    void loadMessages()
    subscribeConversation(convId)
    const off = onEvent('new_message', (payload) => {
      const message = readMessageEvent(payload)
      if (message.conversationId !== convId || message.isOwn) return
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
            }],
      )
    })
    const offReconnect = onEvent('realtime_reconnected', () => {
      void loadMessages()
    })
    return () => {
      off()
      offReconnect()
      unsubscribeConversation(convId)
    }
  }, [convId]) // onMetaLoaded intentionally omitted — stable callback ref

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
      setMessages((prev) => prev.map((message) => (message.id === tempId ? { ...message, ...confirmed, pending: false } : message)))
    } else {
      setMessages((prev) => prev.filter((message) => message.id !== tempId))
      setError(resp.message ?? null)
      setDraft(text)
      setAttachPending(attachments)
    }
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
        onBack={onBack}
        onToggleFiles={() => setShowFiles((value) => !value)}
        onToggleSchedule={() => setShowSchedule((value) => !value)}
        onAssign={() => setAssignOpen(true)}
        onToggleCustomerPanel={onToggleCustPanel}
        showCustomerPanelToggle={showCustToggle}
      />

      <MessageList
        convId={convId}
        messages={messages}
        error={error}
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
        onAssign={() => setAssignOpen(true)}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
        onSubmit={send}
      />

      {convId && assignOpen && (
        <AssignDialog
          open
          conversationId={convId}
          currentTeamId={currentTeamId}
          onClose={() => setAssignOpen(false)}
        />
      )}

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

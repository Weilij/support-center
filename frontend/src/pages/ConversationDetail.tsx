// Single-conversation screen (CRD §8.2): message history, sending with
// optimistic append, realtime updates via the shared channel.

import { useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'

import { get, post } from '../api/client'
import { onEvent, subscribeConversation } from '../realtime/client'
import { session } from '../auth/session'
import { AssignDialog, type AssignMode } from '../components/ConversationAssign'
import { conversationsStore } from '../stores/conversations'
import { FileUpload } from '../components/FileUpload'
import {
  loadConversationFiles,
  uploadConversationFile,
  fileDownloadUrl,
  type Attachment,
} from '../stores/files'
import {
  loadPendingDelayed,
  scheduleDelayed,
  cancelDelayed,
  type PendingDelayed,
} from '../stores/delayedMessages'

interface ConvMeta {
  platform?: string
  platformUserId?: string
  teamId?: number | null
}

interface Message {
  id: string
  content?: string
  senderType?: string
  senderName?: string
  createdAt?: string
  pending?: boolean
}

export default function ConversationDetail() {
  const { id } = useParams<{ id: string }>()
  const [messages, setMessages] = useState<Message[]>([])
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [assignMode, setAssignMode] = useState<AssignMode | null>(null)
  const [files, setFiles] = useState<Attachment[]>([])
  const [showFiles, setShowFiles] = useState(false)
  const [meta, setMeta] = useState<ConvMeta>({})
  const [showSchedule, setShowSchedule] = useState(false)
  const [pending, setPending] = useState<PendingDelayed[]>([])
  const [schedDraft, setSchedDraft] = useState('')
  const [delayMin, setDelayMin] = useState(5)
  const [schedMsg, setSchedMsg] = useState<string | null>(null)
  const bottom = useRef<HTMLDivElement>(null)

  const refreshFiles = async () => {
    if (!id) return
    setFiles(await loadConversationFiles(id))
  }

  // Conversation meta (platform + recipient + team) powers scheduling and the
  // assign dialog's current-team default.
  useEffect(() => {
    if (!id) return
    void get<{ platform?: string; platformUserId?: string; teamId?: number | null }>(
      `/api/conversations/${id}`,
    ).then((resp) => {
      if (resp.success && resp.data) {
        setMeta({
          platform: resp.data.platform,
          platformUserId: resp.data.platformUserId,
          teamId: resp.data.teamId ?? null,
        })
      }
    })
  }, [id])

  const refreshPending = async () => {
    if (!id) return
    setPending(await loadPendingDelayed(id))
  }
  useEffect(() => {
    if (showSchedule) void refreshPending()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showSchedule, id])

  const submitSchedule = async () => {
    if (!id || !schedDraft.trim()) return
    if (!meta.platform || !meta.platformUserId) {
      setSchedMsg('缺少客戶平台資訊，無法排程')
      return
    }
    const res = await scheduleDelayed({
      conversationId: id,
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
  useEffect(() => {
    if (showFiles) void refreshFiles()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showFiles, id])

  useEffect(() => {
    if (!id) return
    void get<{ items?: Message[]; messages?: Message[] }>(
      `/api/conversations/${id}/messages`,
    ).then((resp) => {
      if (resp.success && resp.data) {
        const items = resp.data.items ?? resp.data.messages ?? []
        setMessages([...items].reverse())
      } else {
        setError(resp.message ?? null)
      }
    })
    subscribeConversation(id)
    // Realtime reconciliation: append pushed messages for this conversation.
    return onEvent('new_message', (payload) => {
      if (String(payload.conversationId) !== id) return
      const m = (payload.message ?? {}) as Record<string, unknown>
      setMessages((prev) =>
        prev.some((x) => x.id === m.id)
          ? prev
          : [...prev, {
              id: String(m.id ?? crypto.randomUUID()),
              content: String(m.content ?? ''),
              senderType: String(m.senderType ?? 'customer'),
              createdAt: String(m.timestamp ?? ''),
            }],
      )
    })
  }, [id])

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages.length])

  const send = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!id || !draft.trim()) return
    const text = draft.trim()
    setDraft('')
    // Optimistic append; replaced or reverted after the server answers.
    const tempId = `pending-${Date.now()}`
    const who = session.identity()
    setMessages((prev) => [...prev, {
      id: tempId, content: text, senderType: 'agent',
      senderName: who?.displayName, pending: true,
    }])
    const resp = await post<{ message?: Message; id?: string }>(
      `/api/conversations/${id}/messages`,
      { content: text },
    )
    if (resp.success) {
      const confirmed = resp.data?.message ?? { id: resp.data?.id ?? tempId, content: text }
      setMessages((prev) => prev.map((m) => (m.id === tempId ? { ...m, ...confirmed, pending: false } : m)))
    } else {
      setMessages((prev) => prev.filter((m) => m.id !== tempId)) // rollback
      setError(resp.message ?? null)
      setDraft(text)
    }
  }

  const currentTeamId =
    meta.teamId ??
    ((conversationsStore.get().items.find((c) => c.id === id)?.teamId ?? null) as number | null)

  return (
    <main style={{ maxWidth: 720, margin: '3vh auto', fontFamily: 'sans-serif' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
        <h1 style={{ margin: 0 }}>對話 {id}</h1>
        <div style={{ marginLeft: 'auto', display: 'flex', gap: 8 }}>
          <button onClick={() => setShowFiles((v) => !v)}>
            檔案{files.length > 0 ? ` (${files.length})` : ''}
          </button>
          <button onClick={() => setShowSchedule((v) => !v)}>
            排程{pending.length > 0 ? ` (${pending.length})` : ''}
          </button>
          <button onClick={() => setAssignMode('assign')}>指派</button>
          <button onClick={() => setAssignMode('transfer')}>轉接</button>
          <button onClick={() => setAssignMode('unassign')}>取消指派</button>
        </div>
      </div>

      {showFiles && id && (
        <div style={{ border: '1px solid #eee', borderRadius: 8, padding: 12, margin: '10px 0' }}>
          <FileUpload
            label="拖放或點選上傳檔案到此對話"
            onUpload={async (file) => {
              const { error } = await uploadConversationFile(id, file)
              if (!error) await refreshFiles()
              return error ?? null
            }}
          />
          {files.length === 0 ? (
            <p style={{ color: '#aaa', fontSize: 13, marginBottom: 0 }}>尚無檔案</p>
          ) : (
            <ul style={{ listStyle: 'none', padding: 0, margin: '10px 0 0' }}>
              {files.map((f) => (
                <li
                  key={f.id}
                  style={{
                    display: 'flex',
                    gap: 8,
                    alignItems: 'center',
                    padding: '4px 0',
                    fontSize: 14,
                  }}
                >
                  <span>{f.originalName || f.filename || f.id}</span>
                  {f.size != null && (
                    <span style={{ color: '#aaa', fontSize: 12 }}>
                      {Math.round(f.size / 1024)} KB
                    </span>
                  )}
                  <button
                    style={{ marginLeft: 'auto' }}
                    onClick={async () => {
                      const url = (await fileDownloadUrl(f.id)) ?? f.publicUrl ?? f.url
                      if (url) window.open(url, '_blank')
                    }}
                  >
                    下載
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      {showSchedule && id && (
        <div style={{ border: '1px solid #eee', borderRadius: 8, padding: 12, margin: '10px 0' }}>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
            <input
              value={schedDraft}
              onChange={(e) => setSchedDraft(e.target.value)}
              placeholder="排程訊息內容"
              style={{ flex: 1, minWidth: 200, padding: '6px 8px' }}
            />
            <label style={{ fontSize: 13, color: '#555' }}>
              延遲
              <input
                type="number"
                min={1}
                value={delayMin}
                onChange={(e) => setDelayMin(Number(e.target.value))}
                style={{ width: 60, margin: '0 4px', padding: '4px 6px' }}
              />
              分鐘
            </label>
            <button onClick={() => void submitSchedule()}>排程送出</button>
          </div>
          {schedMsg && <p style={{ fontSize: 13, color: '#666', margin: '6px 0 0' }}>{schedMsg}</p>}
          {pending.length === 0 ? (
            <p style={{ color: '#aaa', fontSize: 13, marginBottom: 0 }}>無待送訊息</p>
          ) : (
            <ul style={{ listStyle: 'none', padding: 0, margin: '10px 0 0' }}>
              {pending.map((p) => (
                <li
                  key={p.messageId}
                  style={{ display: 'flex', gap: 8, alignItems: 'center', padding: '4px 0', fontSize: 14 }}
                >
                  <span style={{ flex: 1 }}>{p.preview || '(無內容)'}</span>
                  <span style={{ color: '#aaa', fontSize: 12 }}>
                    {p.remainingMs != null ? `${Math.ceil(p.remainingMs / 1000)}s` : ''}
                  </span>
                  <button
                    onClick={async () => {
                      if (await cancelDelayed(p.messageId)) await refreshPending()
                    }}
                  >
                    取消
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      {id && assignMode && (
        <AssignDialog
          open
          mode={assignMode}
          conversationId={id}
          currentTeamId={currentTeamId}
          onClose={() => setAssignMode(null)}
        />
      )}
      <div style={{ height: '60vh', overflowY: 'auto', border: '1px solid #eee', padding: 8 }}>
        {messages.map((m) => (
          <div key={m.id} style={{
            textAlign: m.senderType === 'customer' ? 'left' : 'right',
            opacity: m.pending ? 0.5 : 1, margin: '4px 0',
          }}>
            <span style={{
              display: 'inline-block', padding: '6px 10px', borderRadius: 12,
              background: m.senderType === 'customer' ? '#f0f0f0' : '#d2e9ff',
            }}>
              {m.content}
            </span>
          </div>
        ))}
        <div ref={bottom} />
      </div>
      <form onSubmit={send} style={{ display: 'flex', gap: 8, marginTop: 8 }}>
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="輸入訊息…"
          style={{ flex: 1 }}
        />
        <button type="submit">送出</button>
      </form>
    </main>
  )
}

import {
  useState,
  type Dispatch,
  type DragEvent,
  type FormEvent,
  type KeyboardEvent,
  type RefObject,
  type SetStateAction,
} from 'react'

import { Icon } from '../../components/Icon'
import { SlashMenu } from '../../components/SlashMenu'
import { TemplateManager } from '../../components/TemplateManager'
import { useTemplates } from '../../hooks/useTemplates'
import type { PendingAttachment } from './types'

export function MessageComposer({
  draft,
  setDraft,
  attachments,
  setAttachments,
  dragOver,
  fileInput,
  onAddFiles,
  onAssign,
  onDragOver,
  onDragLeave,
  onDrop,
  onSubmit,
}: {
  draft: string
  setDraft: Dispatch<SetStateAction<string>>
  attachments: PendingAttachment[]
  setAttachments: Dispatch<SetStateAction<PendingAttachment[]>>
  dragOver: boolean
  fileInput: RefObject<HTMLInputElement>
  onAddFiles: (files: FileList | File[]) => Promise<void>
  onAssign: () => void
  onDragOver: (event: DragEvent) => void
  onDragLeave: () => void
  onDrop: (event: DragEvent) => Promise<void>
  onSubmit: (event: FormEvent) => Promise<void>
}) {
  const { list: templates } = useTemplates()
  const [slashIndex, setSlashIndex] = useState(0)
  const [mgrOpen, setMgrOpen] = useState(false)
  const slashOpen = draft.startsWith('/')
  const slashQuery = slashOpen ? draft.slice(1).toLowerCase() : ''
  const slashMatches = slashOpen
    ? templates.filter((template) =>
        template.title.toLowerCase().includes(slashQuery) ||
        template.body.toLowerCase().includes(slashQuery),
      )
    : []

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing) return
    if (slashOpen && slashMatches.length > 0) {
      if (event.key === 'ArrowDown') {
        event.preventDefault()
        setSlashIndex((index) => (index + 1) % slashMatches.length)
        return
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault()
        setSlashIndex((index) => (index - 1 + slashMatches.length) % slashMatches.length)
        return
      }
      if (event.key === 'Enter') {
        event.preventDefault()
        setDraft(slashMatches[Math.min(slashIndex, slashMatches.length - 1)].body)
        setSlashIndex(0)
        return
      }
      if (event.key === 'Escape') {
        event.preventDefault()
        setDraft('')
        return
      }
    }
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault()
      void onSubmit(event as unknown as FormEvent)
      return
    }
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault()
      void onSubmit(event as unknown as FormEvent)
    }
  }

  return (
    <div
      className="cs-composer"
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={(event) => void onDrop(event)}
    >
      <form onSubmit={(event) => void onSubmit(event)}>
        <div
          className="cs-composer-box"
          style={dragOver ? {
            border: '2px dashed var(--blue-500)',
            position: 'relative',
          } : undefined}
        >
          {dragOver && (
            <div style={{
              position: 'absolute',
              inset: 0,
              background: 'rgba(14,165,233,.08)',
              borderRadius: 'inherit',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              zIndex: 10,
              fontSize: 14,
              fontWeight: 600,
              color: 'var(--blue-600)',
              pointerEvents: 'none',
            }}>
              放開以附加到訊息
            </div>
          )}
          {slashOpen && slashMatches.length > 0 && (
            <SlashMenu
              templates={slashMatches}
              activeIndex={Math.min(slashIndex, slashMatches.length - 1)}
              onPick={(template) => { setDraft(template.body); setSlashIndex(0) }}
            />
          )}
          {attachments.length > 0 && (
            <div className="cs-attach-row" style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginBottom: 8 }}>
              {attachments.map((attachment) => (
                <div key={attachment.id} className="cs-attach-chip" style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 8px', border: '1px solid var(--border)', borderRadius: 8 }}>
                  {attachment.previewUrl
                    ? <img src={attachment.previewUrl} alt={attachment.name} style={{ width: 36, height: 36, objectFit: 'cover', borderRadius: 4 }} />
                    : <span>📄</span>}
                  <span style={{ maxWidth: 140, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{attachment.name}</span>
                  <button type="button" aria-label="移除" onClick={() => setAttachments((list) => {
                    const found = list.find((item) => item.id === attachment.id)
                    if (found?.previewUrl) URL.revokeObjectURL(found.previewUrl)
                    return list.filter((item) => item.id !== attachment.id)
                  })} style={{ border: 'none', background: 'transparent', cursor: 'pointer' }}>×</button>
                </div>
              ))}
            </div>
          )}
          <textarea
            className="cs-composer-input"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onPaste={(event) => {
              const files = Array.from(event.clipboardData.files)
              if (files.length) { event.preventDefault(); void onAddFiles(files) }
            }}
            onKeyDown={handleKeyDown}
            placeholder="輸入訊息，或按「/」插入罐頭回覆…"
            rows={1}
            style={{
              width: '100%',
              border: 'none',
              outline: 'none',
              resize: 'none',
              background: 'transparent',
              fontFamily: 'inherit',
            }}
          />
          <div className="cs-composer-tools">
            <button type="button" className="cs-composer-ico" aria-label="附件" onClick={() => fileInput.current?.click()}>
              <Icon name="paperclip" w={20} />
            </button>
            <input
              ref={fileInput}
              type="file"
              multiple
              accept="image/*,video/*,audio/*,application/pdf,.doc,.docx,.xls,.xlsx,.zip"
              style={{ display: 'none' }}
              onChange={(event) => { if (event.target.files) void onAddFiles(event.target.files); event.target.value = '' }}
            />
            <button type="button" className="cs-composer-ico" aria-label="表情">
              <Icon name="emoji" w={20} />
            </button>
            <button type="button" className="cs-composer-ico" aria-label="快捷回覆" onClick={() => setMgrOpen(true)}>
              <Icon name="zap" w={20} />
            </button>
            <span style={{ flex: 1 }} />
            <button
              type="button"
              onClick={onAssign}
              className="cs-chip cs-chip--blue"
              style={{ cursor: 'pointer', border: 'none' }}
            >
              指給我的團隊
            </button>
            <button
              type="submit"
              className="cs-btn cs-btn--primary"
              disabled={!draft.trim() && attachments.length === 0}
              style={{ display: 'flex', alignItems: 'center', gap: 6 }}
            >
              <Icon name="send" w={18} />
              傳送
            </button>
          </div>
        </div>
      </form>
      <TemplateManager open={mgrOpen} onClose={() => setMgrOpen(false)} />
    </div>
  )
}

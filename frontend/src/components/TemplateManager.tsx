import { useState } from 'react'
import { useTemplates } from '../hooks/useTemplates'
import { Modal } from './Modal'

export function TemplateManager({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { list, add, update, remove } = useTemplates()
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  return (
    <Modal open={open} title="管理罐頭回覆" onClose={onClose} width={480}>
      <div style={{ display: 'grid', gap: 8, marginBottom: 16 }}>
        {list.map((t) => (
          <div key={t.id} style={{ display: 'flex', gap: 8, alignItems: 'flex-start' }}>
            <input value={t.title} onChange={(e) => update(t.id, { title: e.target.value })} style={{ width: 120, flexShrink: 0 }} />
            <textarea value={t.body} onChange={(e) => update(t.id, { body: e.target.value })} rows={2} style={{ flex: 1 }} />
            <button type="button" onClick={() => remove(t.id)} aria-label="刪除">✕</button>
          </div>
        ))}
      </div>
      <div style={{ display: 'flex', gap: 8, alignItems: 'flex-start', borderTop: '1px solid var(--line)', paddingTop: 12 }}>
        <input placeholder="標題" value={title} onChange={(e) => setTitle(e.target.value)} style={{ width: 120, flexShrink: 0 }} />
        <textarea placeholder="內容" value={body} onChange={(e) => setBody(e.target.value)} rows={2} style={{ flex: 1 }} />
        <button type="button" className="cs-btn cs-btn--primary" disabled={!title.trim() || !body.trim()}
          onClick={() => { add({ title: title.trim(), body: body.trim() }); setTitle(''); setBody('') }}>新增</button>
      </div>
    </Modal>
  )
}

// Frontend-local canned replies, persisted in localStorage under `cannedReplies`.
export interface Template { id: string; title: string; body: string }

const KEY = 'cannedReplies'
const DEFAULTS: Template[] = [
  { id: 'seed-greet', title: '問候', body: '您好，很高興為您服務，請問有什麼能幫您的嗎？' },
  { id: 'seed-wait', title: '請稍候', body: '好的，請您稍候，我馬上為您查詢。' },
  { id: 'seed-thanks', title: '感謝', body: '感謝您的耐心等候！還有其他需要協助的地方嗎？' },
]

function read(): Template[] {
  const raw = localStorage.getItem(KEY)
  if (raw === null) { localStorage.setItem(KEY, JSON.stringify(DEFAULTS)); return [...DEFAULTS] }
  try { const p = JSON.parse(raw); return Array.isArray(p) ? (p as Template[]) : [] } catch { return [] }
}
function write(list: Template[]): void { localStorage.setItem(KEY, JSON.stringify(list)) }

export function listTemplates(): Template[] { return read() }
export function addTemplate(input: { title: string; body: string }): Template {
  const t: Template = { id: `t-${Date.now()}-${Math.floor(Math.random() * 1e6)}`, ...input }
  write([...read(), t]); return t
}
export function updateTemplate(id: string, patch: Partial<Omit<Template, 'id'>>): void {
  write(read().map((t) => (t.id === id ? { ...t, ...patch } : t)))
}
export function removeTemplate(id: string): void { write(read().filter((t) => t.id !== id)) }

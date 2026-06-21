import type { Template } from '../lib/templates'

export function SlashMenu({ templates, activeIndex, onPick }: {
  templates: Template[]; activeIndex: number; onPick: (t: Template) => void
}) {
  if (templates.length === 0) return null
  return (
    <div className="cs-slash-menu" role="listbox" aria-label="罐頭回覆">
      {templates.map((t, i) => (
        <button
          key={t.id} type="button" role="option" aria-selected={i === activeIndex}
          className={`cs-slash-item${i === activeIndex ? ' cs-slash-item--active' : ''}`}
          onMouseDown={(e) => { e.preventDefault(); onPick(t) }}
        >
          <span className="cs-slash-title">{t.title}</span>
          <span className="cs-slash-body">{t.body}</span>
        </button>
      ))}
    </div>
  )
}

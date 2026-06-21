import { beforeEach, describe, expect, it } from 'vitest'

import { addTemplate, listTemplates, removeTemplate, updateTemplate } from '../lib/templates'

describe('templates store', () => {
  beforeEach(() => localStorage.clear())

  it('seeds defaults on first read and persists them', () => {
    const list = listTemplates()
    expect(list.length).toBeGreaterThan(0)
    expect(localStorage.getItem('cannedReplies')).not.toBeNull()
  })

  it('adds, updates, and removes', () => {
    localStorage.setItem('cannedReplies', '[]')
    const t = addTemplate({ title: '問候', body: '您好，有什麼能幫您？' })
    expect(t.id).toBeTruthy()
    expect(listTemplates()).toHaveLength(1)
    updateTemplate(t.id, { body: '您好！' })
    expect(listTemplates()[0].body).toBe('您好！')
    removeTemplate(t.id)
    expect(listTemplates()).toHaveLength(0)
  })
})

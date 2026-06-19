// i18n contract (CRD §8.4): zh-TW default, en switch, fallback chain.

import { beforeEach, describe, expect, it } from 'vitest'

describe('i18n', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('defaults to zh-TW and falls back for unknown keys', async () => {
    const { t } = await import('../i18n')
    expect(t('login.title')).toBe('登入')
    expect(t('does.not.exist')).toBe('does.not.exist')
  })

  it('switches locale and persists the choice', async () => {
    const { t, setLocale } = await import('../i18n')
    setLocale('en')
    expect(t('login.title')).toBe('Sign in')
    expect(localStorage.getItem('mcss.locale')).toBe('en')
    setLocale('zh-TW')
  })

  it('ignores unknown locales', async () => {
    const { t, setLocale } = await import('../i18n')
    setLocale('xx')
    expect(t('login.title')).toBe('登入')
  })

  it('resolves the must-change-password keys in both locales', async () => {
    const { t, setLocale } = await import('../i18n')
    const keys = ['login.mustChangeTitle', 'login.mustChangeHint', 'login.backToLogin']
    setLocale('zh-TW')
    for (const k of keys) expect(t(k)).not.toBe(k)
    setLocale('en')
    for (const k of keys) expect(t(k)).not.toBe(k)
    setLocale('zh-TW')
  })
})

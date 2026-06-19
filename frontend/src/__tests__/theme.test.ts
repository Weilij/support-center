import { beforeEach, describe, expect, it, vi } from 'vitest'

import {
  applyTheme,
  currentTheme,
  initTheme,
  resolveInitialTheme,
  storedTheme,
  toggleTheme,
} from '../theme'

function mockMatchMedia(matches: boolean) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    configurable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches,
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  })
}

describe('theme', () => {
  beforeEach(() => {
    localStorage.clear()
    delete document.documentElement.dataset.theme
    mockMatchMedia(false)
  })

  it('resolves from localStorage when set', () => {
    localStorage.setItem('theme', 'dark')
    expect(storedTheme()).toBe('dark')
    expect(resolveInitialTheme()).toBe('dark')
  })

  it('falls back to OS preference when unset', () => {
    mockMatchMedia(true)
    expect(storedTheme()).toBeNull()
    expect(resolveInitialTheme()).toBe('dark')
  })

  it('applyTheme sets data-theme and persists', () => {
    applyTheme('dark')
    expect(document.documentElement.dataset.theme).toBe('dark')
    expect(localStorage.getItem('theme')).toBe('dark')
  })

  it('toggleTheme flips current and persists', () => {
    applyTheme('light')
    expect(toggleTheme()).toBe('dark')
    expect(currentTheme()).toBe('dark')
    expect(toggleTheme()).toBe('light')
    expect(localStorage.getItem('theme')).toBe('light')
  })

  it('initTheme applies the resolved theme without persisting the OS default', () => {
    mockMatchMedia(true)
    initTheme()
    expect(document.documentElement.dataset.theme).toBe('dark')
    expect(localStorage.getItem('theme')).toBeNull()
  })
})

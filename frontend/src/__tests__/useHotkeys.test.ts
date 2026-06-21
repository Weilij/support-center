import { describe, expect, it } from 'vitest'

import { matchHotkey } from '../hooks/useHotkeys'

function ev(key: string, mods: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }> = {}) {
  return { key, metaKey: false, ctrlKey: false, shiftKey: false, ...mods } as KeyboardEvent
}

describe('matchHotkey', () => {
  it('matches mod+k on meta or ctrl', () => {
    expect(matchHotkey(ev('k', { metaKey: true }), 'mod+k')).toBe(true)
    expect(matchHotkey(ev('k', { ctrlKey: true }), 'mod+k')).toBe(true)
  })
  it('does not match without the modifier', () => {
    expect(matchHotkey(ev('k'), 'mod+k')).toBe(false)
  })
  it('matches mod+enter and is case-insensitive on the key', () => {
    expect(matchHotkey(ev('Enter', { metaKey: true }), 'mod+enter')).toBe(true)
    expect(matchHotkey(ev('K', { ctrlKey: true }), 'mod+k')).toBe(true)
  })
})

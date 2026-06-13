import { describe, expect, it } from 'vitest'

import { positionOf, can, AREA_ACCESS } from '../auth/permissions'

describe('positionOf', () => {
  it('passes through a valid explicit position', () => {
    expect(positionOf({ position: 'supervisor', role: 'agent' })).toBe('supervisor')
  })
  it('falls back to system_admin for admin role when position is null', () => {
    expect(positionOf({ role: 'admin' })).toBe('system_admin')
  })
  it('falls back to agent otherwise', () => {
    expect(positionOf({ role: 'agent' })).toBe('agent')
    expect(positionOf({})).toBe('agent')
  })
  it('ignores an unknown position value and falls back', () => {
    expect(positionOf({ position: 'wizard', role: 'admin' })).toBe('system_admin')
  })
})

describe('can', () => {
  it('agent sees only daily', () => {
    expect(can('agent', 'daily')).toBe(true)
    expect(can('agent', 'ops')).toBe(false)
    expect(can('agent', 'analytics')).toBe(false)
    expect(can('agent', 'system')).toBe(false)
  })
  it('supervisor sees daily, ops, analytics but not system', () => {
    expect(can('supervisor', 'analytics')).toBe(true)
    expect(can('supervisor', 'ops')).toBe(true)
    expect(can('supervisor', 'system')).toBe(false)
  })
  it('system_admin sees everything', () => {
    expect(['daily', 'ops', 'analytics', 'system'].every((a) => can('system_admin', a as never))).toBe(true)
  })
  it('AREA_ACCESS is the source of truth', () => {
    expect(AREA_ACCESS.agent).toEqual(['daily'])
  })
})

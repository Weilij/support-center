import { describe, expect, it } from 'vitest'

import { API_ENDPOINTS, endpointByKey, endpointsForArea, type EndpointArea } from '../api/endpoints'

describe('frontend endpoint contract registry', () => {
  it('covers every traceability-matrix frontend area', () => {
    const requiredAreas: EndpointArea[] = [
      'auth',
      'teams',
      'conversations',
      'realtime',
      'tags',
      'autoReply',
      'dataExport',
      'activities',
      'monitoring',
      'notifications',
      'reports',
      'settings',
      'channels',
    ]

    for (const area of requiredAreas) {
      expect(endpointsForArea(area).length, area).toBeGreaterThan(0)
    }
  })

  it('lists the critical conversation and realtime contracts explicitly', () => {
    expect(endpointByKey('conversations.list')).toMatchObject({
      method: 'GET',
      path: '/api/conversations',
      teamScoped: true,
    })
    expect(endpointByKey('conversations.send')).toMatchObject({
      method: 'POST',
      path: '/api/conversations/:id/messages',
      teamScoped: true,
    })
    expect(endpointByKey('realtime.agentWs')).toMatchObject({
      method: 'GET',
      path: '/api/websocket/connect',
    })
  })

  it('keeps endpoint keys unique and paths absolute', () => {
    const keys = new Set<string>()
    for (const endpoint of API_ENDPOINTS) {
      expect(keys.has(endpoint.key), endpoint.key).toBe(false)
      keys.add(endpoint.key)
      expect(endpoint.path.startsWith('/api/'), endpoint.key).toBe(true)
    }
  })
})

// Frontend-to-backend endpoint contract registry (CRD §8.4 / §8.5).
// Runtime callers may still compose concrete paths locally; this map is the
// reviewable source that proves every user-visible frontend area has a backend
// contract and records whether calls are team scoped.

export type EndpointMethod = 'GET' | 'POST' | 'PUT' | 'DELETE'

export type EndpointArea =
  | 'auth'
  | 'teams'
  | 'conversations'
  | 'realtime'
  | 'tags'
  | 'autoReply'
  | 'dataExport'
  | 'activities'
  | 'monitoring'
  | 'notifications'
  | 'reports'
  | 'settings'
  | 'channels'

export interface EndpointContract {
  key: string
  area: EndpointArea
  method: EndpointMethod
  path: string
  auth: 'public' | 'required' | 'admin'
  teamScoped?: boolean
  purpose: string
}

export const API_ENDPOINTS: readonly EndpointContract[] = [
  { key: 'auth.login', area: 'auth', method: 'POST', path: '/api/auth/login', auth: 'public', purpose: 'Sign in' },
  { key: 'auth.me', area: 'auth', method: 'GET', path: '/api/auth/me', auth: 'required', purpose: 'Current operator' },
  { key: 'auth.profile', area: 'auth', method: 'GET', path: '/api/auth/profile', auth: 'required', purpose: 'Profile view' },
  { key: 'auth.updateProfile', area: 'auth', method: 'PUT', path: '/api/auth/me', auth: 'required', purpose: 'Profile update' },

  { key: 'teams.list', area: 'teams', method: 'GET', path: '/api/teams', auth: 'required', teamScoped: true, purpose: 'Team list' },
  { key: 'teams.create', area: 'teams', method: 'POST', path: '/api/teams', auth: 'admin', teamScoped: true, purpose: 'Create team' },
  { key: 'teams.update', area: 'teams', method: 'PUT', path: '/api/teams/:id', auth: 'admin', teamScoped: true, purpose: 'Update team' },
  { key: 'teams.delete', area: 'teams', method: 'DELETE', path: '/api/teams/:id', auth: 'admin', teamScoped: true, purpose: 'Delete team' },
  { key: 'teams.members', area: 'teams', method: 'GET', path: '/api/teams/:id/members', auth: 'required', teamScoped: true, purpose: 'Team members' },
  { key: 'teams.memberCreate', area: 'teams', method: 'POST', path: '/api/teams/members', auth: 'admin', teamScoped: true, purpose: 'Create member' },
  { key: 'teams.memberUpdate', area: 'teams', method: 'PUT', path: '/api/teams/members/:id', auth: 'admin', teamScoped: true, purpose: 'Update member' },
  { key: 'teams.memberDelete', area: 'teams', method: 'DELETE', path: '/api/teams/members/:id', auth: 'admin', teamScoped: true, purpose: 'Delete member' },

  { key: 'conversations.list', area: 'conversations', method: 'GET', path: '/api/conversations', auth: 'required', teamScoped: true, purpose: 'Conversation list' },
  { key: 'conversations.detail', area: 'conversations', method: 'GET', path: '/api/conversations/:id', auth: 'required', teamScoped: true, purpose: 'Conversation detail' },
  { key: 'conversations.messages', area: 'conversations', method: 'GET', path: '/api/conversations/:id/messages', auth: 'required', teamScoped: true, purpose: 'Message list' },
  { key: 'conversations.send', area: 'conversations', method: 'POST', path: '/api/conversations/:id/messages', auth: 'required', teamScoped: true, purpose: 'Send message' },
  { key: 'conversations.attach', area: 'conversations', method: 'POST', path: '/api/conversations/:id/attachments', auth: 'required', teamScoped: true, purpose: 'Upload attachment' },
  { key: 'conversations.read', area: 'conversations', method: 'PUT', path: '/api/conversations/:id/read', auth: 'required', teamScoped: true, purpose: 'Mark read' },
  { key: 'conversations.assign', area: 'conversations', method: 'POST', path: '/api/conversations/:id/assign', auth: 'admin', teamScoped: true, purpose: 'Assign to team' },
  { key: 'conversations.unassign', area: 'conversations', method: 'POST', path: '/api/conversations/:id/unassign', auth: 'admin', teamScoped: true, purpose: 'Unassign from team' },
  { key: 'conversations.transfer', area: 'conversations', method: 'POST', path: '/api/conversations/:id/transfer', auth: 'admin', teamScoped: true, purpose: 'Transfer team' },
  { key: 'conversations.tags', area: 'conversations', method: 'GET', path: '/api/conversations/:id/tags', auth: 'required', teamScoped: true, purpose: 'Conversation tags' },

  { key: 'realtime.agentWs', area: 'realtime', method: 'GET', path: '/api/websocket/connect', auth: 'required', purpose: 'Agent WebSocket' },
  { key: 'realtime.customerWs', area: 'realtime', method: 'GET', path: '/api/customer-ws', auth: 'required', purpose: 'Customer WebSocket' },

  { key: 'tags.list', area: 'tags', method: 'GET', path: '/api/tags', auth: 'required', teamScoped: true, purpose: 'Tag list' },
  { key: 'tags.create', area: 'tags', method: 'POST', path: '/api/tags', auth: 'required', teamScoped: true, purpose: 'Create tag' },
  { key: 'tags.update', area: 'tags', method: 'PUT', path: '/api/tags/:id', auth: 'required', teamScoped: true, purpose: 'Update tag' },
  { key: 'tags.delete', area: 'tags', method: 'DELETE', path: '/api/tags/:id', auth: 'required', teamScoped: true, purpose: 'Delete tag' },
  { key: 'tags.bulk', area: 'tags', method: 'POST', path: '/api/tags/bulk', auth: 'required', teamScoped: true, purpose: 'Bulk tag operation' },

  { key: 'autoReply.rules', area: 'autoReply', method: 'GET', path: '/api/auto-reply/rules', auth: 'required', teamScoped: true, purpose: 'Auto-reply rules' },
  { key: 'autoReply.ruleCreate', area: 'autoReply', method: 'POST', path: '/api/auto-reply/rules', auth: 'required', teamScoped: true, purpose: 'Create auto-reply rule' },
  { key: 'autoReply.ruleUpdate', area: 'autoReply', method: 'PUT', path: '/api/auto-reply/rules/:id', auth: 'required', teamScoped: true, purpose: 'Update auto-reply rule' },
  { key: 'autoReply.ruleDelete', area: 'autoReply', method: 'DELETE', path: '/api/auto-reply/rules/:id', auth: 'required', teamScoped: true, purpose: 'Delete auto-reply rule' },
  { key: 'autoReply.schedules', area: 'autoReply', method: 'GET', path: '/api/auto-reply/schedules', auth: 'required', teamScoped: true, purpose: 'Schedules' },
  { key: 'autoReply.logs', area: 'autoReply', method: 'GET', path: '/api/auto-reply/logs', auth: 'required', teamScoped: true, purpose: 'Execution logs' },

  { key: 'dataExport.messages', area: 'dataExport', method: 'GET', path: '/api/messages/export', auth: 'required', teamScoped: true, purpose: 'Message export' },
  { key: 'dataExport.count', area: 'dataExport', method: 'GET', path: '/api/messages/export/count', auth: 'required', teamScoped: true, purpose: 'Export count' },
  { key: 'dataExport.customers', area: 'dataExport', method: 'GET', path: '/api/messages/export/customers', auth: 'required', teamScoped: true, purpose: 'Export customer options' },
  { key: 'dataExport.agents', area: 'dataExport', method: 'GET', path: '/api/messages/export/agents', auth: 'required', teamScoped: true, purpose: 'Export agent options' },

  { key: 'activities.list', area: 'activities', method: 'GET', path: '/api/activities', auth: 'required', teamScoped: true, purpose: 'Activity list' },
  { key: 'activities.detail', area: 'activities', method: 'GET', path: '/api/activities/:id', auth: 'required', teamScoped: true, purpose: 'Activity detail' },
  { key: 'activities.restore', area: 'activities', method: 'POST', path: '/api/activities/:id/restore', auth: 'required', teamScoped: true, purpose: 'Restore activity' },
  { key: 'activities.export', area: 'activities', method: 'GET', path: '/api/activities/export', auth: 'required', teamScoped: true, purpose: 'Activity export' },

  { key: 'monitoring.health', area: 'monitoring', method: 'GET', path: '/api/monitoring/health', auth: 'admin', purpose: 'Monitoring health' },
  { key: 'monitoring.metrics', area: 'monitoring', method: 'GET', path: '/api/monitoring/metrics', auth: 'admin', purpose: 'Monitoring metrics' },
  { key: 'monitoring.systemHealth', area: 'monitoring', method: 'GET', path: '/api/health/system', auth: 'admin', purpose: 'System health' },

  { key: 'notifications.list', area: 'notifications', method: 'GET', path: '/api/notifications', auth: 'required', teamScoped: true, purpose: 'Notification list' },
  { key: 'notifications.read', area: 'notifications', method: 'PUT', path: '/api/notifications/:id/read', auth: 'required', purpose: 'Mark notification read' },
  { key: 'notifications.unreadCount', area: 'notifications', method: 'GET', path: '/api/notifications/unread-count', auth: 'required', purpose: 'Unread count' },
  { key: 'notifications.stats', area: 'notifications', method: 'GET', path: '/api/notifications/stats', auth: 'admin', purpose: 'Notification stats' },

  { key: 'reports.list', area: 'reports', method: 'GET', path: '/api/reports', auth: 'required', teamScoped: true, purpose: 'Report list' },
  { key: 'reports.create', area: 'reports', method: 'POST', path: '/api/reports', auth: 'required', teamScoped: true, purpose: 'Generate report' },
  { key: 'reports.download', area: 'reports', method: 'GET', path: '/api/reports/:id/download', auth: 'required', teamScoped: true, purpose: 'Download report' },
  { key: 'reports.preview', area: 'reports', method: 'POST', path: '/api/reports/preview', auth: 'required', teamScoped: true, purpose: 'Preview report' },

  { key: 'settings.system', area: 'settings', method: 'GET', path: '/api/system/settings', auth: 'admin', purpose: 'System settings' },
  { key: 'settings.update', area: 'settings', method: 'PUT', path: '/api/system/settings', auth: 'admin', purpose: 'Update settings' },
  { key: 'channels.list', area: 'channels', method: 'GET', path: '/api/channels', auth: 'admin', teamScoped: true, purpose: 'Channel list' },
  { key: 'channels.verify', area: 'channels', method: 'POST', path: '/api/channels/:id/verify', auth: 'admin', teamScoped: true, purpose: 'Verify channel' },
]

export function endpointByKey(key: string): EndpointContract | undefined {
  return API_ENDPOINTS.find((endpoint) => endpoint.key === key)
}

export function endpointsForArea(area: EndpointArea): EndpointContract[] {
  return API_ENDPOINTS.filter((endpoint) => endpoint.area === area)
}

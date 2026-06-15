// Channel definitions — ported from handoff assets/components.jsx CHANNELS dict.
// channelOf() maps backend platform strings to channel keys.

export interface ChannelDef {
  name: string
  short: string
  color: string
  glyph: string
}

export const CHANNELS: Record<string, ChannelDef> = {
  chat: { name: '線上即時聊天', short: 'Live Chat', color: '#0ea5e9', glyph: 'chat' },
  line: { name: 'LINE', short: 'LINE', color: '#06c755', glyph: 'chat' },
  wa:   { name: 'WhatsApp', short: 'WhatsApp', color: '#25d366', glyph: 'phone' },
  fb:   { name: 'Messenger', short: 'Messenger', color: '#0084ff', glyph: 'chat' },
}

// Maps backend platform strings to a CHANNELS key.
const PLATFORM_MAP: Record<string, string> = {
  line:      'line',
  facebook:  'fb',
  messenger: 'fb',
  fb:        'fb',
  whatsapp:  'wa',
  wa:        'wa',
  webchat:   'chat',
  chat:      'chat',
  livechat:  'chat',
}

export function channelOf(platform: string): string {
  return PLATFORM_MAP[platform?.toLowerCase()] ?? 'chat'
}

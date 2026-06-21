// Channel definitions — ported from handoff assets/components.jsx CHANNELS dict.
// channelOf() maps backend platform strings to channel keys.

export interface ChannelDef {
  name: string
  short: string
  color: string
  glyph: string
}

export const CHANNELS: Record<string, ChannelDef> = {
  chat:   { name: '線上即時聊天', short: 'Live Chat', color: 'var(--chat-blue)',     glyph: 'chat' },
  line:   { name: 'LINE',        short: 'LINE',       color: 'var(--brand-line)',    glyph: 'chat' },
  wa:     { name: 'WhatsApp',    short: 'WhatsApp',   color: 'var(--wa-green)',      glyph: 'phone' },
  fb:     { name: 'Messenger',   short: 'Messenger',  color: 'var(--brand-fb)',      glyph: 'chat' },
  ig:     { name: 'Instagram',   short: 'IG',         color: 'var(--brand-ig)',      glyph: 'chat' },
  shopee: { name: 'Shopee',      short: 'Shopee',     color: 'var(--brand-shopee)',  glyph: 'chat' },
}

const PLATFORM_MAP: Record<string, string> = {
  line:      'line',
  facebook:  'fb',
  messenger: 'fb',
  fb:        'fb',
  instagram: 'ig',
  ig:        'ig',
  shopee:    'shopee',
  whatsapp:  'wa',
  wa:        'wa',
  webchat:   'chat',
  chat:      'chat',
  livechat:  'chat',
}

export function channelOf(platform: string): string {
  return PLATFORM_MAP[platform?.toLowerCase()] ?? 'chat'
}

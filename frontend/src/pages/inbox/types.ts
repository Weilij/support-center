export interface ConvMeta {
  platform?: string
  platformUserId?: string
  teamId?: number | null
  customerId?: number | null
  customerName?: string
  avatarUrl?: string | null
}

export interface PendingAttachment {
  id: string
  name: string
  mime: string
  previewUrl?: string
}

export interface InboxMessage {
  id: string
  content?: string
  senderType?: string
  senderName?: string
  createdAt?: string
  pending?: boolean
  messageType?: string
  media?: Record<string, unknown>
  attachments?: Array<{ id: string; filename?: string; mimeType?: string; url?: string; downloadUrl?: string }>
}

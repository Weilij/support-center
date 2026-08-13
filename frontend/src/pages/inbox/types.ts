export interface ConvMeta {
  platform?: string
  platformUserId?: string
  teamId?: number | null
  teamName?: string | null
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
  deliveryStatus?: string
  isSent?: boolean
  /// Classified outbound failure. REST sends both fields under these names; the
  /// `message_updated` event names the text `error` (CRD 828) and is mapped on
  /// arrival, so everything downstream reads one shape.
  rejectCode?: string
  rejectMessage?: string
  readAt?: string | null
  metadata?: Record<string, unknown>
  messageType?: string
  media?: Record<string, unknown>
  attachments?: Array<{ id: string; filename?: string; mimeType?: string; url?: string; downloadUrl?: string }>
}

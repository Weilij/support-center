// Conversation attachments (Phase 1.3). Agents upload files into a
// conversation (platform=admin) and browse/download what's attached. Upload
// goes through the multipart `upload()` helper; listing reuses unwrapList.

import { get, upload, unwrapList } from '../api/client'

export interface Attachment {
  id: string
  filename?: string
  originalName?: string
  contentType?: string
  size?: number
  url?: string
  publicUrl?: string
  conversationId?: string
  uploadStatus?: string
  createdAt?: string
}

export async function loadConversationFiles(conversationId: string): Promise<Attachment[]> {
  const resp = await get<Attachment[]>(`/api/files/conversation/${conversationId}`)
  return unwrapList<Attachment>(resp as never).items
}

/// Upload one file into the conversation. Returns the stored attachment, or an
/// error message string on failure (shape FileUpload's onUpload expects).
export async function uploadConversationFile(
  conversationId: string,
  file: File,
): Promise<{ attachment?: Attachment; error?: string }> {
  const form = new FormData()
  form.append('file', file)
  form.append('conversationId', conversationId)
  const resp = await upload<Attachment>('/api/files/upload/admin', form)
  if (resp.success && resp.data) return { attachment: resp.data }
  return { error: resp.message ?? '上傳失敗' }
}

/// Fetch a short-lived signed URL for downloading an attachment.
export async function fileDownloadUrl(fileId: string): Promise<string | null> {
  const resp = await get<{ url?: string }>(`/api/files/${fileId}/download-url`)
  return resp.success && resp.data?.url ? resp.data.url : null
}

// Delayed (scheduled) messages, v2 API (Phase 2.5). Scheduling is per
// conversation and needs the recipient's platform + platform user id, so these
// helpers are consumed from the conversation screen.

import { get, post, del, buildQuery } from '../api/client'

export interface PendingDelayed {
  messageId: string
  preview?: string
  scheduledSendTime?: number
  remainingMs?: number
}

export interface ScheduleDelayedInput {
  conversationId: string
  content: string
  platform: string
  userId: string
  delaySeconds: number
  messageType?: string
}

export async function loadPendingDelayed(conversationId: string): Promise<PendingDelayed[]> {
  const resp = await get<{ messages?: PendingDelayed[] }>(
    `/api/delayed-messages-v2/pending${buildQuery({ conversationId })}`,
  )
  return resp.success && resp.data ? resp.data.messages ?? [] : []
}

export async function scheduleDelayed(
  input: ScheduleDelayedInput,
): Promise<{ ok: boolean; message?: string }> {
  const resp = await post('/api/delayed-messages-v2/send', input)
  return { ok: resp.success, message: resp.message }
}

export async function cancelDelayed(messageId: string): Promise<boolean> {
  return (await del(`/api/delayed-messages-v2/cancel/${messageId}`)).success
}

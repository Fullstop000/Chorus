import { get, post } from './client'
import { queryString } from './common'
import { queryOptions } from '@tanstack/react-query'
import type {
  HistoryMessage,
  HistoryResponse,
  UploadResponse,
  AttachmentRef,
} from '../types'

export type {
  AttachmentRef,
  ForwardedFrom,
  HistoryMessage,
  HistoryResponse,
  StreamEvent,
  UploadResponse,
  Target,
} from '../types'

function conversationPath(conversationId: string, suffix = ''): string {
  return `/api/conversations/${encodeURIComponent(conversationId)}${suffix}`
}

export function sendMessage(
  conversationId: string,
  content: string,
  attachmentIds?: string[],
  options?: {
    suppressAgentDelivery?: boolean
    clientNonce?: string
    threadParentId?: string
  }
): Promise<{ messageId: string; seq: number; createdAt: string; clientNonce?: string }> {
  return post(conversationPath(conversationId, '/messages'), {
    content,
    attachmentIds: attachmentIds ?? [],
    clientNonce: options?.clientNonce,
    suppressAgentDelivery: options?.suppressAgentDelivery ?? false,
    threadParentId: options?.threadParentId,
  })
}

export function getHistory(
  conversationId: string,
  limit = 50,
  threadParentId?: string,
  before?: number,
  after?: number
): Promise<HistoryResponse> {
  return get(
    `${conversationPath(conversationId, '/messages')}${queryString({
      limit,
      threadParentId,
      before,
      after,
    })}`
  )
}

export function getHistoryAfter(
  conversationId: string,
  after: number,
  limit = 50,
  threadParentId?: string
): Promise<HistoryResponse> {
  return getHistory(conversationId, limit, threadParentId, undefined, after)
}

export function uploadFile(file: File): Promise<UploadResponse> {
  const form = new FormData()
  form.append('file', file)
  return post('/api/attachments', form)
}

export function attachmentUrl(id: string): string {
  return `/api/attachments/${id}`
}

export interface ReadCursorResponse {
  ok: boolean
  conversationUnreadCount: number
  conversationLastReadSeq: number
  conversationLatestSeq: number
  conversationThreadUnreadCount: number
  threadParentId?: string
  threadUnreadCount?: number
  threadLastReadSeq?: number
  threadLatestSeq?: number
}

export function updateReadCursor(
  conversationId: string,
  lastReadSeq: number,
  threadParentId?: string
): Promise<ReadCursorResponse> {
  return post(conversationPath(conversationId, '/read-cursor'), {
    lastReadSeq,
    threadParentId,
  })
}

export function sortMessagesBySeq(messages: HistoryMessage[]): HistoryMessage[] {
  return [...messages].sort((a, b) => a.seq - b.seq)
}

export function findAttachmentById(message: HistoryMessage, id: string): AttachmentRef | undefined {
  return message.attachments?.find((a) => a.id === id)
}

export const historyQueryKeys = {
  history: (conversationId: string, threadParentId?: string | null) =>
    ['history', conversationId, threadParentId ?? 'root'] as const,
} as const

export const historyQuery = (
  conversationId: string,
  threadParentId?: string | null
) =>
  queryOptions({
    queryKey: historyQueryKeys.history(conversationId, threadParentId),
    queryFn: () => getHistory(conversationId, 50, threadParentId ?? undefined),
    enabled: !!conversationId,
    staleTime: 30_000,
  })

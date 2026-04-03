import { get, post } from './client'
import { queryString } from './common'
import { queryOptions } from '@tanstack/react-query'
import type {
  SendMessageRequest,
  GetHistoryParams,
  UpdateReadCursorRequest,
} from './requests'

// ── Types (source of truth) ──

export interface AttachmentRef {
  id: string
  filename: string
}

export interface ForwardedFrom {
  channelName: string
  senderName: string
}

export interface HistoryMessage {
  id: string
  seq: number
  content: string
  senderName: string
  senderType: 'human' | 'agent'
  senderDeleted: boolean
  createdAt: string
  thread_parent_id?: string
  attachments?: AttachmentRef[]
  replyCount?: number
  forwardedFrom?: ForwardedFrom
  clientNonce?: string
  clientStatus?: 'sending' | 'failed'
  clientError?: string
}

export interface HistoryResponse {
  messages: HistoryMessage[]
  has_more: boolean
  last_read_seq: number
}

export interface StreamEvent {
  eventType: string
  channelId: string
  latestSeq: number
  payload: Record<string, unknown>
  schemaVersion: number
}

export interface UploadResponse {
  id: string
  filename: string
  sizeBytes: number
}

export type Target = string

// ── API functions ──

function conversationPath(conversationId: string, suffix = ''): string {
  return `/api/conversations/${encodeURIComponent(conversationId)}${suffix}`
}

export function sendMessage(
  conversationId: string,
  content: string,
  attachmentIds?: string[],
  options?: Partial<SendMessageRequest>
): Promise<{ messageId: string; seq: number; createdAt: string; clientNonce?: string }> {
  return post(conversationPath(conversationId, '/messages'), {
    content,
    attachmentIds: attachmentIds ?? [],
    ...options,
  })
}

export function getHistory(
  conversationId: string,
  limit = 50,
  threadParentId?: string,
  before?: number,
  after?: number
): Promise<HistoryResponse> {
  const params: GetHistoryParams = { limit, threadParentId, before, after }
  return get(
    `${conversationPath(conversationId, '/messages')}${queryString(params as Record<string, string | number | boolean | undefined>)}`
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
  const payload: UpdateReadCursorRequest = { lastReadSeq, threadParentId }
  return post(conversationPath(conversationId, '/read-cursor'), payload)
}

// ── Transforms ──

export function sortMessagesBySeq(messages: HistoryMessage[]): HistoryMessage[] {
  return [...messages].sort((a, b) => a.seq - b.seq)
}

export function findAttachmentById(message: HistoryMessage, id: string): AttachmentRef | undefined {
  return message.attachments?.find((a) => a.id === id)
}

// ── Query definitions ──

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

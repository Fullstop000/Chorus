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

// Kind-discriminated structured payload for any message variant that needs
// rich rendering on top of the always-readable `content` fallback. Currently
// used for `member_joined` chips and `task_event` cards; new kinds just add
// a renderer branch — no schema change.
//
// The wire shape is intentionally loose at this layer (`{ kind, ...rest }`).
// Each renderer narrows the shape it cares about at use time. Producers tag
// audience-only payloads with `audience: "humans"`; agents filter on it.
export interface MessagePayload {
  kind: string
  audience?: 'humans'
  [key: string]: unknown
}

export interface HistoryMessage {
  id: string
  seq: number
  content: string
  /** Stable sender id (humans.id or agents.id). Use this to correlate
   *  messages with id-keyed state (traces, agent records). */
  senderId: string
  senderName: string
  senderType: 'human' | 'agent' | 'system'
  senderDeleted: boolean
  createdAt: string
  attachments?: AttachmentRef[]
  forwardedFrom?: ForwardedFrom
  runId?: string
  traceSummary?: string
  payload?: MessagePayload
}

export interface HistoryResponse {
  messages: HistoryMessage[]
  has_more: boolean
  last_read_seq: number
}

export interface MessageSenderInfo {
  /** Stable sender id (humans.id or agents.id). */
  id: string
  name: string
  type: 'human' | 'agent' | 'system'
}

export interface MessageCreatedPayload {
  messageId: string
  conversationId: string
  conversationType: string
  sender: MessageSenderInfo
  senderDeleted: boolean
  content: string
  attachmentIds: string[]
  attachments: unknown[]
  seq: number
  createdAt: string
  runId?: string | null
  traceSummary?: string | null
  payload?: MessagePayload | null
}

export interface StreamEvent {
  eventType: string
  channelId: string
  latestSeq: number
  payload: Record<string, unknown> & Partial<MessageCreatedPayload>
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
): Promise<{ messageId: string; seq: number; createdAt: string }> {
  return post(conversationPath(conversationId, '/messages'), {
    content,
    attachmentIds: attachmentIds ?? [],
    ...options,
  })
}

export function getHistory(
  conversationId: string,
  limit = 50,
  before?: number,
  after?: number
): Promise<HistoryResponse> {
  const params: GetHistoryParams = { limit, before, after }
  return get(
    `${conversationPath(conversationId, '/messages')}${queryString(params as Record<string, string | number | boolean | undefined>)}`
  )
}

export function getHistoryAfter(
  conversationId: string,
  after: number,
  limit = 50
): Promise<HistoryResponse> {
  return getHistory(conversationId, limit, undefined, after)
}

export function uploadFile(file: File): Promise<UploadResponse> {
  const form = new FormData()
  form.append('file', file)
  return post('/api/attachments', form)
}

export function attachmentUrl(id: string): string {
  return `/api/attachments/${id}`
}

// ── Trace types ──

export interface TraceSummary {
  toolCalls: number
  duration: number
  status: 'completed' | 'error'
  categories: Record<string, number>
}

export interface TraceEventRecord {
  runId: string
  seq: number
  timestampMs: number
  kind: string
  data: Record<string, string>
}

export function getTraceEvents(runId: string): Promise<{ events: TraceEventRecord[] }> {
  return get(`/api/traces/${encodeURIComponent(runId)}`)
}

export interface ReadCursorResponse {
  ok: boolean
  conversationUnreadCount: number
  conversationLastReadSeq: number
  conversationLatestSeq: number
}

export function updateReadCursor(
  conversationId: string,
  lastReadSeq: number
): Promise<ReadCursorResponse> {
  const payload: UpdateReadCursorRequest = { lastReadSeq }
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
  history: (conversationId: string) =>
    ['history', conversationId] as const,
} as const

export const historyQuery = (conversationId: string) =>
  queryOptions({
    queryKey: historyQueryKeys.history(conversationId),
    queryFn: () => getHistory(conversationId, 50),
    enabled: !!conversationId,
    staleTime: 30_000,
  })

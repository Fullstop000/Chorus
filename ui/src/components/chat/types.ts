// ── Messages, history, stream payloads, uploads ──

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

/** Encoded channel/DM string for send/history, e.g. "#all" or "dm:@alice" */
export type Target = string

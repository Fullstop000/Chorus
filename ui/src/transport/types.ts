// ── Realtime transport types ──
// Wire types for the WebSocket connection. No app/domain logic.

import type { StreamEvent } from '../data/chat'

/** Known server-sent event types. Extend as server adds new ones. */
export const EventType = {
  MessageCreated: 'message.created',
  TombstoneChanged: 'tombstone_changed',
} as const

export type EventType = (typeof EventType)[keyof typeof EventType]

/** Server event — re-exported from data layer for transport consumers. */
export type ServerEvent = StreamEvent

/** A single agent trace event delivered over the WebSocket. */
export interface TraceFrame {
  eventType: 'agent.trace'
  runId: string
  agentName: string
  seq: number
  timestampMs: number
  kind: string
  data: Record<string, string>
}

/**
 * Cross-channel task state delta. Mirrors the Rust `TaskUpdateEvent` shape
 * (camelCase serialization). Fanned out globally so the parent-channel
 * task_card host message can re-render even when the viewer is not a member
 * of the task's sub-channel.
 */
export interface TaskUpdateFrame {
  taskId: string
  channelId: string
  taskNumber: number
  /** Wire string — one of proposed/dismissed/todo/in_progress/in_review/done. */
  status: string
  owner: string | null
  subChannelId: string | null
  updatedAt: string
}

/** Discriminated frame envelope arriving over the WebSocket. */
export type RealtimeFrame =
  | { type: 'event'; event: ServerEvent }
  | { type: 'trace'; event: TraceFrame }
  | { type: 'task_update'; event: TaskUpdateFrame }
  | { type: 'error'; code: string; message: string }

// ── Realtime transport types ──
// Wire types for the WebSocket connection. No app/domain logic.

import type { StreamEvent } from '../data/chat'

/** Known server-sent event types. Extend as server adds new ones. */
export const EventType = {
  MessageCreated: 'message.created',
  TombstoneChanged: 'tombstone_changed',
  ChannelMemberJoined: 'channel.member_joined',
} as const

export type EventType = (typeof EventType)[keyof typeof EventType]

/** Server event — re-exported from data layer for transport consumers. */
export type ServerEvent = StreamEvent

/** A single agent trace event delivered over the WebSocket.
 *  Keyed by `agentId` end-to-end. Display names come from the agent
 *  record loaded separately. */
export interface TraceFrame {
  eventType: 'agent.trace'
  runId: string
  agentId: string
  channelId?: string | null
  seq: number
  timestampMs: number
  kind: string
  data: Record<string, string>
}

/** Discriminated frame envelope arriving over the WebSocket. */
export type RealtimeFrame =
  | { type: 'event'; event: ServerEvent }
  | { type: 'trace'; event: TraceFrame }
  | { type: 'error'; code: string; message: string }

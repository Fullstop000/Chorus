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

/** Discriminated frame envelope arriving over the WebSocket. */
export type RealtimeFrame =
  | { type: 'event'; event: ServerEvent }
  | { type: 'error'; code: string; message: string }

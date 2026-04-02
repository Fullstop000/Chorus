// ── Realtime WebSocket message envelope ──

import type { StreamEvent } from '../components/chat/types'

export type RealtimeMessage =
  | {
      type: 'event'
      event: StreamEvent
    }
  | {
      type: 'error'
      code: string
      message: string
    }

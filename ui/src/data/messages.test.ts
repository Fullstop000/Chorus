import { describe, expect, it } from 'vitest'

import { normalizeEvent } from './messages'
import type { StreamEvent } from './chat'
import { EventType } from '../transport'

function makeEvent(overrides: Partial<StreamEvent> = {}): StreamEvent {
  return {
    eventType: EventType.MessageCreated,
    channelId: 'conversation-1',
    latestSeq: 6,
    payload: {
      messageId: 'message-6',
      conversationId: 'conversation-1',
      conversationType: 'channel',
    },
    schemaVersion: 1,
    ...overrides,
  }
}

describe('normalizeEvent', () => {
  it('returns null for non-message events', () => {
    const event: StreamEvent = { ...makeEvent(), eventType: EventType.TombstoneChanged }
    expect(normalizeEvent(event)).toBeNull()
  })

  it('returns null when required fields are missing', () => {
    expect(normalizeEvent({ ...makeEvent(), payload: {} })).toBeNull()
  })

  it('converts a valid WS event into a HistoryMessage', () => {
    const msg = normalizeEvent(makeEvent({
      payload: {
        messageId: 'msg-1',
        conversationId: 'conv-1',
        conversationType: 'channel',
        content: 'hello',
        sender: { id: 'alice-id', name: 'alice', type: 'human' },
        seq: 10,
        createdAt: '2025-01-01T00:00:00Z',
      },
    }))
    expect(msg).toEqual({
      id: 'msg-1',
      seq: 10,
      content: 'hello',
      senderId: 'alice-id',
      senderName: 'alice',
      senderType: 'human',
      senderDeleted: false,
      createdAt: '2025-01-01T00:00:00Z',
    })
  })
})

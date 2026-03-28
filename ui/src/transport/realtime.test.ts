import { describe, expect, it } from 'vitest'

import {
  applyRealtimeEvent,
  nextRealtimeCursor,
  parseHistoryTarget,
} from './realtime'
import type { HistoryMessage, RealtimeEvent, RealtimeMessage } from '../types'

describe('realtime transport helpers', () => {
  it('parses thread targets without corrupting dm targets', () => {
    expect(parseHistoryTarget('#general:msg-1')).toEqual({
      conversationTarget: '#general',
      threadParentId: 'msg-1',
    })
    expect(parseHistoryTarget('dm:@bot-a')).toEqual({
      conversationTarget: 'dm:@bot-a',
      threadParentId: null,
    })
  })

  it('applies message.created events incrementally', () => {
    const messages: HistoryMessage[] = []
    const event: RealtimeEvent = {
      eventId: 7,
      eventType: 'message.created',
      scopeKind: 'channel',
      scopeId: 'channel:abc',
      payload: {
        messageId: 'msg-1',
        content: 'hello',
        sender: { name: 'alice', type: 'human' },
        senderDeleted: true,
        forwardedFrom: {
          channelName: 'eng-team',
          senderName: 'bob',
        },
        seq: 1,
        createdAt: '2026-03-28T00:00:00Z',
      },
      createdAt: '2026-03-28T00:00:00Z',
    }

    expect(applyRealtimeEvent(messages, event)).toEqual([
      {
        id: 'msg-1',
        seq: 1,
        content: 'hello',
        senderName: 'alice',
        senderType: 'human',
        senderDeleted: true,
        forwardedFrom: {
          channelName: 'eng-team',
          senderName: 'bob',
        },
        createdAt: '2026-03-28T00:00:00Z',
      },
    ])
  })

  it('trusts subscribed resumeFrom as the authoritative cursor', () => {
    const frame: RealtimeMessage = {
      type: 'subscribed',
      resumeFrom: 3,
      scopes: [],
    }

    expect(nextRealtimeCursor(10, frame)).toBe(3)
  })

  it('preserves stream resume metadata on subscribed frames', () => {
    const frame: RealtimeMessage = {
      type: 'subscribed',
      resumeFrom: 0,
      streamId: 'conversation:abc',
      resumeFromStreamPos: 7,
      scopes: [],
    }

    expect(frame.streamId).toBe('conversation:abc')
    expect(frame.resumeFromStreamPos).toBe(7)
  })
})

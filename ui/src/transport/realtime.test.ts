import { describe, expect, it, vi } from 'vitest'

import {
  applyRealtimeEvent,
  nextRealtimeCursor,
  parseHistoryTarget,
  resolveRealtimeTarget,
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

  it('treats message.created bus frames as notification-only', () => {
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

    expect(applyRealtimeEvent(messages, event)).toEqual([])
  })

  it('expects conversation.state notifications to carry absolute unread state', () => {
    const event: RealtimeEvent = {
      eventId: 8,
      eventType: 'conversation.state',
      scopeKind: 'channel',
      scopeId: 'channel:abc',
      payload: {
        conversationId: 'channel:abc',
        latestSeq: 12,
        lastReadSeq: 9,
        unreadCount: 3,
      },
      createdAt: '2026-03-28T00:00:00Z',
    }

    expect(event.eventType).toBe('conversation.state')
    expect(event.payload).not.toHaveProperty('content')
    expect(event.payload).toMatchObject({
      conversationId: 'channel:abc',
      latestSeq: 12,
      lastReadSeq: 9,
      unreadCount: 3,
    })
  })

  it('trusts subscribed resumeFrom as the authoritative cursor', () => {
    const frame: RealtimeMessage = {
      type: 'subscribed',
      resumeFrom: 3,
      targets: [],
    }

    expect(nextRealtimeCursor(10, frame)).toBe(3)
  })

  it('preserves stream resume metadata on subscribed frames', () => {
    const frame: RealtimeMessage = {
      type: 'subscribed',
      resumeFrom: 0,
      streamId: 'conversation:abc',
      resumeFromStreamPos: 7,
      targets: ['conversation:abc'],
    }

    expect(frame.targets).toEqual(['conversation:abc'])
    expect(frame.streamId).toBe('conversation:abc')
    expect(frame.resumeFromStreamPos).toBe(7)
  })

  it('resolves thread history targets to thread subscription targets', async () => {
    const api = await import('../api')
    const spy = vi.spyOn(api, 'resolveChannel').mockResolvedValue({
      channelId: 'conv-1',
    })

    await expect(resolveRealtimeTarget('alice', '#general:msg-1')).resolves.toBe('thread:msg-1')
    await expect(resolveRealtimeTarget('alice', 'dm:@bot-a')).resolves.toBe(
      'conversation:conv-1'
    )

    spy.mockRestore()
  })
})

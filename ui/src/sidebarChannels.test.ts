import { describe, expect, it } from 'vitest'
import { isVisibleSidebarChannel } from './sidebarChannels'
import type { ChannelInfo } from './types'

function makeChannel(overrides: Partial<ChannelInfo>): ChannelInfo {
  return {
    id: 'channel-1',
    name: 'qa-eng',
    description: undefined,
    joined: false,
    ...overrides,
  }
}

describe('isVisibleSidebarChannel', () => {
  it('keeps joined user channels visible', () => {
    expect(
      isVisibleSidebarChannel(makeChannel({ channel_type: 'channel', joined: true }))
    ).toBe(true)
  })

  it('hides unjoined non-team channels', () => {
    expect(
      isVisibleSidebarChannel(makeChannel({ channel_type: 'channel', joined: false }))
    ).toBe(false)
  })

  it('keeps team channels visible even when the human is not a member', () => {
    expect(
      isVisibleSidebarChannel(makeChannel({ channel_type: 'team', joined: false }))
    ).toBe(true)
  })
})

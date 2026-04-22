import { describe, expect, it } from 'vitest'
import { isVisibleSidebarChannel } from './sidebarChannels'
import type { ChannelInfo } from '../../components/channels/types'

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

  it('task sub-channels are never visible, even when joined', () => {
    expect(
      isVisibleSidebarChannel(
        makeChannel({
          channel_type: 'task',
          joined: true,
          parent_channel_id: 'parent-1',
        })
      )
    ).toBe(false)
  })

  it('task sub-channels are never visible, even when marked as a team channel', () => {
    // Belt-and-suspenders: if the short-circuit is placed correctly,
    // 'task' wins over any other property — no path through the function
    // can render a task sub-channel in the sidebar tree.
    expect(
      isVisibleSidebarChannel(
        makeChannel({
          channel_type: 'task',
          joined: false,
          parent_channel_id: 'parent-1',
        })
      )
    ).toBe(false)
  })
})

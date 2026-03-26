import type { ChannelInfo } from './types'

export function isVisibleSidebarChannel(channel: ChannelInfo): boolean {
  return channel.joined || channel.channel_type === 'team'
}

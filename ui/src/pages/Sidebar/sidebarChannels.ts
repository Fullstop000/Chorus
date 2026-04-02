import type { ChannelInfo } from '../../components/channels/types'

export function isVisibleSidebarChannel(channel: ChannelInfo): boolean {
  return channel.joined || channel.channel_type === 'team'
}

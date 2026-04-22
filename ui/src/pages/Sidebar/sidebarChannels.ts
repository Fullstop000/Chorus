import type { ChannelInfo } from '../../components/channels/types'

export function isVisibleSidebarChannel(channel: ChannelInfo): boolean {
  // Task sub-channels live under a parent channel's task list and must never
  // appear in the flat sidebar tree — regardless of membership or type flags.
  if (channel.channel_type === 'task') return false
  return channel.joined || channel.channel_type === 'team'
}

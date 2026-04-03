import { ChevronDown } from 'lucide-react'

interface NewMessageBadgeProps {
  unreadCount: number
  onScrollToBottom?: () => void
}

export function NewMessageBadge({ unreadCount, onScrollToBottom }: NewMessageBadgeProps) {
  if (unreadCount <= 0) return null

  return (
    <button
      className="new-message-badge"
      type="button"
      onClick={onScrollToBottom}
      title={`Jump to ${unreadCount} new message${unreadCount > 1 ? 's' : ''}`}
    >
      <ChevronDown size={12} strokeWidth={2.5} />
      <span>{unreadCount} new message{unreadCount > 1 ? 's' : ''}</span>
    </button>
  )
}

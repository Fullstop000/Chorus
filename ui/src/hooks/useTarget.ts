import { useRouteSubject } from './useRouteSubject'

/**
 * Backend routing key for the current selection. Centralizes the `#`/`dm:@`
 * prefix. Reads from the URL via `useRouteSubject` — replaces the previous
 * implementation that read `currentChannel`/`currentAgent` from `uiStore`.
 */
export function useTarget(): string | null {
  const subject = useRouteSubject()
  if (subject.kind === 'channel' || subject.kind === 'task') return `#${subject.channel.name}`
  if (subject.kind === 'dm') return `dm:@${subject.agent.name}`
  return null
}

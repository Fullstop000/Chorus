/**
 * URL path builders. All link construction goes through these helpers.
 *
 * Names are `encodeURIComponent`-encoded so DM channel names (`dm:a:b`)
 * round-trip safely. Empty / `.` / `..` inputs throw — `encodeURIComponent`
 * leaves those unchanged and they get normalized away by browsers.
 */

function encodeSegment(name: string, kind: 'channel' | 'agent'): string {
  if (!name || name === '.' || name === '..') {
    throw new Error(`invalid ${kind} name for URL: ${JSON.stringify(name)}`)
  }
  return encodeURIComponent(name)
}

export type AgentTab = 'profile' | 'activity' | 'workspace'
export type SettingsSection =
  | 'profile'
  | 'devices'
  | 'workspaces'
  | 'appearance'
  | 'system'
  | 'logs'

export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  'profile',
  'devices',
  'workspaces',
  'appearance',
  'system',
  'logs',
]

export function isSettingsSection(value: string): value is SettingsSection {
  return (SETTINGS_SECTIONS as readonly string[]).includes(value)
}

export function rootPath(): string {
  return '/'
}

export function channelPath(name: string): string {
  return `/c/${encodeSegment(name, 'channel')}`
}

export function tasksBoardPath(name: string): string {
  return `${channelPath(name)}/tasks`
}

export function taskDetailPath(name: string, taskNumber: number): string {
  if (!Number.isInteger(taskNumber) || taskNumber <= 0) {
    throw new Error(`invalid task number for URL: ${taskNumber}`)
  }
  return `${tasksBoardPath(name)}/${taskNumber}`
}

export function dmPath(agentName: string): string {
  return `/dm/${encodeSegment(agentName, 'agent')}`
}

export function agentTabPath(agentName: string, tab: AgentTab): string {
  return `/agent/${encodeSegment(agentName, 'agent')}/${tab}`
}

export function inboxPath(): string {
  return '/inbox'
}

export function settingsPath(section?: SettingsSection): string {
  return section ? `/settings/${section}` : '/settings'
}

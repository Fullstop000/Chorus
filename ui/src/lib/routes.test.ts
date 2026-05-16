import { describe, it, expect } from 'vitest'
import {
  channelPath,
  tasksBoardPath,
  taskDetailPath,
  dmPath,
  agentTabPath,
  inboxPath,
  settingsPath,
  rootPath,
  isSettingsSection,
} from './routes'

describe('routes helpers', () => {
  it('builds channel paths', () => {
    expect(channelPath('general')).toBe('/c/general')
  })

  it('encodes colon for DM channel names', () => {
    // DM channels carry server-generated names like dm:<id>:<id>
    expect(channelPath('dm:abc:def')).toBe('/c/dm%3Aabc%3Adef')
  })

  it('builds nested channel paths', () => {
    expect(tasksBoardPath('general')).toBe('/c/general/tasks')
    expect(taskDetailPath('general', 7)).toBe('/c/general/tasks/7')
  })

  it('builds agent paths', () => {
    expect(dmPath('alice')).toBe('/dm/alice')
    expect(agentTabPath('alice', 'profile')).toBe('/agent/alice/profile')
    expect(agentTabPath('alice', 'activity')).toBe('/agent/alice/activity')
    expect(agentTabPath('alice', 'workspace')).toBe('/agent/alice/workspace')
  })

  it('builds singleton paths', () => {
    expect(rootPath()).toBe('/')
    expect(inboxPath()).toBe('/inbox')
    expect(settingsPath()).toBe('/settings')
    expect(settingsPath('logs')).toBe('/settings/logs')
  })

  it('throws on empty / . / .. inputs', () => {
    expect(() => channelPath('')).toThrow()
    expect(() => channelPath('.')).toThrow()
    expect(() => channelPath('..')).toThrow()
    expect(() => dmPath('')).toThrow()
    expect(() => dmPath('..')).toThrow()
    expect(() => agentTabPath('', 'profile')).toThrow()
  })

  it('throws on invalid task numbers', () => {
    expect(() => taskDetailPath('general', 0)).toThrow()
    expect(() => taskDetailPath('general', -1)).toThrow()
    expect(() => taskDetailPath('general', 1.5)).toThrow()
  })

  it('round-trips through decodeURIComponent', () => {
    const original = 'dm:abc:def'
    const encoded = channelPath(original)
    // Routes look like /c/<encoded>; extract and decode
    const segment = encoded.slice('/c/'.length)
    expect(decodeURIComponent(segment)).toBe(original)
  })

  it('isSettingsSection narrows known sections', () => {
    expect(isSettingsSection('logs')).toBe(true)
    expect(isSettingsSection('profile')).toBe(true)
    expect(isSettingsSection('nope')).toBe(false)
    expect(isSettingsSection('')).toBe(false)
  })
})

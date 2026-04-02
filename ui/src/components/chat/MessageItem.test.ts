import { describe, expect, it } from 'vitest'

const MENTION_REGEX = /(@[\w-]+)/g

describe('mention regex', () => {
  it('matches simple agent mentions', () => {
    const text = 'hello @bot how are you'
    const matches = text.match(MENTION_REGEX)
    expect(matches).toEqual(['@bot'])
  })

  it('matches hyphenated agent mentions', () => {
    const text = 'hello @bot-a how are you'
    const matches = text.match(MENTION_REGEX)
    expect(matches).toEqual(['@bot-a'])
  })

  it('matches multiple mentions with mixed names', () => {
    const text = 'hello @bot and @agent-x and @kimi-ai'
    const matches = text.match(MENTION_REGEX)
    expect(matches).toEqual(['@bot', '@agent-x', '@kimi-ai'])
  })

  it('matches mentions with trailing hyphen', () => {
    const text = 'hello @bot- how are you'
    const matches = text.match(MENTION_REGEX)
    expect(matches).toEqual(['@bot-'])
  })

  it('splits text correctly around mentions', () => {
    const text = 'hello @bot-a world'
    const parts = text.split(MENTION_REGEX)
    expect(parts).toEqual(['hello ', '@bot-a', ' world'])
  })
})

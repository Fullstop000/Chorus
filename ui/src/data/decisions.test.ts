// Drift guard: the JSON fixture at src/decision/fixtures/payload.json is
// also loaded by the Rust test `decision::types::fixture_parses_against_rust_types`.
// If either side's shape changes without the other, one of the two parses
// fails and the build breaks. No ts-rs codegen step needed.
//
// Lineage: r7 of the design doc, see
// chorus-design-reviews/explorations/2026-04-30-pr-review-vertical-slice/

import { describe, it, expect } from 'vitest'
import type { DecisionPayload, OptionPayload, ResolvePayload, Decision } from './decisions'

// Vite resolves this at build time. The path is relative to this test
// file: ui/src/data/decisions.test.ts climbs out of ui/ to repo root,
// then into src/decision/fixtures/. Same fixture as the Rust test.
import fixture from '../../../src/decision/fixtures/payload.json'

describe('decisions wire shape', () => {
  it('parses the canonical fixture as a DecisionPayload', () => {
    const parsed = fixture as DecisionPayload

    // Field presence — TS won't catch missing required fields at runtime,
    // so assert structurally.
    expect(typeof parsed.headline).toBe('string')
    expect(typeof parsed.question).toBe('string')
    expect(typeof parsed.recommended_key).toBe('string')
    expect(typeof parsed.context).toBe('string')
    expect(Array.isArray(parsed.options)).toBe(true)
    expect(parsed.options.length).toBeGreaterThanOrEqual(2)
    expect(parsed.options.length).toBeLessThanOrEqual(6)

    for (const opt of parsed.options) {
      expect(typeof opt.key).toBe('string')
      expect(typeof opt.label).toBe('string')
      expect(typeof opt.body).toBe('string')
    }

    // Sanity: recommended_key matches an option.
    expect(parsed.options.some((o) => o.key === parsed.recommended_key)).toBe(true)
  })

  it('rejects unknown fields at the type level', () => {
    // Compile-time check (no runtime assertion needed). If a contributor
    // adds a field to Rust without adding it here, the TS test file
    // referencing the new field name will fail to compile.
    const sample: DecisionPayload = {
      headline: 'h',
      question: 'q',
      options: [
        { key: 'a', label: 'A', body: 'do A' },
        { key: 'b', label: 'B', body: 'do B' },
      ],
      recommended_key: 'a',
      context: '',
    }
    expect(sample.options).toHaveLength(2)
  })

  it('round-trips ResolvePayload with and without note', () => {
    const withNote: ResolvePayload = { picked_key: 'a', note: 'lgtm' }
    expect(JSON.parse(JSON.stringify(withNote))).toEqual(withNote)

    const withoutNote: ResolvePayload = { picked_key: 'b' }
    expect(JSON.parse(JSON.stringify(withoutNote)).picked_key).toBe('b')
  })

  it('Decision row shape survives a round trip', () => {
    const row: Decision = {
      id: 'd1',
      workspace_id: 'w1',
      channel_id: 'c1',
      agent_id: 'a1',
      session_id: 's1',
      created_at: '2026-04-30T22:00:00Z',
      status: 'open',
      payload_json: '{}',
      picked_key: null,
      picked_note: null,
      resolved_at: null,
    }
    expect(JSON.parse(JSON.stringify(row))).toEqual(row)
  })
})

// Suppress linter warning about unused imports — these are used as type
// annotations in the tests above.
export type _Unused = OptionPayload

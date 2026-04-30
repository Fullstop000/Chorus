// Decision-inbox types — mirror of Rust types in `src/decision/types.rs`.
//
// IMPORTANT: this file is hand-maintained. The wire format is snake_case
// (matching the Rust serde defaults), NOT the camelCase convention used
// elsewhere in this UI. Keep the field names identical to the Rust
// struct fields. The CI test `decisions.test.ts` round-trips a fixture
// to catch drift; the same fixture exists in
// `src/decision/tests/fixtures/payload.json` so a Rust-side change
// without a TS-side change fails the build.
//
// When Day 3 ships the public HTTP handlers, a separate camelCase
// `PublicDecision` shape will be added if the UI needs it; for now
// the inbox UI consumes the snake_case shape directly.
//
// Lineage: r7 of the design doc, see
// chorus-design-reviews/explorations/2026-04-30-pr-review-vertical-slice/

export type DecisionStatus = 'open' | 'resolved'

export interface OptionPayload {
  /** 1..=2 alphanumeric chars, e.g. "a", "b", "1". */
  key: string
  /** Short action verb, ≤40 chars. */
  label: string
  /** Markdown describing consequences of picking this option, ≤2048 chars. */
  body: string
}

/**
 * What the agent emits via `chorus_create_decision`. Identity columns
 * (workspace_id, channel_id, agent_id, session_id) are added by the
 * server from the bridge auth context — agents do not supply them.
 */
export interface DecisionPayload {
  /** One-line summary, ≤80 chars. */
  headline: string
  /** The actual ask, ≤120 chars. */
  question: string
  /** 2..=6 entries, unique `key`s. */
  options: OptionPayload[]
  /** Must equal one option's `key`. */
  recommended_key: string
  /**
   * Markdown body, ≤4096 chars. v1 renders plainly; H2-section parsing
   * and inline-prefix styling ship in v2 if Day-5 dogfood reveals the
   * convention is being followed.
   */
  context: string
}

/**
 * What the server stores. Mirrors the `decisions` table schema.
 * `payload_json` is the serialized `DecisionPayload`.
 */
export interface Decision {
  id: string
  workspace_id: string
  channel_id: string
  agent_id: string
  session_id: string
  /** RFC 3339 UTC timestamp. */
  created_at: string

  status: DecisionStatus
  payload_json: string

  picked_key: string | null
  picked_note: string | null
  resolved_at: string | null
}

/** Body of `POST /api/decisions/:id/resolve`. */
export interface ResolvePayload {
  picked_key: string
  /** Optional free-text note from the human, included in the envelope
   *  delivered back to the agent. */
  note?: string | null
}

// ── Public-shape view returned by GET /api/decisions ──
//
// The server returns the row with the agent payload pre-deserialized
// under `payload`, instead of the raw JSON-string `payload_json`. The
// snake_case shape matches `src/server/handlers/decisions.rs::DecisionView`.

export interface DecisionView {
  id: string
  workspace_id: string
  channel_id: string
  agent_id: string
  session_id: string
  created_at: string
  status: DecisionStatus
  payload: DecisionPayload
  picked_key: string | null
  picked_note: string | null
  resolved_at: string | null
}

export interface ListDecisionsResponse {
  decisions: DecisionView[]
}

export interface ResolveDecisionResponse {
  decision: DecisionView
}

// ── API functions ──

import { get, post } from './client'

export function listDecisions(status: 'open' | 'resolved' | 'all' = 'open'): Promise<ListDecisionsResponse> {
  return get<ListDecisionsResponse>(`/api/decisions?status=${status}`)
}

export function resolveDecision(
  id: string,
  body: ResolvePayload,
): Promise<ResolveDecisionResponse> {
  return post<ResolveDecisionResponse>(`/api/decisions/${encodeURIComponent(id)}/resolve`, body)
}

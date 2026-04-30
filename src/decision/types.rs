//! Decision-inbox types.
//!
//! See r7 design doc:
//! `chorus-design-reviews/explorations/2026-04-30-pr-review-vertical-slice/design.md`.
//!
//! The agent emits a `DecisionPayload` via the `chorus_create_decision`
//! MCP tool. The server stores it as a `Decision` row. When the human
//! picks via the UI, the server resumes the agent's runtime session
//! with a self-contained envelope as the new turn prompt.
//!
//! v1 carries five payload fields (headline, question, options,
//! recommended_key, context) plus identity columns the server fills in.
//! Everything else (urgency, deadline, confidence, reversibility, kind,
//! version) is deferred to v2 per YAGNI.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

/// The shape the agent emits.
///
/// Identity (workspace_id, channel_id, agent_id, session_id) is filled
/// in by the server from the bridge's auth context plus the active-run
/// channel inference, NOT from the payload. The agent supplies only
/// the human-readable fields below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionPayload {
    /// One-line summary, ≤80 chars. The human's first read.
    pub headline: String,
    /// The actual ask, ≤120 chars.
    pub question: String,
    /// 2..=6 options. Each has a `key` the human picks by.
    pub options: Vec<OptionPayload>,
    /// Must equal one option's `key`. Always required — the agent
    /// recommends, never abstains.
    pub recommended_key: String,
    /// Markdown body, ≤4096 chars. See the Context Convention in
    /// `docs/DECISIONS.md` for suggested H2 sections and inline prefix
    /// conventions. v1 renders plainly without H2 parsing.
    pub context: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OptionPayload {
    /// 1..=2 alphanumeric characters, e.g. "a", "b", "1".
    pub key: String,
    /// Short action verb, ≤40 chars.
    pub label: String,
    /// Markdown describing the consequences of picking this option,
    /// ≤2048 chars.
    pub body: String,
}

/// What the server stores in the `decisions` table.
///
/// 11 columns total: 5 identity + 5 agent-authored + 1 status enum
/// + delivery fields. Match the schema in `src/store/schema.sql`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub workspace_id: String,
    pub channel_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub created_at: DateTime<Utc>,

    pub status: Status,
    /// Serialized `DecisionPayload` JSON. Read-back deserializes via
    /// `serde_json::from_str`.
    pub payload_json: String,

    /// Set when status transitions to Resolved.
    pub picked_key: Option<String>,
    pub picked_note: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    Resolved,
}

/// Body of `POST /api/decisions/:id/resolve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePayload {
    /// The picked option's `key`. Must match one of the original
    /// `DecisionPayload.options[].key` values; the resolve handler
    /// rejects mismatches with 400.
    pub picked_key: String,
    /// Optional free-text note from the human, included in the
    /// envelope delivered to the agent.
    #[serde(default)]
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> DecisionPayload {
        DecisionPayload {
            headline: "PR #121 ready: archived-channel del/join fix".into(),
            question: "How do you want to land this?".into(),
            options: vec![
                OptionPayload {
                    key: "a".into(),
                    label: "Merge as-is".into(),
                    body: "Squash and merge to main. CI green. 4 files changed.".into(),
                },
                OptionPayload {
                    key: "b".into(),
                    label: "Approve + comment".into(),
                    body: "Approve so I can self-merge; note the test gap.".into(),
                },
            ],
            recommended_key: "a".into(),
            context: "## Why now\nCI green at 20:40Z.\n\n## Risk\n[team] minor regression risk.".into(),
        }
    }

    #[test]
    fn round_trip_payload() {
        let original = sample_payload();
        let json = serde_json::to_string(&original).expect("serialize");
        let parsed: DecisionPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, original);
    }

    /// Drift guard: the JSON fixture at `fixtures/payload.json` is also
    /// loaded by the TypeScript mirror's vitest test in
    /// `ui/src/data/decisions.test.ts`. If either side's shape changes
    /// without the other, one of the two parses fails and the build
    /// breaks. Cheap drift detection without a codegen step.
    #[test]
    fn fixture_parses_against_rust_types() {
        let json = include_str!("fixtures/payload.json");
        let parsed: DecisionPayload =
            serde_json::from_str(json).expect("fixture must round-trip into DecisionPayload");
        assert_eq!(parsed.options.len(), 3);
        assert_eq!(parsed.recommended_key, "a");
        assert!(parsed.context.contains("Why now"));
    }

    #[test]
    fn payload_serializes_with_snake_case() {
        let json = serde_json::to_value(sample_payload()).expect("serialize");
        // Field names must round-trip exactly so the TS mirror in
        // ui/src/types/decision.ts stays in sync.
        assert!(json.get("headline").is_some());
        assert!(json.get("question").is_some());
        assert!(json.get("options").is_some());
        assert!(json.get("recommended_key").is_some());
        assert!(json.get("context").is_some());
    }

    #[test]
    fn status_serializes_lowercase_snake() {
        assert_eq!(serde_json::to_string(&Status::Open).unwrap(), "\"open\"");
        assert_eq!(
            serde_json::to_string(&Status::Resolved).unwrap(),
            "\"resolved\""
        );
    }

    #[test]
    fn resolve_payload_omits_absent_note() {
        let r = ResolvePayload {
            picked_key: "a".into(),
            note: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        let parsed: ResolvePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.picked_key, "a");
        assert!(parsed.note.is_none());

        // Also accept JSON without the field at all.
        let parsed: ResolvePayload =
            serde_json::from_str(r#"{"picked_key":"a"}"#).expect("note is optional");
        assert!(parsed.note.is_none());
    }
}

//! Decision payload validation.
//!
//! Three structural rules + six length checks. Per r7 design:
//! "Constrain only what the system needs to act on." Anything more
//! (slug validation on `kind`, reserved-key blacklist, schema
//! versioning, urgency enums) is deferred until the loop runs once
//! and reveals the need.
//!
//! JSON Schema `maxLength` in the MCP tool spec is informational for
//! the client. Serde does NOT enforce it on the server, so the
//! length checks below are load-bearing.

use std::collections::HashSet;
use std::fmt;

use crate::decision::types::DecisionPayload;

/// Caps. Single source of truth — must match the MCP tool's
/// `inputSchema.maxLength` declarations.
pub const MAX_HEADLINE: usize = 80;
pub const MAX_QUESTION: usize = 120;
pub const MAX_CONTEXT: usize = 4096;
pub const MAX_OPTION_KEY: usize = 2;
pub const MAX_OPTION_LABEL: usize = 40;
pub const MAX_OPTION_BODY: usize = 2048;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// `options.len()` is outside `2..=6`.
    OptionCount { got: usize },
    /// Two or more options share the same `key`.
    DuplicateOptionKeys,
    /// `recommended_key` doesn't match any option's `key`.
    RecommendedKeyMissing { recommended_key: String },
    /// A length cap was exceeded.
    TooLong {
        field: &'static str,
        got: usize,
        max: usize,
    },
    /// Option key length is outside `1..=2`.
    OptionKeyLength { key: String, got: usize },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::OptionCount { got } => write!(
                f,
                "options must have between {MIN_OPTIONS} and {MAX_OPTIONS} entries; got {got}"
            ),
            ValidationError::DuplicateOptionKeys => {
                write!(f, "option keys must be unique within a decision")
            }
            ValidationError::RecommendedKeyMissing { recommended_key } => write!(
                f,
                "recommended_key '{recommended_key}' does not match any option's key"
            ),
            ValidationError::TooLong { field, got, max } => {
                write!(f, "{field} too long: {got} chars (max {max})")
            }
            ValidationError::OptionKeyLength { key, got } => write!(
                f,
                "option key '{key}' has invalid length {got} (must be 1 or 2 characters)"
            ),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate a `DecisionPayload`. Returns the first error encountered.
///
/// Order: structural rules first (cardinality, uniqueness, recommendation
/// reference) then length caps. Cardinality and uniqueness are cheap
/// invariants the system relies on for picking semantics; length caps
/// only matter for storage and rendering.
pub fn validate(p: &DecisionPayload) -> Result<(), ValidationError> {
    // Structural rule 1: option count.
    let n = p.options.len();
    if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&n) {
        return Err(ValidationError::OptionCount { got: n });
    }

    // Structural rule 2: option keys unique.
    let unique_keys: HashSet<&String> = p.options.iter().map(|o| &o.key).collect();
    if unique_keys.len() != p.options.len() {
        return Err(ValidationError::DuplicateOptionKeys);
    }

    // Structural rule 3: recommended_key references a real option.
    if !p.options.iter().any(|o| o.key == p.recommended_key) {
        return Err(ValidationError::RecommendedKeyMissing {
            recommended_key: p.recommended_key.clone(),
        });
    }

    // Length cap: headline.
    if p.headline.len() > MAX_HEADLINE {
        return Err(ValidationError::TooLong {
            field: "headline",
            got: p.headline.len(),
            max: MAX_HEADLINE,
        });
    }

    // Length cap: question.
    if p.question.len() > MAX_QUESTION {
        return Err(ValidationError::TooLong {
            field: "question",
            got: p.question.len(),
            max: MAX_QUESTION,
        });
    }

    // Length cap: context.
    if p.context.len() > MAX_CONTEXT {
        return Err(ValidationError::TooLong {
            field: "context",
            got: p.context.len(),
            max: MAX_CONTEXT,
        });
    }

    // Length caps: per-option key, label, body.
    for o in &p.options {
        let key_len = o.key.len();
        if !(1..=MAX_OPTION_KEY).contains(&key_len) {
            return Err(ValidationError::OptionKeyLength {
                key: o.key.clone(),
                got: key_len,
            });
        }
        if o.label.len() > MAX_OPTION_LABEL {
            return Err(ValidationError::TooLong {
                field: "option label",
                got: o.label.len(),
                max: MAX_OPTION_LABEL,
            });
        }
        if o.body.len() > MAX_OPTION_BODY {
            return Err(ValidationError::TooLong {
                field: "option body",
                got: o.body.len(),
                max: MAX_OPTION_BODY,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::types::{DecisionPayload, OptionPayload};

    fn ok_payload() -> DecisionPayload {
        DecisionPayload {
            headline: "ok".into(),
            question: "?".into(),
            options: vec![
                OptionPayload {
                    key: "a".into(),
                    label: "A".into(),
                    body: "do A".into(),
                },
                OptionPayload {
                    key: "b".into(),
                    label: "B".into(),
                    body: "do B".into(),
                },
            ],
            recommended_key: "a".into(),
            context: String::new(),
        }
    }

    #[test]
    fn baseline_payload_is_valid() {
        validate(&ok_payload()).expect("baseline should validate");
    }

    #[test]
    fn rejects_one_option() {
        let mut p = ok_payload();
        p.options.truncate(1);
        match validate(&p) {
            Err(ValidationError::OptionCount { got: 1 }) => {}
            other => panic!("expected OptionCount{{1}}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_seven_options() {
        let mut p = ok_payload();
        let template = p.options[0].clone();
        while p.options.len() < 7 {
            p.options.push(OptionPayload {
                key: format!("{}{}", template.key, p.options.len()),
                ..template.clone()
            });
        }
        match validate(&p) {
            Err(ValidationError::OptionCount { got: 7 }) => {}
            other => panic!("expected OptionCount{{7}}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_duplicate_keys() {
        let mut p = ok_payload();
        p.options[1].key = "a".into();
        assert_eq!(validate(&p), Err(ValidationError::DuplicateOptionKeys));
    }

    #[test]
    fn rejects_recommended_key_missing() {
        let mut p = ok_payload();
        p.recommended_key = "z".into();
        match validate(&p) {
            Err(ValidationError::RecommendedKeyMissing { recommended_key }) => {
                assert_eq!(recommended_key, "z");
            }
            other => panic!("expected RecommendedKeyMissing, got {other:?}"),
        }
    }

    #[test]
    fn rejects_headline_too_long() {
        let mut p = ok_payload();
        p.headline = "x".repeat(MAX_HEADLINE + 1);
        match validate(&p) {
            Err(ValidationError::TooLong { field, got, max }) => {
                assert_eq!(field, "headline");
                assert_eq!(got, MAX_HEADLINE + 1);
                assert_eq!(max, MAX_HEADLINE);
            }
            other => panic!("expected TooLong{{headline}}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_question_too_long() {
        let mut p = ok_payload();
        p.question = "x".repeat(MAX_QUESTION + 1);
        match validate(&p) {
            Err(ValidationError::TooLong { field, .. }) if field == "question" => {}
            other => panic!("expected TooLong{{question}}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_context_too_long() {
        let mut p = ok_payload();
        p.context = "x".repeat(MAX_CONTEXT + 1);
        match validate(&p) {
            Err(ValidationError::TooLong { field, .. }) if field == "context" => {}
            other => panic!("expected TooLong{{context}}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_zero_length_option_key() {
        // Set recommended_key to "" too so the recommended_key check
        // passes and we exercise the per-option key-length check below.
        let mut p = ok_payload();
        p.options[0].key = String::new();
        p.recommended_key = String::new();
        match validate(&p) {
            Err(ValidationError::OptionKeyLength { got: 0, .. }) => {}
            other => panic!("expected OptionKeyLength{{0}}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_three_char_option_key() {
        let mut p = ok_payload();
        p.options[0].key = "abc".into();
        p.recommended_key = "abc".into();
        match validate(&p) {
            Err(ValidationError::OptionKeyLength { got: 3, .. }) => {}
            other => panic!("expected OptionKeyLength{{3}}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_option_label_too_long() {
        let mut p = ok_payload();
        p.options[0].label = "x".repeat(MAX_OPTION_LABEL + 1);
        match validate(&p) {
            Err(ValidationError::TooLong { field, .. }) if field == "option label" => {}
            other => panic!("expected TooLong{{option label}}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_option_body_too_long() {
        let mut p = ok_payload();
        p.options[0].body = "x".repeat(MAX_OPTION_BODY + 1);
        match validate(&p) {
            Err(ValidationError::TooLong { field, .. }) if field == "option body" => {}
            other => panic!("expected TooLong{{option body}}, got {other:?}"),
        }
    }
}

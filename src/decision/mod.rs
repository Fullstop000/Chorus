//! Decision-inbox subsystem.
//!
//! Lifecycle: agent calls `chorus_create_decision(payload)` → server
//! validates and inserts a row → tool returns `decision_id` → agent
//! ends turn cleanly. When the human picks via the UI, the resolve
//! handler does a CAS update and resumes the agent's runtime session
//! with a self-contained envelope as the new turn prompt.
//!
//! v1 scope is the minimum mechanism. See the r7 design doc in
//! `chorus-design-reviews/explorations/2026-04-30-pr-review-vertical-slice/`.
//!
//! Day 1 ships types + validator + tests. Day 3 adds the storage
//! handlers and the `resume_with_prompt` lifecycle method.

pub mod types;
pub mod validator;

pub use types::{Decision, DecisionPayload, OptionPayload, ResolvePayload, Status};
pub use validator::{validate, ValidationError};

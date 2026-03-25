use crate::store::teams::TeamMembership;

/// Pluggable coordination protocol for a team.
pub trait CollaborationModel: Send + Sync {
    /// Role-specific instructions injected into a member's system prompt section.
    fn member_role_prompt(&self, role: &str) -> String;

    /// System message posted into the team channel when a forwarded task arrives.
    /// Returns None if no deliberation phase (e.g. Leader+Operators).
    fn deliberation_prompt(&self) -> Option<String>;

    /// Returns true if the message content is a consensus signal (e.g. starts with "READY:").
    fn is_consensus_signal(&self, content: &str) -> bool;
}

/// Factory: returns the right CollaborationModel for the stored string.
/// Unknown values fall back to LeaderOperators.
pub fn make_collaboration_model(model: &str) -> Box<dyn CollaborationModel> {
    match model {
        "swarm" => Box::new(Swarm),
        _ => Box::new(LeaderOperators),
    }
}

// ── Leader + Operators ──

/// Collaboration model where one leader decomposes tasks and delegates to operators.
/// No deliberation phase — the leader acts immediately on task arrival.
pub struct LeaderOperators;

impl CollaborationModel for LeaderOperators {
    fn member_role_prompt(&self, role: &str) -> String {
        match role {
            "leader" => {
                "You are the **leader** of this team. When a task arrives:\n\
                 1. Decompose it into subtasks.\n\
                 2. Delegate each subtask to an operator via DM or a channel message.\n\
                 3. Synthesize the results and post a summary back to the channel where the task originated.\n\
                 Do not execute subtasks yourself — delegate and coordinate.".to_string()
            }
            _ => {
                "You are an **operator** in this team. Wait for task delegation from the leader. \
                 Execute your assigned subtask and report back to the leader when done.".to_string()
            }
        }
    }

    fn deliberation_prompt(&self) -> Option<String> {
        None
    }

    fn is_consensus_signal(&self, _content: &str) -> bool {
        false
    }
}

// ── Swarm ──

/// Collaboration model where all members deliberate before executing.
/// Each member posts `READY: <subtask>` to signal readiness; the system posts a GO message once all are ready.
pub struct Swarm;

impl CollaborationModel for Swarm {
    fn member_role_prompt(&self, _role: &str) -> String {
        "You are a **swarm member** of this team. When a task arrives:\n\
         1. Read the task and discuss the best approach with your teammates in the channel.\n\
         2. When you have decided what your part of the work is, post a message starting with \
            `READY: ` followed by a brief description of your assigned subtask.\n\
         3. Once all members have posted READY:, the system will confirm and you should begin your subtask.\n\
         Do not start work before the system posts the GO message.".to_string()
    }

    fn deliberation_prompt(&self) -> Option<String> {
        Some(
            "**New task received.** Discuss the best approach with your teammates. \
             When you are ready to proceed, reply with `READY: <brief description of your assigned subtask>`. \
             Execution will begin once all members have confirmed.".to_string(),
        )
    }

    fn is_consensus_signal(&self, content: &str) -> bool {
        content.trim_start().starts_with("READY:")
    }
}

/// Build the `## Your Teams` section for an agent's system prompt.
pub fn build_teams_prompt_section(memberships: &[TeamMembership]) -> String {
    if memberships.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = memberships
        .iter()
        .map(|m| format!("- #{} — role: {}", m.team_name, m.role))
        .collect();
    format!("## Your Teams\n{}\n", lines.join("\n"))
}

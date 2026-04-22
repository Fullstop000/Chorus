use crate::agent::AgentRuntime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeMetadata {
    pub label: &'static str,
    pub order: u32,
    pub reasoning_efforts: &'static [&'static str],
}

const EMPTY_REASONING_EFFORTS: &[&str] = &[];
// TODO: Claude and Codex are advertised as supporting reasoning efforts here,
// but their drivers do not yet plumb `AgentSpec.reasoning_effort` into the
// actual runtime requests. Keep this catalog aligned with driver behavior.
const CLAUDE_REASONING_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

const CODEX_REASONING_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];

const CLAUDE_METADATA: RuntimeMetadata = RuntimeMetadata {
    label: "Claude Code",
    order: 0,
    reasoning_efforts: CLAUDE_REASONING_EFFORTS,
};

const CODEX_METADATA: RuntimeMetadata = RuntimeMetadata {
    label: "Codex CLI",
    order: 1,
    reasoning_efforts: CODEX_REASONING_EFFORTS,
};

const KIMI_METADATA: RuntimeMetadata = RuntimeMetadata {
    label: "Kimi CLI",
    order: 2,
    reasoning_efforts: EMPTY_REASONING_EFFORTS,
};

const OPENCODE_METADATA: RuntimeMetadata = RuntimeMetadata {
    label: "OpenCode",
    order: 3,
    reasoning_efforts: EMPTY_REASONING_EFFORTS,
};

const GEMINI_METADATA: RuntimeMetadata = RuntimeMetadata {
    label: "Gemini CLI",
    order: 4,
    reasoning_efforts: EMPTY_REASONING_EFFORTS,
};

pub const fn runtime_metadata(runtime: AgentRuntime) -> &'static RuntimeMetadata {
    match runtime {
        AgentRuntime::Claude => &CLAUDE_METADATA,
        AgentRuntime::Codex => &CODEX_METADATA,
        AgentRuntime::Kimi => &KIMI_METADATA,
        AgentRuntime::Opencode => &OPENCODE_METADATA,
        AgentRuntime::Gemini => &GEMINI_METADATA,
    }
}

pub const fn supports_reasoning_effort(runtime: AgentRuntime) -> bool {
    !runtime_metadata(runtime).reasoning_efforts.is_empty()
}

pub fn supports_reasoning_effort_value(runtime: AgentRuntime, value: &str) -> bool {
    runtime_metadata(runtime).reasoning_efforts.contains(&value)
}

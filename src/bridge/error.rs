//! Structured error codes for the MCP bridge.
//!
//! `ChorusBackend` returns `Result<_, BridgeError>`; `ChatBridge` converts to
//! `rmcp::ErrorData` at the MCP handler boundary via `From<BridgeError>`.
//! This gives agents structured error codes (CHORUS-XXXX) for programmatic
//! handling while preserving the JSON-RPC error contract toward MCP clients.
//!
//! See `docs/BRIDGE_MIGRATION.md` for the full phased migration.

use std::fmt;

/// Structured error codes for the MCP bridge.
/// Each variant includes a human-readable message with cause and suggested fix.
#[derive(Debug, Clone)]
pub enum BridgeError {
// Note: std::error::Error is not derived because the variants use named fields
// incompatible with thiserror's #[error] derive. Manual Display impl below.
// Add `impl std::error::Error for BridgeError {}` if needed for anyhow compatibility.
    /// Platform (Chorus server) is unreachable — server not running or network failure.
    PlatformUnreachable { url: String, cause: String },

    /// The agent key is not registered in the session registry.
    AgentNotFound { agent_key: String },

    /// The MCP session has expired (TTL exceeded).
    SessionExpired { session_id: String },

    /// Invalid channel/DM target format.
    InvalidTarget { target: String, hint: String },

    /// File upload failed.
    UploadFailed { cause: String },

    /// Attachment not found or inaccessible.
    AttachmentNotFound { attachment_id: String },

    /// Invalid input parameter (e.g., path traversal attempt).
    InvalidParam { param: String, reason: String },

    /// Server returned an HTTP error.
    ServerError { status: u16, body: String },
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::PlatformUnreachable { url, cause } => write!(
                f,
                "CHORUS-4001: Platform unreachable at {url} ({cause}) — is 'chorus serve' running?"
            ),
            BridgeError::AgentNotFound { agent_key } => write!(
                f,
                "CHORUS-4002: Agent '{agent_key}' not found — check agent name or register via 'chorus agent create'"
            ),
            BridgeError::SessionExpired { session_id } => write!(
                f,
                "CHORUS-4003: Session '{session_id}' has expired — reconnect to start a new session"
            ),
            BridgeError::InvalidTarget { target, hint } => write!(
                f,
                "CHORUS-4004: Invalid target '{target}' — {hint}"
            ),
            BridgeError::UploadFailed { cause } => write!(
                f,
                "CHORUS-4005: File upload failed ({cause}) — check file path and permissions"
            ),
            BridgeError::AttachmentNotFound { attachment_id } => write!(
                f,
                "CHORUS-4006: Attachment '{attachment_id}' not found — verify the attachment ID is correct"
            ),
            BridgeError::InvalidParam { param, reason } => write!(
                f,
                "CHORUS-4007: Invalid parameter '{param}': {reason}"
            ),
            BridgeError::ServerError { status, body } => write!(
                f,
                "CHORUS-5001: Server error ({status}): {body} — check server logs"
            ),
        }
    }
}

impl std::error::Error for BridgeError {}

impl From<BridgeError> for rmcp::ErrorData {
    fn from(err: BridgeError) -> rmcp::ErrorData {
        let message = err.to_string();
        match err {
            BridgeError::PlatformUnreachable { .. } | BridgeError::ServerError { .. } => {
                rmcp::ErrorData::internal_error(message, None)
            }
            BridgeError::AgentNotFound { .. } | BridgeError::SessionExpired { .. } => {
                rmcp::ErrorData::invalid_request(message, None)
            }
            BridgeError::InvalidTarget { .. } | BridgeError::InvalidParam { .. } => {
                rmcp::ErrorData::invalid_params(message, None)
            }
            BridgeError::UploadFailed { .. } | BridgeError::AttachmentNotFound { .. } => {
                rmcp::ErrorData::internal_error(message, None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Display: verify CHORUS-XXXX code prefix
    // -----------------------------------------------------------------------

    #[test]
    fn platform_unreachable_display() {
        let err = BridgeError::PlatformUnreachable {
            url: "http://localhost:3001".to_string(),
            cause: "connection refused".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.starts_with("CHORUS-4001:"), "got: {msg}");
        assert!(msg.contains("http://localhost:3001"), "got: {msg}");
        assert!(msg.contains("connection refused"), "got: {msg}");
        assert!(msg.contains("chorus serve"), "got: {msg}");
    }

    #[test]
    fn agent_not_found_display() {
        let err = BridgeError::AgentNotFound {
            agent_key: "bot-1".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.starts_with("CHORUS-4002:"), "got: {msg}");
        assert!(msg.contains("bot-1"), "got: {msg}");
        assert!(msg.contains("chorus agent create"), "got: {msg}");
    }

    #[test]
    fn session_expired_display() {
        let err = BridgeError::SessionExpired {
            session_id: "sess-abc".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.starts_with("CHORUS-4003:"), "got: {msg}");
        assert!(msg.contains("sess-abc"), "got: {msg}");
        assert!(msg.contains("reconnect"), "got: {msg}");
    }

    #[test]
    fn invalid_target_display() {
        let err = BridgeError::InvalidTarget {
            target: "bad-target".to_string(),
            hint: "use '#channel' or 'dm:@name'".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.starts_with("CHORUS-4004:"), "got: {msg}");
        assert!(msg.contains("bad-target"), "got: {msg}");
        assert!(msg.contains("use '#channel' or 'dm:@name'"), "got: {msg}");
    }

    #[test]
    fn upload_failed_display() {
        let err = BridgeError::UploadFailed {
            cause: "permission denied".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.starts_with("CHORUS-4005:"), "got: {msg}");
        assert!(msg.contains("permission denied"), "got: {msg}");
        assert!(msg.contains("permissions"), "got: {msg}");
    }

    #[test]
    fn attachment_not_found_display() {
        let err = BridgeError::AttachmentNotFound {
            attachment_id: "att-xyz".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.starts_with("CHORUS-4006:"), "got: {msg}");
        assert!(msg.contains("att-xyz"), "got: {msg}");
    }

    #[test]
    fn invalid_param_display() {
        let err = BridgeError::InvalidParam {
            param: "file_path".to_string(),
            reason: "path traversal not allowed".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.starts_with("CHORUS-4007:"), "got: {msg}");
        assert!(msg.contains("file_path"), "got: {msg}");
        assert!(msg.contains("path traversal not allowed"), "got: {msg}");
    }

    #[test]
    fn server_error_display() {
        let err = BridgeError::ServerError {
            status: 500,
            body: "internal error".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.starts_with("CHORUS-5001:"), "got: {msg}");
        assert!(msg.contains("500"), "got: {msg}");
        assert!(msg.contains("internal error"), "got: {msg}");
        assert!(msg.contains("server logs"), "got: {msg}");
    }

    // -----------------------------------------------------------------------
    // ErrorData conversion: verify the right error kind is produced
    // -----------------------------------------------------------------------

    fn error_code(err: BridgeError) -> i32 {
        let data: rmcp::ErrorData = err.into();
        data.code.0
    }

    // JSON-RPC standard codes:
    //   INVALID_REQUEST  = -32600
    //   INVALID_PARAMS   = -32602
    //   INTERNAL_ERROR   = -32603

    #[test]
    fn platform_unreachable_maps_to_internal_error() {
        let code = error_code(BridgeError::PlatformUnreachable {
            url: "http://localhost:3001".to_string(),
            cause: "refused".to_string(),
        });
        assert_eq!(code, -32603);
    }

    #[test]
    fn server_error_maps_to_internal_error() {
        let code = error_code(BridgeError::ServerError {
            status: 502,
            body: "bad gateway".to_string(),
        });
        assert_eq!(code, -32603);
    }

    #[test]
    fn agent_not_found_maps_to_invalid_request() {
        let code = error_code(BridgeError::AgentNotFound {
            agent_key: "bot-x".to_string(),
        });
        assert_eq!(code, -32600);
    }

    #[test]
    fn session_expired_maps_to_invalid_request() {
        let code = error_code(BridgeError::SessionExpired {
            session_id: "s1".to_string(),
        });
        assert_eq!(code, -32600);
    }

    #[test]
    fn invalid_target_maps_to_invalid_params() {
        let code = error_code(BridgeError::InvalidTarget {
            target: "??".to_string(),
            hint: "use #channel".to_string(),
        });
        assert_eq!(code, -32602);
    }

    #[test]
    fn invalid_param_maps_to_invalid_params() {
        let code = error_code(BridgeError::InvalidParam {
            param: "x".to_string(),
            reason: "bad".to_string(),
        });
        assert_eq!(code, -32602);
    }

    #[test]
    fn upload_failed_maps_to_internal_error() {
        let code = error_code(BridgeError::UploadFailed {
            cause: "disk full".to_string(),
        });
        assert_eq!(code, -32603);
    }

    #[test]
    fn attachment_not_found_maps_to_internal_error() {
        let code = error_code(BridgeError::AttachmentNotFound {
            attachment_id: "a1".to_string(),
        });
        assert_eq!(code, -32603);
    }

    // -----------------------------------------------------------------------
    // Error messages include cause and fix hint
    // -----------------------------------------------------------------------

    #[test]
    fn error_data_message_contains_chorus_code() {
        let data: rmcp::ErrorData = BridgeError::PlatformUnreachable {
            url: "http://localhost:3001".to_string(),
            cause: "timeout".to_string(),
        }
        .into();
        assert!(data.message.contains("CHORUS-4001"), "got: {}", data.message);
        assert!(data.message.contains("timeout"), "got: {}", data.message);
    }
}

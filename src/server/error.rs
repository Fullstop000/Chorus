use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

// ── Wire type ──────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<AppErrorCode>,
}

/// Shorthand result type used by every handler.
pub type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

// ── Typed error codes ──────────────────────────────────────────────────────────

/// Typed error codes that unlock distinct frontend UX behavior.
///
/// Only errors that enable a *different* UI action than status + message alone
/// earn a code. Everything else uses `None` (omitted from JSON).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AppErrorCode {
    /// Generic fatal failure fallback — red toast.
    InternalError,
    /// Agent name already in use — keep create modal open, inline field error.
    AgentNameTaken,
    /// Channel name already in use — keep create modal open, inline field error.
    ChannelNameTaken,
    /// Team name already in use — keep create modal open, inline field error.
    TeamNameTaken,
    /// Agent restart failed — keep restart modal, show retry + "check logs".
    AgentRestartFailed,
    /// Agent deleted but workspace cleanup failed — partial success, warning toast.
    AgentDeleteWorkspaceCleanupFailed,
    /// DM/system/team channel hit unsupported endpoint — disable affordance.
    ChannelOperationUnsupported,
    /// Sender is not a channel member — show "join channel" guidance.
    MessageNotAMember,
}

impl AppErrorCode {
    pub fn http_status(&self) -> StatusCode {
        match self {
            Self::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            Self::AgentNameTaken => StatusCode::CONFLICT,
            Self::ChannelNameTaken => StatusCode::CONFLICT,
            Self::TeamNameTaken => StatusCode::CONFLICT,
            Self::AgentRestartFailed => StatusCode::INTERNAL_SERVER_ERROR,
            Self::AgentDeleteWorkspaceCleanupFailed => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ChannelOperationUnsupported => StatusCode::BAD_REQUEST,
            Self::MessageNotAMember => StatusCode::FORBIDDEN,
        }
    }
}

// ── ErrKind trait + app_err ────────────────────────────────────────────────────

/// Anything that can serve as the "kind" argument to [`app_err`].
///
/// - `StatusCode` — plain HTTP error, no typed code in the JSON body.
/// - `AppErrorCode` — typed error; status is derived from the code automatically.
pub trait ErrKind {
    fn status(&self) -> StatusCode;
    fn code(&self) -> Option<AppErrorCode>;
}

impl ErrKind for StatusCode {
    fn status(&self) -> StatusCode {
        *self
    }
    fn code(&self) -> Option<AppErrorCode> {
        None
    }
}

impl ErrKind for AppErrorCode {
    fn status(&self) -> StatusCode {
        self.http_status()
    }
    fn code(&self) -> Option<AppErrorCode> {
        Some(*self)
    }
}

/// Internal implementation — call the [`app_err!`] macro instead.
#[doc(hidden)]
pub(crate) fn app_err_inner(
    kind: impl ErrKind,
    msg: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        kind.status(),
        Json(ErrorResponse {
            error: msg.into(),
            code: kind.code(),
        }),
    )
}

/// Build a handler error response.
///
/// Two forms:
/// - `app_err!(kind, "plain message")`
/// - `app_err!(kind, "value is {}", x)` — args are passed to [`format!`]
///
/// A string literal without extra args is also routed through [`format!`],
/// so `app_err!(kind, "{x}")` works for implicit display capture.
///
/// `kind` is either a [`StatusCode`] (no `code` field in JSON) or an
/// [`AppErrorCode`] (status derived automatically, `code` field included).
macro_rules! app_err {
    // Literal format string with optional extra args.
    ($kind:expr, $fmt:literal $($arg:tt)*) => {
        crate::server::error::app_err_inner($kind, ::std::format!($fmt $($arg)*))
    };
    // Owned string or any other expression.
    ($kind:expr, $msg:expr) => {
        crate::server::error::app_err_inner($kind, $msg)
    };
}
pub(crate) use app_err;

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AppErrorCode::http_status ──

    #[test]
    fn internal_error_maps_to_500() {
        assert_eq!(
            AppErrorCode::InternalError.http_status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn name_taken_codes_map_to_409() {
        assert_eq!(
            AppErrorCode::AgentNameTaken.http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AppErrorCode::ChannelNameTaken.http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AppErrorCode::TeamNameTaken.http_status(),
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn restart_failed_maps_to_500() {
        assert_eq!(
            AppErrorCode::AgentRestartFailed.http_status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn workspace_cleanup_failed_maps_to_500() {
        assert_eq!(
            AppErrorCode::AgentDeleteWorkspaceCleanupFailed.http_status(),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }

    #[test]
    fn unsupported_channel_op_maps_to_400() {
        assert_eq!(
            AppErrorCode::ChannelOperationUnsupported.http_status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn not_a_member_maps_to_403() {
        assert_eq!(
            AppErrorCode::MessageNotAMember.http_status(),
            StatusCode::FORBIDDEN
        );
    }

    // ── ErrKind dispatch ──

    #[test]
    fn status_code_errkind_passes_through_status_and_no_code() {
        let (status, Json(body)) = app_err!(StatusCode::BAD_REQUEST, "bad input");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body.error, "bad input");
        assert!(body.code.is_none());
    }

    #[test]
    fn app_error_code_errkind_derives_status_and_sets_code() {
        let (status, Json(body)) = app_err!(AppErrorCode::AgentNameTaken, "already taken");
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.error, "already taken");
        assert!(matches!(body.code, Some(AppErrorCode::AgentNameTaken)));
    }

    #[test]
    fn macro_interpolates_format_args() {
        let id = 99u32;
        let (_, Json(body)) = app_err!(StatusCode::NOT_FOUND, "item {} not found", id);
        assert_eq!(body.error, "item 99 not found");
    }

    #[test]
    fn macro_supports_implicit_display_capture() {
        let name = "alice";
        let (_, Json(body)) = app_err!(StatusCode::BAD_REQUEST, "{name} is invalid");
        assert_eq!(body.error, "alice is invalid");
    }

    #[test]
    fn macro_passes_owned_string_expression_through() {
        let msg = String::from("already taken");
        let (_, Json(body)) = app_err!(StatusCode::CONFLICT, msg);
        assert_eq!(body.error, "already taken");
    }

    // ── JSON serialisation ──

    #[test]
    fn error_response_without_code_omits_code_field() {
        let resp = ErrorResponse {
            error: "oops".into(),
            code: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"], "oops");
        assert!(json.get("code").is_none(), "code must be omitted when None");
    }

    #[test]
    fn error_response_with_code_serialises_screaming_snake() {
        let resp = ErrorResponse {
            error: "conflict".into(),
            code: Some(AppErrorCode::ChannelNameTaken),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["code"], "CHANNEL_NAME_TAKEN");
    }

    #[test]
    fn all_codes_round_trip_through_json() {
        let codes = [
            AppErrorCode::InternalError,
            AppErrorCode::AgentNameTaken,
            AppErrorCode::ChannelNameTaken,
            AppErrorCode::TeamNameTaken,
            AppErrorCode::AgentRestartFailed,
            AppErrorCode::AgentDeleteWorkspaceCleanupFailed,
            AppErrorCode::ChannelOperationUnsupported,
            AppErrorCode::MessageNotAMember,
        ];
        for code in codes {
            let serialised = serde_json::to_string(&code).unwrap();
            let deserialised: AppErrorCode = serde_json::from_str(&serialised).unwrap();
            // Compare via serialised form since AppErrorCode doesn't derive PartialEq.
            assert_eq!(
                serde_json::to_string(&deserialised).unwrap(),
                serialised,
                "round-trip failed for {serialised}",
            );
        }
    }
}

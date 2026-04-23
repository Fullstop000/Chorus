//! `chorus channel del <name>` — delete a channel, with a TTY-gated confirmation.
//!
//! The prompt-read logic is factored into a pure function [`confirm_delete`] so
//! it can be unit-tested without touching real stdin/stdout. The caller passes a
//! `BufRead` and a `is_tty` hint; the function returns an outcome enum.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::Context;

/// Outcome of a confirmation prompt for `channel del`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfirmOutcome {
    Proceed,
    Abort,
    RefuseNonInteractive,
}

/// Read a yes/no decision for deleting `_name`.
///
/// When `is_tty` is false, returns [`ConfirmOutcome::RefuseNonInteractive`]
/// without consuming any input. When `is_tty` is true, reads a single line:
///   - `"y"` or `"Y"` (optionally with a trailing newline) → [`ConfirmOutcome::Proceed`]
///   - anything else (including empty, `"n"`, `"N"`) → [`ConfirmOutcome::Abort`]
pub(crate) fn confirm_delete<R: BufRead>(
    reader: &mut R,
    is_tty: bool,
    _name: &str,
) -> ConfirmOutcome {
    if !is_tty {
        return ConfirmOutcome::RefuseNonInteractive;
    }
    let mut line = String::new();
    // An error or EOF is treated as Abort (default No) — we don't want to
    // destroy data on a stdin that closed unexpectedly.
    if reader.read_line(&mut line).is_err() {
        return ConfirmOutcome::Abort;
    }
    let trimmed = line.trim_end_matches(['\r', '\n']);
    match trimmed {
        "y" | "Y" => ConfirmOutcome::Proceed,
        _ => ConfirmOutcome::Abort,
    }
}

pub async fn run(name: String, yes: bool, server_url: &str) -> anyhow::Result<()> {
    let normalized = super::normalize_channel_name(&name);

    if !yes {
        // Prompt on stderr so stdout stays clean for scripting; `eprint!` keeps
        // the cursor on the same line as the user's response.
        eprint!("Delete #{normalized}? [y/N] ");
        std::io::stderr().flush().ok();
        let stdin = std::io::stdin();
        let is_tty = stdin.is_terminal();
        let outcome = {
            let mut locked = stdin.lock();
            confirm_delete(&mut locked, is_tty, &normalized)
        };
        match outcome {
            ConfirmOutcome::Proceed => {}
            ConfirmOutcome::Abort => {
                tracing::info!("Aborted.");
                return Ok(());
            }
            ConfirmOutcome::RefuseNonInteractive => {
                return Err(super::super::UserError(format!(
                    "refusing to delete #{normalized} without --yes on non-interactive stdin"
                )).into());
            }
        }
    }

    let client = super::http::client();
    let id = super::resolve_channel_id(&client, server_url, &normalized).await?;
    let url = format!("{server_url}/api/channels/{id}");
    let res = client
        .delete(&url)
        .send()
        .await
        .with_context(|| format!("is the Chorus server running at {server_url}?"))?;
    let status = res.status();
    if status.is_success() {
        tracing::info!("Channel #{normalized} deleted.");
        return Ok(());
    }
    let body = res.text().await.unwrap_or_default();
    Err(super::surface_http_error(status, &body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn tty_y_proceeds() {
        let mut r = Cursor::new(b"y\n".to_vec());
        assert_eq!(confirm_delete(&mut r, true, "x"), ConfirmOutcome::Proceed);
    }

    #[test]
    fn tty_uppercase_y_proceeds() {
        let mut r = Cursor::new(b"Y\n".to_vec());
        assert_eq!(confirm_delete(&mut r, true, "x"), ConfirmOutcome::Proceed);
    }

    #[test]
    fn tty_n_aborts() {
        let mut r = Cursor::new(b"n\n".to_vec());
        assert_eq!(confirm_delete(&mut r, true, "x"), ConfirmOutcome::Abort);
    }

    #[test]
    fn tty_empty_aborts() {
        let mut r = Cursor::new(b"\n".to_vec());
        assert_eq!(confirm_delete(&mut r, true, "x"), ConfirmOutcome::Abort);
    }

    #[test]
    fn non_tty_refuses() {
        let mut r = Cursor::new(b"y\n".to_vec());
        assert_eq!(
            confirm_delete(&mut r, false, "x"),
            ConfirmOutcome::RefuseNonInteractive
        );
        // The reader must not have been consumed — the original bytes remain.
        let mut leftover = String::new();
        r.read_to_string(&mut leftover).unwrap();
        assert_eq!(leftover, "y\n");
    }
}

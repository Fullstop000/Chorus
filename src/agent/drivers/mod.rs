pub mod v2;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::Context;

pub(crate) fn command_exists(command: &str) -> bool {
    is_executable_in_dirs(command, &process_path_dirs())
        || is_executable_in_dirs(command, user_shell_path_dirs())
}

fn is_executable_in_dirs(command: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().map(|dir| dir.join(command)).any(|candidate| {
        fs::metadata(&candidate)
            .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
            .unwrap_or(false)
    })
}

fn process_path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// Resolve the user's interactive shell PATH once and cache it.
/// Handles any node/tool version manager (nvm, volta, fnm, etc.) that
/// hooks into the shell init files rather than the system PATH.
static USER_SHELL_PATH_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

fn user_shell_path_dirs() -> &'static [PathBuf] {
    USER_SHELL_PATH_DIRS.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Command::new(&shell)
            .args(["-i", "-c", "echo $PATH"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| std::env::split_paths(s.trim()).collect())
            .unwrap_or_default()
    })
}

pub(crate) fn run_command(program: &str, args: &[&str]) -> anyhow::Result<CommandProbeResult> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program} {}", args.join(" ")))?;
    Ok(CommandProbeResult {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub(crate) fn read_file(path: &Path) -> anyhow::Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandProbeResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

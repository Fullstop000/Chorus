//! `chorus setup` — first-run doctor that detects runtimes, ACP adaptors,
//! and agent templates, then writes a `config.toml` to the data directory.
//!
//! Runs interactively when stdin is a TTY (prompts for data / template dirs,
//! lets the user pick between duplicate binaries on `$PATH`). Passes `--yes`
//! to skip all prompts and accept defaults.

use chorus::config::ChorusConfig;
use chorus::store::Store;
use console::{style, Emoji};
use std::io::IsTerminal;
use std::process::Command;
use std::time::{Duration, Instant};

use super::{default_data_dir, DATA_SUBDIR, DEFAULT_TEMPLATE_DIR};

// Glyphs. `console::Emoji` falls back to ASCII on dumb terminals.
static OK: Emoji<'_, '_> = Emoji("✓ ", "ok ");
static BAD: Emoji<'_, '_> = Emoji("✗ ", "x  ");
static WARN: Emoji<'_, '_> = Emoji("⚠ ", "!  ");

fn banner() {
    // Render visible content for each inner row at a fixed width, then apply
    // ANSI styling on top (styling adds bytes but no visible columns).
    const INNER: usize = 41;
    let dashes = "─".repeat(INNER);
    let row1_plain = format!(
        "{:<width$}",
        " Chorus · local AI agent platform",
        width = INNER
    );
    let row1_styled = row1_plain
        .replacen("Chorus", &style("Chorus").bold().cyan().to_string(), 1)
        .replacen(
            "· local AI agent platform",
            &style("· local AI agent platform").dim().to_string(),
            1,
        );
    let row2_styled = style(format!("{:<width$}", " first-run setup", width = INNER))
        .dim()
        .to_string();
    let bar = style("│").dim();
    println!();
    println!("  {}", style(format!("┌{}┐", dashes)).dim());
    println!("  {}{}{}", bar, row1_styled, bar);
    println!("  {}{}{}", bar, row2_styled, bar);
    println!("  {}", style(format!("└{}┘", dashes)).dim());
    println!();
}

fn section(title: &str) {
    println!();
    println!("  {}", style(title).bold());
}

fn row_ok(name: &str, detail: &str) {
    println!(
        "  {}{:<12} {}",
        style(OK).green(),
        style(name).bold(),
        style(detail).dim()
    );
}

fn row_warn(name: &str, detail: &str) {
    println!(
        "  {}{:<12} {}",
        style(WARN).yellow(),
        style(name).bold(),
        style(detail).dim()
    );
}

fn row_info(label: &str, value: &str) {
    println!("  {:<12} {}", style(label).dim(), value);
}

fn footer(elapsed: Duration, next: &str) {
    println!();
    println!("  {}", style("─".repeat(41)).dim());
    println!(
        "  All set in {}. Next:",
        style(format!("{:.1}s", elapsed.as_secs_f64())).bold()
    );
    println!("    {} {}", style("$").dim(), style(next).cyan().bold());
    println!();
}

/// Extract the first dotted version number from a tool's `--version` output,
/// so we show "1.3.12" instead of "kimi, version 1.31.0".
fn extract_version(s: &str) -> Option<String> {
    static VERSION_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = VERSION_RE
        .get_or_init(|| regex::Regex::new(r"\b\d+\.\d+(?:\.\d+)?(?:[-+][\w.]+)?\b").unwrap());
    re.find(s).map(|m| m.as_str().to_string())
}

/// Resolve an executable's absolute path by walking `$PATH`, the same way
/// `which <name>` does. Returns `None` if the binary isn't found.
fn which_tool(name: &str) -> Option<std::path::PathBuf> {
    which_all(name).into_iter().next()
}

/// Return every absolute path where `name` is found on `$PATH`, deduped,
/// in discovery order. Empty vec if nothing found.
fn which_all(name: &str) -> Vec<std::path::PathBuf> {
    which_all_in(name, std::env::var_os("PATH").as_deref())
}

/// Like `which_all` but searches an explicit PATH value (an `OsStr` in
/// standard PATH format). Accepts `None` and returns an empty vec.
/// Separated from `which_all` so tests can inject a controlled PATH without
/// mutating the process-wide environment variable.
fn which_all_in(name: &str, path_var: Option<&std::ffi::OsStr>) -> Vec<std::path::PathBuf> {
    let Some(path) = path_var else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .filter(|p| p.is_file())
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Fill `target` with the resolved absolute path for `name` iff `target`
/// is currently unset or empty. Preserves any non-empty user-pinned value
/// across re-runs. `Some("")` is treated as unset to handle legacy configs
/// that stored an empty string instead of omitting the field.
/// Always uses the first match; intended for ACP adapters where a silent
/// choice is fine.
fn fill_resolved_path(target: &mut Option<String>, name: &str) {
    if target.as_deref().unwrap_or("").is_empty() {
        if let Some(p) = which_tool(name) {
            *target = Some(p.to_string_lossy().into_owned());
        }
    }
}

/// Like `fill_resolved_path`, but when multiple matches exist and we're in
/// interactive mode, ask the user to pick one. Non-interactive mode falls
/// back to the first match (current behavior).
fn fill_binary_path(target: &mut Option<String>, name: &str, interactive: bool) {
    // Treat Some("") as unset — normalizes legacy String-based configs.
    if !target.as_deref().unwrap_or("").is_empty() {
        return; // user-pinned, preserve
    }
    let candidates = which_all(name);
    let chosen = match candidates.len() {
        0 => None,
        1 => candidates.into_iter().next(),
        _ if !interactive => candidates.into_iter().next(),
        _ => {
            use dialoguer::theme::ColorfulTheme;
            use dialoguer::Select;
            let labels: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
            let idx = Select::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Multiple `{name}` binaries on PATH; pick one"))
                .items(&labels)
                .default(0)
                .interact()
                .unwrap_or(0);
            candidates.into_iter().nth(idx)
        }
    };
    if let Some(p) = chosen {
        *target = Some(p.to_string_lossy().into_owned());
    }
}

/// Run `<name> --version` and return the extracted dotted version, or `None`
/// if the binary is missing or the command fails. Some tools print their
/// version to stderr (historically `python --version` did), so we fall
/// back to stderr if stdout is empty.
fn check_tool(name: &str) -> Option<String> {
    let output = Command::new(name)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let source = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    extract_version(&source).or_else(|| {
        source
            .lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// What kind of ACP support a runtime has.
enum AcpStatus {
    /// External adaptor binary is on PATH.
    AdapterFound(&'static str),
    /// External adaptor binary is missing; chorus will fall back to raw mode.
    AdapterMissing(&'static str),
    /// Runtime provides its own `acp` subcommand; nothing to install.
    Native,
}

struct RuntimeReport {
    name: &'static str,
    hint: &'static str,
    version: Option<String>,
    acp: AcpStatus,
}

fn check_runtime(name: &'static str, hint: &'static str, acp: AcpStatus) -> RuntimeReport {
    let version = check_tool(name);
    // If an external adaptor is expected, re-resolve at check time so PATH
    // changes between test runs are reflected.
    let acp = match acp {
        AcpStatus::AdapterFound(bin) | AcpStatus::AdapterMissing(bin) => {
            if Command::new(bin)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                AcpStatus::AdapterFound(bin)
            } else {
                AcpStatus::AdapterMissing(bin)
            }
        }
        AcpStatus::Native => AcpStatus::Native,
    };
    RuntimeReport {
        name,
        hint,
        version,
        acp,
    }
}

fn render_runtime(r: &RuntimeReport) {
    let (glyph, glyph_style): (Emoji<'_, '_>, _) = match &r.version {
        Some(_) => (OK, "green"),
        None => (BAD, "red"),
    };
    let glyph_styled = match glyph_style {
        "green" => style(glyph).green(),
        _ => style(glyph).red(),
    };
    let name = style(format!("{:<12}", r.name)).bold();
    let version = match &r.version {
        Some(v) => style(format!("{:<10}", v)).dim().to_string(),
        None => style(format!("{:<10}", "not found")).dim().to_string(),
    };
    let acp_detail = match (&r.version, &r.acp) {
        (None, _) => style(format!("install: {}", r.hint))
            .dim()
            .italic()
            .to_string(),
        (Some(_), AcpStatus::AdapterFound(bin)) => {
            format!(
                "{} {} {}",
                style("·").dim(),
                style(bin).cyan(),
                style("found").dim()
            )
        }
        (Some(_), AcpStatus::AdapterMissing(bin)) => {
            format!(
                "{} {} {} {}",
                style("·").dim(),
                style(bin).yellow(),
                style("missing").yellow(),
                style("→ raw mode").dim()
            )
        }
        (Some(_), AcpStatus::Native) => {
            format!("{} {}", style("·").dim(), style("native ACP").dim())
        }
    };
    println!("  {}{} {} {}", glyph_styled, name, version, acp_detail);
}

fn check_template_dir(dir: &std::path::Path) -> (usize, usize) {
    if !dir.is_dir() {
        return (0, 0);
    }
    let mut templates = 0usize;
    let mut categories = 0usize;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (0, 0),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let mut has_md = false;
            if let Ok(sub) = std::fs::read_dir(&path) {
                for s in sub.flatten() {
                    if s.path().extension().and_then(|e| e.to_str()) == Some("md") {
                        templates += 1;
                        has_md = true;
                    }
                }
            }
            if has_md {
                categories += 1;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            templates += 1;
        }
    }
    (templates, categories)
}

/// Reconcile older installs with the current layout:
///   root → <root>/data/   : chorus.db*, attachments/, teams/
///   <root>/data/ → root   : agents/  (an earlier commit mistakenly moved it)
/// Only moves when the source exists and the target doesn't — idempotent,
/// refuses to clobber.
fn migrate_legacy_layout(
    root: &std::path::Path,
    data_subdir: &std::path::Path,
) -> anyhow::Result<()> {
    // Things that live under <root>/data/ going forward.
    let into_data = [
        "chorus.db",
        "chorus.db-wal",
        "chorus.db-shm",
        "attachments",
        "teams",
    ];
    for name in into_data {
        let src = root.join(name);
        let dst = data_subdir.join(name);
        if src.exists() && !dst.exists() {
            tracing::info!(from = %src.display(), to = %dst.display(), "migrating legacy layout");
            std::fs::rename(&src, &dst)?;
        }
    }
    // Undo the misplacement of agents/ by pulling it back to the root.
    let stray = data_subdir.join("agents");
    let home = root.join("agents");
    if stray.exists() && !home.exists() {
        tracing::info!(from = %stray.display(), to = %home.display(), "restoring agents/ to root");
        std::fs::rename(&stray, &home)?;
    }
    Ok(())
}

pub async fn run(
    yes: bool,
    data_dir: Option<String>,
    template_dir_cli: Option<String>,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let interactive = !yes && std::io::stdin().is_terminal();

    // Resolve the data dir early so we can check for an existing config.
    let data_dir_str_early = data_dir
        .clone()
        .unwrap_or_else(default_data_dir);
    let data_dir_early = chorus::agent::templates::expand_tilde(&data_dir_str_early);
    let config_path = data_dir_early.join("config.toml");

    // Skip setup when config already exists and the caller didn't explicitly
    // force it (e.g. via --yes in an automated context). This makes
    // `chorus setup` safe to call from scripts without wrapping it in a
    // `[ ! -f config.toml ]` guard.
    if config_path.exists() && !yes {
        println!(
            "  {} config already exists at {}  (re-run with --yes to overwrite)",
            style(OK).green(),
            style(config_path.display()).cyan()
        );
        return Ok(());
    }

    if !yes && !interactive {
        println!(
            "  {} stdin is not a terminal; running in non-interactive mode.",
            style(WARN).yellow()
        );
    }

    banner();

    // Data dir: respect --data-dir if given, otherwise prompt (with default)
    // when interactive, or silently use the default when not.
    let data_dir_str = match data_dir {
        Some(s) => s,
        None if interactive => {
            use dialoguer::theme::ColorfulTheme;
            use dialoguer::Input;
            Input::<String>::with_theme(&ColorfulTheme::default())
                .with_prompt("Data directory")
                .default(default_data_dir())
                .interact_text()
                .unwrap_or_else(|_| default_data_dir())
        }
        None => default_data_dir(),
    };
    let data_dir = chorus::agent::templates::expand_tilde(&data_dir_str);

    // Template dir precedence: CLI flag > existing config > default.
    // Setup always writes the result back so `chorus start` picks it up later.
    let template_dir_raw = template_dir_cli
        .or_else(|| {
            ChorusConfig::load(&data_dir)
                .ok()
                .flatten()
                .and_then(|c| c.agent_template.dir)
        })
        .unwrap_or_else(|| DEFAULT_TEMPLATE_DIR.to_string());
    let template_dir = chorus::agent::templates::expand_tilde(&template_dir_raw);

    row_info("Data dir", &style(data_dir.display()).cyan().to_string());
    row_info(
        "Templates",
        &style(template_dir.display()).cyan().to_string(),
    );

    // Runtimes + their ACP adaptor status
    section("Runtimes");
    let runtimes = [
        check_runtime(
            "claude",
            "https://docs.claude.com/en/docs/claude-code",
            AcpStatus::AdapterMissing("claude-agent-acp"),
        ),
        check_runtime(
            "codex",
            "https://github.com/openai/codex",
            AcpStatus::AdapterMissing("codex-acp"),
        ),
        check_runtime(
            "kimi",
            "https://github.com/MoonshotAI/kimi-cli",
            AcpStatus::Native,
        ),
        check_runtime("opencode", "https://opencode.ai", AcpStatus::Native),
    ];
    for r in &runtimes {
        render_runtime(r);
    }
    let any_adapter_missing = runtimes
        .iter()
        .any(|r| r.version.is_some() && matches!(r.acp, AcpStatus::AdapterMissing(_)));
    if any_adapter_missing {
        println!(
            "  {} {} {}",
            style(" ").dim(),
            style("ACP adaptors:").dim(),
            style("https://github.com/openclaw/acpx").dim().italic()
        );
    }
    let detected_runtimes: Vec<&str> = runtimes
        .iter()
        .filter(|r| r.version.is_some())
        .map(|r| r.name)
        .collect();

    // 3. Templates
    section("Templates");
    let (tmpl_count, tmpl_cats) = check_template_dir(&template_dir);
    if !template_dir.exists() {
        row_warn(
            "templates",
            &format!(
                "{} not found · starter gallery will be empty",
                template_dir.display()
            ),
        );
    } else if tmpl_count == 0 {
        row_warn(
            "templates",
            &format!(
                "{} exists but contains no .md files",
                template_dir.display()
            ),
        );
    } else {
        row_ok(
            "templates",
            &format!(
                "{} templates across {} categor{}",
                tmpl_count,
                tmpl_cats,
                if tmpl_cats == 1 { "y" } else { "ies" }
            ),
        );
    }

    // Ensure the directory layout exists and migrate any old-layout files
    // that were created before `data/` and `logs/` became first-class.
    //   <root>/config.toml           (config — stays at root)
    //   <root>/logs/                 (new: log files)
    //   <root>/data/                 (new: all data)
    //       chorus.db*, agents/, attachments/, teams/
    let data_subdir = data_dir.join(DATA_SUBDIR);
    let logs_dir = data_dir.join("logs");
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&data_subdir)?;
    std::fs::create_dir_all(&logs_dir)?;
    // Migrate BEFORE creating an empty root `agents/`, otherwise the
    // reverse move would see the target already exists and skip.
    migrate_legacy_layout(&data_dir, &data_subdir)?;
    std::fs::create_dir_all(&agents_dir)?;

    // Always call Store::open: it runs migrations idempotently, so an
    // existing chorus.db gets schema upgrades as part of setup.
    let db_path = data_subdir.join("chorus.db");
    let _ = Store::open(db_path.to_str().unwrap())?;

    // Persist config — machine_id (stable across re-runs) + template_dir,
    // so `chorus start` can read the chosen paths without the user re-passing
    // --template-dir every time.
    let mut cfg = ChorusConfig::load(&data_dir)?.unwrap_or_default();
    let machine_id = cfg.ensure_machine_id().to_string();
    cfg.agent_template.dir = Some(template_dir_raw.clone());

    // Pin runtime binaries to the exact paths detected on this machine,
    // but don't overwrite anything the user has already customized. When
    // a CLI binary shows up in multiple PATH entries (e.g. ~/.local/bin
    // AND /usr/local/bin), prompt interactively. ACP adapters always use
    // the first match — they're less likely to ship multiple versions.
    fill_binary_path(&mut cfg.claude.binary_path, "claude", interactive);
    fill_resolved_path(&mut cfg.claude.acp_adaptor, "claude-agent-acp");
    fill_binary_path(&mut cfg.codex.binary_path, "codex", interactive);
    fill_resolved_path(&mut cfg.codex.acp_adaptor, "codex-acp");
    fill_binary_path(&mut cfg.kimi.binary_path, "kimi", interactive);
    fill_binary_path(&mut cfg.opencode.binary_path, "opencode", interactive);

    let cfg_path = cfg.save(&data_dir)?;

    section("Layout");
    row_ok("config", &format!("wrote {}", cfg_path.display()));
    row_ok("data", &format!("{}", data_subdir.display()));
    row_ok("logs", &format!("{}", logs_dir.display()));
    row_ok("agents", &format!("{}", agents_dir.display()));
    row_ok(
        "machine id",
        &format!("{} (persistent)", style(&machine_id).cyan()),
    );

    // Summary line
    println!();
    if detected_runtimes.is_empty() {
        println!(
            "  {} no agent runtimes detected · install one, then re-run setup",
            style(WARN).yellow()
        );
    } else {
        println!(
            "  {} runtimes available: {}",
            style("→").cyan().bold(),
            style(detected_runtimes.join(", ")).bold()
        );
        println!(
            "  {} {}",
            style(" ").dim(),
            style("chorus agent create <name> --runtime <runtime>").dim()
        );
    }

    footer(started.elapsed(), "chorus start");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_tool_returns_none_for_missing_binary() {
        assert!(check_tool("definitely-not-a-real-binary-xyzzy").is_none());
    }

    #[test]
    fn extract_version_handles_common_formats() {
        assert_eq!(extract_version("bun 1.3.12"), Some("1.3.12".to_string()));
        assert_eq!(
            extract_version("kimi, version 1.31.0"),
            Some("1.31.0".to_string())
        );
        assert_eq!(
            extract_version("codex-cli 0.120.0"),
            Some("0.120.0".to_string())
        );
        assert_eq!(extract_version("no version here"), None);
    }

    #[test]
    fn which_all_finds_every_match_across_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_a = tmp.path().join("a");
        let dir_b = tmp.path().join("b");
        let dir_c = tmp.path().join("c");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::create_dir_all(&dir_c).unwrap();
        // Two copies of a fake binary, different dirs.
        for d in [&dir_a, &dir_c] {
            let p = d.join("myfake-bin");
            std::fs::write(&p, "#!/bin/sh\ntrue\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&p).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&p, perms).unwrap();
            }
        }

        // Inject PATH directly — no global env mutation, no flakiness.
        let joined = std::env::join_paths([&dir_a, &dir_b, &dir_c]).unwrap();
        let found = which_all_in("myfake-bin", Some(&joined));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], dir_a.join("myfake-bin"));
        assert_eq!(found[1], dir_c.join("myfake-bin"));
    }
}

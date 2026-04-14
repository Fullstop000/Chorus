//! On-disk configuration for Chorus.
//!
//! Lives at `<data_dir>/config.toml`. Written by `chorus setup` after the
//! user picks their data and template directories; read by `chorus start`
//! and `chorus serve` so a user who chose a non-default location once
//! doesn't have to keep passing flags forever.
//!
//! Precedence for any one setting:
//!   1. CLI flag (if the user set it explicitly)
//!   2. This config file
//!   3. Environment variable (where applicable)
//!   4. Hard-coded default
//!
//! `data_dir` is not stored here because the config lives inside it — the
//! directory you're reading from IS the data dir.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "config.toml";

/// Per-runtime configuration. All fields are optional:
/// - `None` = not configured / not applicable (use PATH / not available).
///
/// `acp_adaptor` is `Some` only for runtimes that ship a separate ACP
/// adapter binary (claude → `claude-agent-acp`, codex → `codex-acp`).
/// Native-ACP runtimes (kimi, opencode) leave it `None`.
/// Empty strings (`Some("")`) from legacy configs are normalized to `None`
/// by `ChorusConfig::load`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRuntimeConfig {
    /// Absolute path to the runtime CLI binary. `None` = discover via PATH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,

    /// Absolute path to the ACP adapter binary. `None` = not applicable or
    /// discover via PATH. Only set for ACP-adapter runtimes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_adaptor: Option<String>,
}

/// Agent-template-related settings. Groups the markdown-template directory
/// and the default template id under one section so they live together.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTemplateConfig {
    /// Directory holding agent-template markdown files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,

    /// Default template id (e.g. `engineering/backend-architect`) used
    /// when `chorus agent create` runs without `--template`. Empty string
    /// = no default; user is prompted or create fails if nothing else
    /// resolves.
    #[serde(default)]
    pub default: String,
}

/// File-logging settings. Applied by `chorus serve` / `chorus start` to
/// write structured logs into `<data_dir>/logs/` alongside stdout output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogsConfig {
    /// Tracing-subscriber directive string (same syntax as `RUST_LOG`).
    /// Default `chorus=info`.
    #[serde(default = "LogsConfig::default_level")]
    pub level: String,

    /// Rotation cadence: `"daily"` | `"hourly"` | `"never"`.
    #[serde(default = "LogsConfig::default_rotation")]
    pub rotation: String,

    /// Max number of rotated log files to keep. Older files are deleted
    /// as new ones are written. Only meaningful when `rotation` != "never".
    #[serde(default = "LogsConfig::default_retention")]
    pub retention: u32,
}

impl LogsConfig {
    fn default_level() -> String {
        "chorus=info".into()
    }
    fn default_rotation() -> String {
        // `never` = one `chorus.log` that grows indefinitely. Set to
        // `daily` or `hourly` in config.toml to enable date-stamped
        // rotation with retention.
        "never".into()
    }
    fn default_retention() -> u32 {
        14
    }
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            level: Self::default_level(),
            rotation: Self::default_rotation(),
            retention: Self::default_retention(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChorusConfig {
    /// Stable random identifier for this installation. Generated at first
    /// setup, preserved across upgrades. Used by agent-data and telemetry
    /// to attribute records to a specific machine/install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,

    #[serde(default)]
    pub agent_template: AgentTemplateConfig,

    #[serde(default)]
    pub logs: LogsConfig,

    #[serde(default)]
    pub claude: AgentRuntimeConfig,
    #[serde(default)]
    pub codex: AgentRuntimeConfig,
    #[serde(default)]
    pub kimi: AgentRuntimeConfig,
    #[serde(default)]
    pub opencode: AgentRuntimeConfig,
}

impl ChorusConfig {
    pub fn path_for(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Return the machine id, generating and persisting one if missing.
    /// Fresh installs end up with a UUID v4 on their first call here.
    pub fn ensure_machine_id(&mut self) -> &str {
        if self.machine_id.is_none() {
            self.machine_id = Some(uuid::Uuid::new_v4().to_string());
        }
        self.machine_id.as_deref().unwrap()
    }

    /// Load from `<data_dir>/config.toml`. Returns `Ok(None)` if the file
    /// doesn't exist; `Err` only on I/O or parse failures.
    pub fn load(data_dir: &Path) -> anyhow::Result<Option<Self>> {
        let path = Self::path_for(data_dir);
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
        let mut cfg: Self = toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
        // Normalize legacy empty strings to None so callers can use Option semantics.
        for runtime in [
            &mut cfg.claude,
            &mut cfg.codex,
            &mut cfg.kimi,
            &mut cfg.opencode,
        ] {
            if runtime.binary_path.as_deref() == Some("") {
                runtime.binary_path = None;
            }
            if runtime.acp_adaptor.as_deref() == Some("") {
                runtime.acp_adaptor = None;
            }
        }
        Ok(Some(cfg))
    }

    /// Write atomically (write-to-temp + rename) so an interrupted setup
    /// never leaves a half-written config behind.
    pub fn save(&self, data_dir: &Path) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(data_dir)?;
        let path = Self::path_for(data_dir);
        let tmp = path.with_extension("toml.tmp");
        let body = format!(
            "# Chorus config — auto-generated by `chorus setup`.\n\
             # Edit by hand or re-run setup to regenerate.\n\n\
             {}",
            toml::to_string_pretty(self)?
        );
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ChorusConfig::load(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn save_then_load_roundtrips_template_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ChorusConfig {
            agent_template: AgentTemplateConfig {
                dir: Some("/opt/templates".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let written = cfg.save(tmp.path()).unwrap();
        assert!(written.exists());
        let loaded = ChorusConfig::load(tmp.path()).unwrap().unwrap();
        assert_eq!(loaded.agent_template.dir.as_deref(), Some("/opt/templates"));
    }

    #[test]
    fn save_omits_none_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ChorusConfig::default();
        cfg.save(tmp.path()).unwrap();
        let raw = std::fs::read_to_string(ChorusConfig::path_for(tmp.path())).unwrap();
        // Optional fields should not render when None / default.
        assert!(!raw.contains("dir ="));
        assert!(!raw.contains("machine_id"));
    }

    #[test]
    fn ensure_machine_id_is_stable() {
        let mut cfg = ChorusConfig::default();
        let first = cfg.ensure_machine_id().to_string();
        let second = cfg.ensure_machine_id().to_string();
        assert_eq!(first, second);
        assert!(uuid::Uuid::parse_str(&first).is_ok());
    }

    #[test]
    fn ensure_machine_id_preserves_existing() {
        let mut cfg = ChorusConfig {
            machine_id: Some("custom-id".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.ensure_machine_id(), "custom-id");
    }

    #[test]
    fn save_emits_all_runtime_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ChorusConfig::default();
        cfg.save(tmp.path()).unwrap();
        let raw = std::fs::read_to_string(ChorusConfig::path_for(tmp.path())).unwrap();
        // All four runtime sections are present.
        for runtime in ["claude", "codex", "kimi", "opencode"] {
            // With all-None fields, toml serializes empty tables as `[runtime]`
            // only when the section is non-empty. The sections are emitted because
            // the struct fields themselves are always present in ChorusConfig.
            // Verify the section key exists in any form.
            assert!(
                raw.contains(&format!("[{runtime}]")),
                "missing [{runtime}] section in:\n{raw}"
            );
        }
    }

    #[test]
    fn roundtrip_preserves_runtime_binary_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ChorusConfig {
            claude: AgentRuntimeConfig {
                binary_path: Some("/opt/claude".into()),
                acp_adaptor: Some("/opt/claude-agent-acp".into()),
            },
            kimi: AgentRuntimeConfig {
                binary_path: Some("/opt/kimi".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.save(tmp.path()).unwrap();
        let loaded = ChorusConfig::load(tmp.path()).unwrap().unwrap();
        assert_eq!(loaded.claude.binary_path.as_deref(), Some("/opt/claude"));
        assert_eq!(
            loaded.claude.acp_adaptor.as_deref(),
            Some("/opt/claude-agent-acp")
        );
        assert_eq!(loaded.kimi.binary_path.as_deref(), Some("/opt/kimi"));
        assert_eq!(loaded.codex.binary_path, None); // untouched default
    }

    #[test]
    fn roundtrip_preserves_agent_template_default() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ChorusConfig {
            agent_template: AgentTemplateConfig {
                default: "engineering/backend-architect".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.save(tmp.path()).unwrap();
        let loaded = ChorusConfig::load(tmp.path()).unwrap().unwrap();
        assert_eq!(
            loaded.agent_template.default,
            "engineering/backend-architect"
        );
    }

    #[test]
    fn load_normalizes_legacy_empty_string_paths_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        // Write config as it would look from the old String-based schema.
        let toml_body = "[claude]\nbinary_path = \"\"\nacp_adaptor = \"\"\n\n[codex]\nbinary_path = \"/opt/codex\"\n";
        std::fs::write(ChorusConfig::path_for(tmp.path()), toml_body).unwrap();
        let loaded = ChorusConfig::load(tmp.path()).unwrap().unwrap();
        // Empty strings must be normalized to None.
        assert_eq!(loaded.claude.binary_path, None);
        assert_eq!(loaded.claude.acp_adaptor, None);
        // Non-empty values are preserved.
        assert_eq!(loaded.codex.binary_path.as_deref(), Some("/opt/codex"));
    }
}

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

const MAX_TEMPLATE_FILE_SIZE: u64 = 100 * 1024; // 100KB

/// A parsed agent template from a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    /// Stable id: `{category}/{filestem}`, e.g. `engineering/backend-architect`.
    pub id: String,
    /// Human-readable name from frontmatter.
    pub name: String,
    /// Emoji icon from frontmatter.
    pub emoji: Option<String>,
    /// Color accent from frontmatter.
    pub color: Option<String>,
    /// One-line personality summary from frontmatter.
    pub vibe: Option<String>,
    /// Brief description from frontmatter (for card display).
    pub description: Option<String>,
    /// Category derived from parent directory name.
    pub category: String,
    /// Suggested runtime driver (defaults to "claude").
    pub suggested_runtime: String,
    /// Full markdown body (the system prompt for the agent).
    pub prompt_body: String,
}

/// Templates grouped by category for the API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateCategory {
    pub name: String,
    pub templates: Vec<AgentTemplate>,
}

/// YAML frontmatter fields parsed from the template file.
#[derive(Debug, Deserialize)]
struct TemplateFrontmatter {
    name: Option<String>,
    description: Option<String>,
    color: Option<String>,
    emoji: Option<String>,
    vibe: Option<String>,
    suggested_runtime: Option<String>,
}

/// Expand `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

/// Load all templates from the given directory.
///
/// Each subdirectory is treated as a category. Files in the root are categorized
/// as "other". Malformed files are skipped with a warning.
pub fn load_templates(template_dir: &Path) -> Vec<AgentTemplate> {
    if !template_dir.is_dir() {
        warn!(
            path = %template_dir.display(),
            "template directory not found, no templates loaded"
        );
        return Vec::new();
    }

    let mut templates = Vec::new();

    let entries = match std::fs::read_dir(template_dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!(path = %template_dir.display(), error = %e, "failed to read template directory");
            return Vec::new();
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let category = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("other")
                .to_string();
            load_category(&path, &category, &mut templates);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            if let Some(t) = parse_template_file(&path, "other") {
                templates.push(t);
            }
        }
    }

    info!(
        count = templates.len(),
        path = %template_dir.display(),
        "loaded agent templates"
    );
    templates
}

/// Group a flat list of templates into categories, sorted by category then name.
pub fn group_by_category(templates: &[AgentTemplate]) -> Vec<TemplateCategory> {
    let mut by_category: BTreeMap<String, Vec<AgentTemplate>> = BTreeMap::new();
    for t in templates {
        by_category
            .entry(t.category.clone())
            .or_default()
            .push(t.clone());
    }
    for templates in by_category.values_mut() {
        templates.sort_by(|a, b| a.name.cmp(&b.name));
    }
    by_category
        .into_iter()
        .map(|(name, templates)| TemplateCategory { name, templates })
        .collect()
}

fn load_category(dir: &Path, category: &str, templates: &mut Vec<AgentTemplate>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!(path = %dir.display(), error = %e, "failed to read category directory");
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            if let Some(t) = parse_template_file(&path, category) {
                templates.push(t);
            }
        }
    }
}

fn parse_template_file(path: &Path, category: &str) -> Option<AgentTemplate> {
    // Check file size.
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_TEMPLATE_FILE_SIZE {
        warn!(path = %path.display(), size = metadata.len(), "template file too large, skipping");
        return None;
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to read template file");
            return None;
        }
    };

    // Split frontmatter from body. Frontmatter is between first and second `---`.
    let (frontmatter_str, body) = split_frontmatter(&content)?;

    let fm: TemplateFrontmatter = match serde_yaml::from_str(frontmatter_str) {
        Ok(fm) => fm,
        Err(e) => {
            debug!(path = %path.display(), error = %e, "failed to parse YAML frontmatter");
            return None;
        }
    };

    let name = fm.name.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string()
    });

    // Build filestem: strip category prefix if present (e.g. "engineering-backend-architect" -> "backend-architect").
    let raw_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed");
    let filestem = raw_stem
        .strip_prefix(&format!("{category}-"))
        .unwrap_or(raw_stem);
    let id = format!("{category}/{filestem}");

    let suggested_runtime = fm.suggested_runtime.unwrap_or_else(|| "claude".to_string());

    Some(AgentTemplate {
        id,
        name,
        emoji: fm.emoji,
        color: fm.color,
        vibe: fm.vibe,
        description: fm.description,
        category: category.to_string(),
        suggested_runtime,
        prompt_body: body.trim().to_string(),
    })
}

/// Split a markdown file into YAML frontmatter and body.
/// Returns `None` if frontmatter delimiters are not found.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    // Find the closing --- (skip the first one).
    let after_first = &trimmed[3..];
    let closing = after_first.find("\n---")?;
    let frontmatter = &after_first[..closing].trim();
    let body = &after_first[closing + 4..]; // skip "\n---"
    Some((frontmatter, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_correctly() {
        let content = "---\nname: Test\n---\n\nBody here";
        let (fm, body) = split_frontmatter(content).unwrap();
        assert_eq!(fm, "name: Test");
        assert_eq!(body.trim(), "Body here");
    }

    #[test]
    fn returns_none_for_no_frontmatter() {
        assert!(split_frontmatter("No frontmatter here").is_none());
    }

    #[test]
    fn strips_category_prefix_from_filestem() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("engineering-backend-architect.md");
        std::fs::write(
            &file,
            "---\nname: Backend Architect\n---\nYou are a backend architect.",
        )
        .unwrap();
        let t = parse_template_file(&file, "engineering").unwrap();
        assert_eq!(t.id, "engineering/backend-architect");
        assert_eq!(t.name, "Backend Architect");
        assert_eq!(t.prompt_body, "You are a backend architect.");
    }

    #[test]
    fn defaults_suggested_runtime_to_claude() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.md");
        std::fs::write(&file, "---\nname: Test\n---\nBody").unwrap();
        let t = parse_template_file(&file, "other").unwrap();
        assert_eq!(t.suggested_runtime, "claude");
    }

    #[test]
    fn skips_file_over_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.md");
        let content = format!("---\nname: Big\n---\n{}", "x".repeat(101 * 1024));
        std::fs::write(&file, content).unwrap();
        assert!(parse_template_file(&file, "other").is_none());
    }

    #[test]
    fn expand_tilde_works() {
        let expanded = expand_tilde("~/agency-agents");
        if dirs::home_dir().is_some() {
            assert!(!expanded.to_str().unwrap().starts_with('~'));
        } else {
            assert_eq!(expanded, PathBuf::from("~/agency-agents"));
        }
    }

    #[test]
    fn load_templates_returns_empty_for_missing_dir() {
        let templates = load_templates(Path::new("/nonexistent/path"));
        assert!(templates.is_empty());
    }
}

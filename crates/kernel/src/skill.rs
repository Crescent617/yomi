use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A loaded skill with metadata and content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    #[serde(skip)]
    pub source_path: PathBuf,
}

/// Frontmatter metadata for a skill
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    triggers: Vec<String>,
}

/// Skill loader that scans directories for SKILL.md files
#[derive(Debug, Clone)]
pub struct SkillLoader {
    folders: Vec<PathBuf>,
}

impl SkillLoader {
    pub const fn new(folders: Vec<PathBuf>) -> Self {
        Self { folders }
    }

    /// Load all skills from configured folders
    pub fn load_all(&self) -> Result<Vec<Arc<Skill>>> {
        let mut skills = Vec::new();

        for folder in &self.folders {
            if folder.exists() {
                self.load_from_folder(folder, &mut skills)
                    .with_context(|| format!("Failed to load skills from {}", folder.display()))?;
            } else {
                tracing::warn!("Skill folder does not exist: {}", folder.display());
            }
        }
        // if name conflicts, keep the first one found and log a warning
        let mut seen_names = std::collections::HashSet::new();
        skills.retain(|skill| {
            if seen_names.contains(&skill.name) {
                tracing::warn!(
                    "Duplicate skill name '{}' found in {}. Ignoring this instance.",
                    skill.name,
                    skill.source_path.display()
                );
                false
            } else {
                seen_names.insert(skill.name.clone());
                true
            }
        });
        Ok(skills)
    }

    /// Load skills from a single folder (recursively)
    #[allow(clippy::only_used_in_recursion)]
    fn load_from_folder(&self, folder: &Path, skills: &mut Vec<Arc<Skill>>) -> Result<()> {
        for entry in std::fs::read_dir(folder)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.load_from_folder(&path, skills)?;
            } else if path.is_file() {
                let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

                if file_name.ends_with("SKILL.md") {
                    match Self::load_skill(&path) {
                        Ok(skill) => {
                            tracing::debug!(
                                "Loaded skill '{}' from {}",
                                skill.name,
                                path.display()
                            );
                            skills.push(Arc::new(skill));
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load skill from {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Load a single skill from a file
    /// Only reads the frontmatter portion for efficiency
    fn load_skill(path: &Path) -> Result<Skill> {
        use std::io::{BufRead, BufReader};

        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open skill file: {}", path.display()))?;
        let reader = BufReader::new(file);

        let mut lines = reader.lines();

        // Check if file starts with ---
        let first_line = lines.next().transpose()?;
        if first_line.as_deref() != Some("---") {
            anyhow::bail!("Skill file must start with frontmatter delimiter ---");
        }

        // Collect frontmatter lines until second ---
        let mut yaml_lines = Vec::new();
        let mut found_end = false;

        for line in lines {
            let line = line?;
            if line == "---" {
                found_end = true;
                break;
            }
            yaml_lines.push(line);
        }

        if !found_end {
            anyhow::bail!("Frontmatter end delimiter not found");
        }

        // Parse just the frontmatter YAML
        let yaml_content = yaml_lines.join("\n");
        let frontmatter: SkillFrontmatter = serde_yaml::from_str(&yaml_content)
            .context("Failed to parse skill frontmatter YAML")?;

        Ok(Skill {
            name: frontmatter.name,
            description: frontmatter.description,
            triggers: frontmatter.triggers,
            source_path: path.to_path_buf(),
        })
    }
}

/// Parse YAML frontmatter from markdown content
///
/// Expects format:
/// ```text
/// ---
/// key: value
/// ---
/// content
/// ```
#[allow(dead_code)]
fn parse_frontmatter(content: &str) -> Result<(SkillFrontmatter, &str)> {
    // Check if content starts with ---
    if !content.trim_start().starts_with("---") {
        // No frontmatter, return default
        let frontmatter = SkillFrontmatter {
            name: "unnamed".to_string(),
            description: String::new(),
            triggers: Vec::new(),
        };
        return Ok((frontmatter, content));
    }

    // Find the end of frontmatter (second ---)
    let after_first_delim = &content[content.find("---").unwrap() + 3..];
    let Some(end_pos) = after_first_delim.find("---") else {
        anyhow::bail!("Frontmatter end delimiter not found");
    };

    let yaml_content = &after_first_delim[..end_pos];
    let body = &after_first_delim[end_pos + 3..];

    // Parse YAML
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(yaml_content).context("Failed to parse skill frontmatter YAML")?;

    Ok((frontmatter, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_with_all_fields() {
        let content = r#"---
name: test-skill
description: A test skill
triggers:
  - trigger1
  - trigger2
---

# Skill Content

This is the skill content."#;

        let (frontmatter, body) = parse_frontmatter(content).unwrap();

        assert_eq!(frontmatter.name, "test-skill");
        assert_eq!(frontmatter.description, "A test skill");
        assert_eq!(frontmatter.triggers, vec!["trigger1", "trigger2"]);
        assert!(body.contains("# Skill Content"));
    }

    #[test]
    fn test_parse_frontmatter_minimal() {
        let content = r#"---
name: minimal-skill
---

Just content."#;

        let (frontmatter, body) = parse_frontmatter(content).unwrap();

        assert_eq!(frontmatter.name, "minimal-skill");
        assert_eq!(frontmatter.description, "");
        assert!(frontmatter.triggers.is_empty());
        assert_eq!(body.trim(), "Just content.");
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = "Just plain content without frontmatter.";

        let (frontmatter, body) = parse_frontmatter(content).unwrap();

        assert_eq!(frontmatter.name, "unnamed");
        assert_eq!(body, content);
    }
}

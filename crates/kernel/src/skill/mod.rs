use crate::types::{KernelError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A loaded skill with metadata and content
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    /// Raw hooks value from frontmatter (parsed later into `HookRegistry`)
    pub hooks: Option<serde_yaml::Value>,
    pub source_path: PathBuf,
}

/// Frontmatter metadata for a skill
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    /// Name is kept for backwards compatibility but no longer used.
    /// Skill name is now derived from the file path.
    #[allow(dead_code)]
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    triggers: Vec<String>,
    /// Optional hooks declaration (YAML array string or inline table array)
    #[serde(default)]
    hooks: Option<serde_yaml::Value>,
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
                Self::load_from_folder(folder, &mut skills).map_err(|e| {
                    KernelError::skill(format!(
                        "Failed to load skills from {}: {e}",
                        folder.display()
                    ))
                })?;
            } else {
                tracing::warn!("Skill folder does not exist: {}", folder.display());
            }
        }
        Ok(skills)
    }

    /// Load skills from a single folder (recursively)
    fn load_from_folder(folder: &Path, skills: &mut Vec<Arc<Skill>>) -> Result<()> {
        Self::load_from_folder_recursive(folder, folder, skills)
    }

    /// Recursively load skills, tracking the root folder for name derivation
    fn load_from_folder_recursive(
        root_folder: &Path,
        current_folder: &Path,
        skills: &mut Vec<Arc<Skill>>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(current_folder)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                Self::load_from_folder_recursive(root_folder, &path, skills)?;
            } else if path.is_file() {
                let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

                if file_name.ends_with("SKILL.md") {
                    match Self::load_skill(&path, root_folder) {
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

    /// Parse the YAML frontmatter from a SKILL.md file.
    fn parse_skill_frontmatter(path: &Path) -> Result<SkillFrontmatter> {
        use std::io::{BufRead, BufReader};

        let file = std::fs::File::open(path).map_err(|e| {
            KernelError::skill(format!(
                "Failed to open skill file: {}: {e}",
                path.display()
            ))
        })?;
        let reader = BufReader::new(file);

        let mut lines = reader.lines();

        // Check if file starts with ---
        let first_line = lines.next().transpose()?;
        if first_line.as_deref() != Some("---") {
            return Err(KernelError::skill(
                "Skill file must start with frontmatter delimiter ---",
            ));
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
            return Err(KernelError::skill("Frontmatter end delimiter not found"));
        }

        // Parse just the frontmatter YAML
        let yaml_content = yaml_lines.join("\n");
        serde_yaml::from_str(&yaml_content)
            .map_err(|e| KernelError::skill(format!("Failed to parse skill frontmatter YAML: {e}")))
    }

    /// Load a single skill from a file
    /// Only reads the frontmatter portion for efficiency
    /// Derives skill name from relative path (e.g., `skill_dir/a/b/SKILL.md` -> a:b)
    fn load_skill(path: &Path, root_folder: &Path) -> Result<Skill> {
        let frontmatter = Self::parse_skill_frontmatter(path)?;
        let skill_name = Self::derive_skill_name(path, root_folder);

        Ok(Skill {
            name: skill_name,
            description: frontmatter.description,
            triggers: frontmatter.triggers,
            hooks: frontmatter.hooks,
            source_path: path.to_path_buf(),
        })
    }

    /// Derive skill name from relative path
    /// e.g., root/a/b/SKILL.md -> a:b
    pub fn derive_skill_name(path: &Path, root_folder: &Path) -> String {
        // Get the relative path from root
        let relative = path.strip_prefix(root_folder).unwrap_or(path);

        // Get all parent components except the file itself
        let components: Vec<_> = relative
            .parent()
            .into_iter()
            .flat_map(|p| p.components())
            .filter_map(|c| {
                if let std::path::Component::Normal(os_str) = c {
                    os_str.to_str()
                } else {
                    None
                }
            })
            .collect();

        if components.is_empty() {
            // Skill is at root level, use filename without extension
            relative
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unnamed")
                .to_string()
        } else {
            // Join components with ':'
            components.join(":")
        }
    }

    /// Find a skill file by name in configured folders (async version)
    /// Returns the path to the skill file if found
    pub async fn find_skill_file(&self, name: &str) -> Option<PathBuf> {
        for folder in &self.folders {
            if let Some(path) = Self::resolve_skill_path(folder, name).await {
                return Some(path);
            }
        }
        None
    }

    /// Resolve skill path by name: folder/{name}/SKILL.md
    /// e.g., "debugging" -> folder/debugging/SKILL.md
    /// e.g., "superpowers:writing" -> folder/superpowers/writing/SKILL.md
    async fn resolve_skill_path(folder: &Path, name: &str) -> Option<PathBuf> {
        let parts: Vec<&str> = name.split(':').collect();
        // Reject path traversal, empty components, and platform separators
        if parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains('/') || p.contains('\\'))
        {
            return None;
        }
        let skill_path = folder
            .join(parts.iter().collect::<std::path::PathBuf>())
            .join("SKILL.md");

        // Ensure the constructed path stays under the skill folder
        if !skill_path.starts_with(folder) {
            return None;
        }

        if tokio::fs::try_exists(&skill_path).await.unwrap_or(false) {
            let canonical = tokio::fs::canonicalize(&skill_path).await.ok()?;
            let canonical_folder = tokio::fs::canonicalize(folder).await.ok()?;
            canonical
                .starts_with(&canonical_folder)
                .then_some(canonical)
        } else {
            None
        }
    }

    /// Read skill file content asynchronously
    pub async fn read_skill_content(path: &Path) -> Result<String> {
        tokio::fs::read_to_string(path).await.map_err(|e| {
            KernelError::skill(format!(
                "Failed to read skill file: {}: {e}",
                path.display()
            ))
        })
    }
}

/// Deduplicate skills by name, keeping the first occurrence.
/// This is a utility function that can be used after loading skills from multiple sources
/// (e.g., folders and plugins) to ensure no duplicate names exist.
pub fn deduplicate_skills(skills: &mut Vec<Arc<Skill>>) {
    let mut seen_names = std::collections::HashSet::new();
    skills.retain(|skill| {
        if seen_names.contains(&skill.name) {
            tracing::debug!(
                "Duplicate skill name '{}' found, keeping first instance.",
                skill.name
            );
            false
        } else {
            seen_names.insert(skill.name.clone());
            true
        }
    });
}

#[cfg(test)]
#[path = "skill_test.rs"]
mod tests;

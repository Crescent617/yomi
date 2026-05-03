use crate::misc::plugin::Plugin;
use crate::types::{KernelError, Result};
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
    /// Name is kept for backwards compatibility but no longer used.
    /// Skill name is now derived from the file path.
    #[allow(dead_code)]
    #[serde(default)]
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

    /// Load a single skill from a file
    /// Only reads the frontmatter portion for efficiency
    /// Derives skill name from relative path (e.g., `skill_dir/a/b/SKILL.md` -> a:b)
    fn load_skill(path: &Path, root_folder: &Path) -> Result<Skill> {
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
        let frontmatter: SkillFrontmatter = serde_yaml::from_str(&yaml_content).map_err(|e| {
            KernelError::skill(format!("Failed to parse skill frontmatter YAML: {e}"))
        })?;

        // Derive skill name from relative path
        // e.g., ~/.claude/skills/superpowers/writing/SKILL.md -> superpowers:writing
        let skill_name = Self::derive_skill_name(path, root_folder);

        Ok(Skill {
            name: skill_name,
            description: frontmatter.description,
            triggers: frontmatter.triggers,
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
        let skill_path = folder
            .join(parts.iter().collect::<std::path::PathBuf>())
            .join("SKILL.md");

        if tokio::fs::try_exists(&skill_path).await.unwrap_or(false) {
            skill_path.canonicalize().ok().or(Some(skill_path))
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

    /// Load skills from a plugin
    pub fn load_from_plugin(plugin: &Plugin) -> Result<Vec<Arc<Skill>>> {
        let mut skills = Vec::new();

        // Load from default skills path
        if let Some(ref skills_path) = plugin.skills_path {
            Self::load_plugin_skills_dir(skills_path, &plugin.name, &mut skills)?;
        }

        // Load from additional skills paths
        for skills_path in &plugin.skills_paths {
            Self::load_plugin_skills_dir(skills_path, &plugin.name, &mut skills)?;
        }

        Ok(skills)
    }

    fn load_plugin_skills_dir(
        skills_path: &Path,
        plugin_name: &str,
        skills: &mut Vec<Arc<Skill>>,
    ) -> Result<()> {
        if !skills_path.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(skills_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    match Self::load_plugin_skill(&skill_file, plugin_name) {
                        Ok(skill) => {
                            tracing::debug!(
                                "Loaded plugin skill '{}' from {}",
                                skill.name,
                                skill_file.display()
                            );
                            skills.push(Arc::new(skill));
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to load plugin skill from {}: {}",
                                skill_file.display(),
                                e
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn load_plugin_skill(path: &Path, plugin_name: &str) -> Result<Skill> {
        use std::io::{BufRead, BufReader};

        let skill_name = Self::derive_plugin_skill_name(path, plugin_name)?;

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
        let frontmatter: SkillFrontmatter = serde_yaml::from_str(&yaml_content).map_err(|e| {
            KernelError::skill(format!("Failed to parse skill frontmatter YAML: {e}"))
        })?;

        Ok(Skill {
            name: skill_name,
            description: frontmatter.description,
            triggers: frontmatter.triggers,
            source_path: path.to_path_buf(),
        })
    }

    fn derive_plugin_skill_name(path: &Path, plugin_name: &str) -> Result<String> {
        let skill_dir = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .ok_or_else(|| KernelError::skill(format!("Invalid skill path: {}", path.display())))?;

        Ok(format!("{plugin_name}:{skill_dir}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_derive_skill_name_single_level() {
        let root = Path::new("/home/user/.claude/skills");
        let path = Path::new("/home/user/.claude/skills/debugging/SKILL.md");
        assert_eq!(SkillLoader::derive_skill_name(path, root), "debugging");
    }

    #[test]
    fn test_derive_skill_name_two_levels() {
        let root = Path::new("/home/user/.claude/skills");
        let path = Path::new("/home/user/.claude/skills/superpowers/writing/SKILL.md");
        assert_eq!(
            SkillLoader::derive_skill_name(path, root),
            "superpowers:writing"
        );
    }

    #[test]
    fn test_derive_skill_name_three_levels() {
        let root = Path::new("/home/user/.claude/skills");
        let path = Path::new("/home/user/.claude/skills/superpowers/writing/plans/SKILL.md");
        assert_eq!(
            SkillLoader::derive_skill_name(path, root),
            "superpowers:writing:plans"
        );
    }

    #[test]
    fn test_derive_skill_name_at_root() {
        let root = Path::new("/home/user/.claude/skills");
        let path = Path::new("/home/user/.claude/skills/SKILL.md");
        assert_eq!(SkillLoader::derive_skill_name(path, root), "SKILL");
    }

    #[test]
    fn test_derive_skill_name_different_filename() {
        let root = Path::new("/home/user/.claude/skills");
        let path = Path::new("/home/user/.claude/skills/mycorp/team/SKILL.md");
        assert_eq!(SkillLoader::derive_skill_name(path, root), "mycorp:team");
    }

    #[test]
    fn test_derive_skill_name_with_windows_separator() {
        // This test is mainly to ensure the logic works with different path separators
        let root = Path::new("/root/skills");
        let path = Path::new("/root/skills/a/b/c/SKILL.md");
        assert_eq!(SkillLoader::derive_skill_name(path, root), "a:b:c");
    }

    #[test]
    fn test_derive_plugin_skill_name() {
        let _loader = SkillLoader::new(vec![]);
        let path = Path::new("/plugins/my-plugin/skills/debugging/SKILL.md");
        let name = SkillLoader::derive_plugin_skill_name(path, "my-plugin").unwrap();
        assert_eq!(name, "my-plugin:debugging");
    }

    #[test]
    fn test_load_plugin_skill() {
        use std::io::Write;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join("debugging");
        std::fs::create_dir(&skill_dir).unwrap();

        let skill_content = r"---
description: A debugging skill
triggers:
  - debug
---

# Debugging Skill

Content here.";

        let mut file = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        file.write_all(skill_content.as_bytes()).unwrap();

        let skill =
            SkillLoader::load_plugin_skill(&skill_dir.join("SKILL.md"), "my-plugin").unwrap();

        assert_eq!(skill.name, "my-plugin:debugging");
        assert_eq!(skill.description, "A debugging skill");
        assert_eq!(skill.triggers, vec!["debug"]);
    }

    #[test]
    fn test_load_from_plugin() {
        use std::io::Write;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let plugin_dir = temp.path().join("test-plugin");
        let skills_dir = plugin_dir.join("skills");
        let skill_a_dir = skills_dir.join("skill-a");
        let skill_b_dir = skills_dir.join("skill-b");

        std::fs::create_dir_all(&skill_a_dir).unwrap();
        std::fs::create_dir_all(&skill_b_dir).unwrap();

        // Create skill A
        let skill_a_content = r"---
description: Skill A
---
";
        let mut file_a = std::fs::File::create(skill_a_dir.join("SKILL.md")).unwrap();
        file_a.write_all(skill_a_content.as_bytes()).unwrap();

        // Create skill B
        let skill_b_content = r"---
description: Skill B
---
";
        let mut file_b = std::fs::File::create(skill_b_dir.join("SKILL.md")).unwrap();
        file_b.write_all(skill_b_content.as_bytes()).unwrap();

        let plugin = Plugin {
            name: "test-plugin".to_string(),
            path: plugin_dir,
            skills_path: Some(skills_dir),
            skills_paths: vec![],
        };

        let _loader = SkillLoader::new(vec![]);
        let skills = SkillLoader::load_from_plugin(&plugin).unwrap();

        assert_eq!(skills.len(), 2);

        let names: Vec<_> = skills.iter().map(|s| s.name.clone()).collect();
        assert!(names.contains(&"test-plugin:skill-a".to_string()));
        assert!(names.contains(&"test-plugin:skill-b".to_string()));
    }
}

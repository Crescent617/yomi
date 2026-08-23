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
    /// Frontmatter `disable-model-invocation: true`: loadable by name/path
    /// (the `skill` tool resolves paths directly, never consults the index)
    /// but excluded from the prompt index — auto-invocation is opt-out.
    #[serde(default)]
    pub disable_model_invocation: bool,
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
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: bool,
}

/// Skill loader that scans directories for SKILL.md files
#[derive(Debug, Clone)]
pub struct SkillLoader {
    folders: Vec<PathBuf>,
}

/// Directory (relative to a session's working directory) scanned for workspace skills.
pub const WORKSPACE_SKILLS_DIR: &str = ".agents/skills";

/// Scan depth below a skill root: only the top level is indexed
/// (`foo/SKILL.md`); nested SKILL.md files (`suite/child/SKILL.md`) never
/// enter the index — a suite's top-level SKILL.md routes to its children,
/// which load on demand via the `skill` tool or `read`.
pub const MAX_SCAN_DEPTH: usize = 1;

/// Resolve the workspace skills directory for `cwd`, if it exists.
pub async fn workspace_skill_dir(cwd: &Path) -> Option<PathBuf> {
    let dir = cwd.join(WORKSPACE_SKILLS_DIR);
    tokio::fs::try_exists(&dir)
        .await
        .unwrap_or(false)
        .then_some(dir)
}

/// Load workspace skills from `dir` and merge them over `global`; workspace
/// skills win on name collision. The scan is best-effort — failures are
/// logged and skipped — so `global` is effectively the fallback.
pub async fn load_workspace_skills(dir: &Path, global: Vec<Arc<Skill>>) -> Vec<Arc<Skill>> {
    let workspace = SkillLoader::new(vec![dir.to_path_buf()]).load_all().await;
    tracing::info!(
        "loaded {} skill(s) from workspace {}",
        workspace.len(),
        dir.display()
    );
    let mut merged = merge_skills(global, workspace);
    drop_manual_skills(&mut merged);
    merged
}

/// Merge two skill sets; `workspace` entries override `global` on name
/// collision. Order is stable: global order is preserved (overridden entries
/// keep their position), workspace-only skills are appended.
pub fn merge_skills(global: Vec<Arc<Skill>>, workspace: Vec<Arc<Skill>>) -> Vec<Arc<Skill>> {
    let mut merged = global;
    for skill in workspace {
        match merged.iter_mut().find(|s| s.name == skill.name) {
            Some(existing) => *existing = skill,
            None => merged.push(skill),
        }
    }
    merged
}

/// Skill files are any `*SKILL.md` (e.g. `SKILL.md`, `debugging/SKILL.md`).
fn is_skill_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .ends_with("SKILL.md")
}

impl SkillLoader {
    pub const fn new(folders: Vec<PathBuf>) -> Self {
        Self { folders }
    }

    /// Load all skills from configured folders.
    ///
    /// Best-effort scan: a missing or unreadable folder is logged and skipped
    /// rather than failing the whole load. Results are sorted by name so the
    /// assembled system prompt is stable across spawns.
    pub async fn load_all(&self) -> Vec<Arc<Skill>> {
        let mut skills = Vec::new();

        for folder in &self.folders {
            if tokio::fs::try_exists(folder).await.unwrap_or(false) {
                Self::load_from_folder(folder, &mut skills).await;
            } else {
                tracing::warn!("Skill folder does not exist: {}", folder.display());
            }
        }
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }

    /// Load skills from a single folder; only top-level `SKILL.md` files are
    /// indexed (see [`MAX_SCAN_DEPTH`]).
    ///
    /// Anything broken along the way (unreadable directory, failed stat,
    /// malformed SKILL.md) is logged and skipped so one bad entry cannot take
    /// down the rest.
    async fn load_from_folder(root_folder: &Path, skills: &mut Vec<Arc<Skill>>) {
        let mut stack = vec![(root_folder.to_path_buf(), 0usize)];
        while let Some((current, depth)) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&current).await {
                Ok(entries) => entries,
                Err(e) => {
                    tracing::warn!(
                        "Failed to read skill directory {}: {}",
                        current.display(),
                        e
                    );
                    continue;
                }
            };
            loop {
                let entry = match entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(e) => {
                        // Stop this directory — a persistent iterator error
                        // must not spin the loop.
                        tracing::warn!("Failed to read entry in {}: {}", current.display(), e);
                        break;
                    }
                };
                let path = entry.path();
                // `metadata` (not `entry.file_type`) so symlinks are followed:
                // skills are commonly linked in from dotfiles repos. Cycles
                // are bounded by MAX_SCAN_DEPTH. Dangling links fail here and
                // are skipped with a warning.
                let file_type = match tokio::fs::metadata(&path).await {
                    Ok(metadata) => metadata.file_type(),
                    Err(e) => {
                        tracing::warn!("Failed to stat {}: {}", path.display(), e);
                        continue;
                    }
                };

                if file_type.is_dir() {
                    if depth < MAX_SCAN_DEPTH {
                        stack.push((path, depth + 1));
                    }
                } else if file_type.is_file() && is_skill_file(&path) {
                    match Self::load_skill(&path, root_folder).await {
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
    }

    /// Parse the YAML frontmatter from a SKILL.md file.
    async fn parse_skill_frontmatter(path: &Path) -> Result<SkillFrontmatter> {
        use tokio::io::AsyncBufReadExt;

        let file = tokio::fs::File::open(path).await.map_err(|e| {
            KernelError::skill(format!(
                "Failed to open skill file: {}: {e}",
                path.display()
            ))
        })?;
        let mut lines = tokio::io::BufReader::new(file).lines();

        // Check if file starts with ---
        let first_line = lines.next_line().await?;
        if first_line.as_deref() != Some("---") {
            return Err(KernelError::skill(
                "Skill file must start with frontmatter delimiter ---",
            ));
        }

        // Collect frontmatter lines until second ---
        let mut yaml_lines = Vec::new();
        let mut found_end = false;

        while let Some(line) = lines.next_line().await? {
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
    async fn load_skill(path: &Path, root_folder: &Path) -> Result<Skill> {
        let frontmatter = Self::parse_skill_frontmatter(path).await?;
        let skill_name = Self::derive_skill_name(path, root_folder);

        Ok(Skill {
            name: skill_name,
            description: frontmatter.description,
            triggers: frontmatter.triggers,
            disable_model_invocation: frontmatter.disable_model_invocation,
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

/// Drop skills marked `disable-model-invocation`: they stay loadable by
/// name/path (the `skill` tool resolves paths directly, never consults the
/// index) but never enter the prompt index. Apply after the final merge so a
/// workspace skill can disable a same-named global one.
pub fn drop_manual_skills(skills: &mut Vec<Arc<Skill>>) {
    let before = skills.len();
    skills.retain(|s| !s.disable_model_invocation);
    if skills.len() != before {
        tracing::debug!("drop_manual_skills: {} -> {}", before, skills.len());
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

//! CLAUDE.md and AGENTS.md loader
//!
//! Loads project instructions from current directory only:
//! - CLAUDE.md: Project instructions
//! - AGENTS.md: Agent definitions
//! - .claude/CLAUDE.md: Alternative location
//! - .claude/rules/*.md: Rule files

use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;

/// Memory file info
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub content: String,
    pub ty: MemoryType,
}

/// Type of memory file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// Project-level instructions (CLAUDE.md)
    Project,
    /// Agent definitions (AGENTS.md)
    Agents,
    /// Rule files (.claude/rules/*.md)
    Rule,
}

/// Load CLAUDE.md and AGENTS.md from current directory
pub async fn load(cwd: &Path) -> anyhow::Result<MemoryFiles> {
    let mut files = vec![];

    // Try AGENTS.md
    let agents_md = cwd.join("AGENTS.md");
    let claude_md = cwd.join("CLAUDE.md");

    if let Ok(content) = fs::read_to_string(&agents_md).await {
        files.push(MemoryFile {
            path: agents_md,
            content,
            ty: MemoryType::Agents,
        });
    } else if let Ok(content) = fs::read_to_string(&claude_md).await {
        files.push(MemoryFile {
            path: claude_md,
            content,
            ty: MemoryType::Project,
        });
    }

    info!("Loaded {} memory files from {}", files.len(), cwd.display());
    Ok(MemoryFiles { files })
}

/// Collection of loaded memory files
#[derive(Debug, Clone, Default)]
pub struct MemoryFiles {
    files: Vec<MemoryFile>,
}

impl MemoryFiles {
    /// Create empty memory files
    pub const fn empty() -> Self {
        Self { files: vec![] }
    }

    /// Check if any files were loaded
    pub const fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get number of files
    pub const fn len(&self) -> usize {
        self.files.len()
    }

    /// Get project instructions (CLAUDE.md content)
    pub fn project_instructions(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter(|f| f.ty == MemoryType::Project)
            .map(|f| f.content.as_str())
            .collect()
    }

    /// Get agent definitions (AGENTS.md content)
    pub fn agent_definitions(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter(|f| f.ty == MemoryType::Agents)
            .map(|f| f.content.as_str())
            .collect()
    }

    /// Get rule files
    pub fn rules(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter(|f| f.ty == MemoryType::Rule)
            .map(|f| f.content.as_str())
            .collect()
    }

    /// Build system prompt from all project instructions
    pub fn build_system_prompt(&self, base_prompt: &str) -> String {
        let mut parts = vec![base_prompt.to_string()];

        for file in &self.files {
            if file.ty == MemoryType::Project {
                parts.push(format!(
                    "\n\n# Project Instructions ({}):\n\n{}",
                    file.path.display(),
                    file.content.trim()
                ));
            }
        }

        parts.join("")
    }

    /// Get all files
    pub fn files(&self) -> &[MemoryFile] {
        &self.files
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_claude_md() {
        let temp = TempDir::new().unwrap();
        let mut file = std::fs::File::create(temp.path().join("CLAUDE.md")).unwrap();
        writeln!(file, "# Test Instructions").unwrap();

        let files = load(temp.path()).await.unwrap();
        assert_eq!(files.len(), 1);
        assert!(files.project_instructions()[0].contains("Test Instructions"));
    }

    #[tokio::test]
    async fn test_build_system_prompt() {
        let temp = TempDir::new().unwrap();
        let mut file = std::fs::File::create(temp.path().join("CLAUDE.md")).unwrap();
        writeln!(file, "Be helpful").unwrap();

        let files = load(temp.path()).await.unwrap();
        let prompt = files.build_system_prompt("You are a coding assistant.");
        assert!(prompt.contains("You are a coding assistant."));
        assert!(prompt.contains("Be helpful"));
    }
}

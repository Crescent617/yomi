//! Project memory loader (CLAUDE.md, AGENTS.md)
//!
//! Loads project instructions from current directory:
//! - AGENTS.md: Agent definitions (preferred, takes precedence)
//! - CLAUDE.md: Project instructions (fallback)

use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;

/// Memory file info
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub content: String,
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
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get number of files
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Build system prompt from all memory files
    pub fn build_system_prompt(&self, base_prompt: &str) -> String {
        let mut parts = vec![base_prompt.to_string()];

        for file in &self.files {
            parts.push(format!(
                "\n\n# Project Instructions ({}):\n\n{}",
                file.path.display(),
                file.content.trim()
            ));
        }

        parts.join("")
    }

    /// Get all files
    pub fn files(&self) -> &[MemoryFile] {
        &self.files
    }
}

/// Load CLAUDE.md and AGENTS.md from current directory
pub async fn load(cwd: &Path) -> crate::types::Result<MemoryFiles> {
    let mut files = vec![];

    // Try AGENTS.md
    let agents_md = cwd.join("AGENTS.md");
    let claude_md = cwd.join("CLAUDE.md");

    // Try AGENTS.md first, fall back to CLAUDE.md
    // Note: Only one file is loaded to avoid conflicting instructions.
    // AGENTS.md takes precedence as it's the newer, more generic standard.
    if let Ok(content) = fs::read_to_string(&agents_md).await {
        files.push(MemoryFile {
            path: agents_md,
            content,
        });
    } else if let Ok(content) = fs::read_to_string(&claude_md).await {
        files.push(MemoryFile {
            path: claude_md,
            content,
        });
    }

    info!("Loaded {} memory files from {}", files.len(), cwd.display());
    Ok(MemoryFiles { files })
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

//! Path utilities for the kernel crate

use std::path::PathBuf;
use std::sync::LazyLock;

/// Default data directory path
pub const DEFAULT_DATA_DIR: &str = "~/.yomi";

static HOME_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
});

/// Expand `~` to the user's home directory
pub fn expand_tilde(path: impl AsRef<str>) -> PathBuf {
    let path = path.as_ref();
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(ref home) = *HOME_DIR {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

/// Generate default skill folders based on `data_dir`
/// (project-level skills are resolved per-session by the kernel).
pub fn default_skill_folders(data_dir: &std::path::Path) -> Vec<PathBuf> {
    vec![data_dir.join("skills"), expand_tilde("~/.agents/skills")]
}

/// Session workspace cwd rule: the session's `working_dir` when set, else
/// `<data_dir>/workspace`. Subagent spawn (`conductor`, `subagent` tool) and
/// workspace-layer asset resolution (agent templates) must all agree on
/// this rule.
pub fn session_workspace_dir(data_dir: &std::path::Path, working_dir: Option<PathBuf>) -> PathBuf {
    working_dir.unwrap_or_else(|| data_dir.join("workspace"))
}

#[cfg(test)]
#[path = "path_test.rs"]
mod tests;

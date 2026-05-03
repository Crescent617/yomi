//! Path utilities for the kernel crate

use std::path::PathBuf;

/// Default data directory path
pub const DEFAULT_DATA_DIR: &str = "~/.yomi";

/// Expand `~` to the user's home directory
pub fn expand_tilde(path: impl AsRef<str>) -> PathBuf {
    let path = path.as_ref();
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            return PathBuf::from(home).join(stripped);
        }
    }
    PathBuf::from(path)
}

/// Generate default skill folders based on `working_dir` and `data_dir`
pub fn default_skill_folders(
    working_dir: &std::path::Path,
    data_dir: &std::path::Path,
) -> Vec<PathBuf> {
    vec![
        working_dir.join(".agents/skills"),
        data_dir.join("skills"),
        expand_tilde("~/.agents/skills"),
        expand_tilde("~/.claude/skills"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde() {
        let home = std::env::var("HOME").unwrap_or_default();

        // Test tilde expansion
        assert_eq!(expand_tilde("~/foo"), PathBuf::from(format!("{home}/foo")));
        assert_eq!(
            expand_tilde("~/.yomi"),
            PathBuf::from(format!("{home}/.yomi"))
        );

        // Test paths without tilde are unchanged
        assert_eq!(
            expand_tilde("/absolute/path"),
            PathBuf::from("/absolute/path")
        );
        assert_eq!(
            expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );

        // Test tilde not at start
        assert_eq!(expand_tilde("/foo~/bar"), PathBuf::from("/foo~/bar"));
    }

    #[test]
    fn test_default_data_dir_expanded() {
        let config = expand_tilde(DEFAULT_DATA_DIR);
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(config, PathBuf::from(format!("{home}/.yomi")));
    }

    #[test]
    fn test_default_skill_folders() {
        let working = PathBuf::from("/working");
        let data = PathBuf::from("/data");
        let folders = default_skill_folders(&working, &data);

        assert_eq!(folders.len(), 4);
        assert_eq!(folders[0], PathBuf::from("/working/.agents/skills"));
        assert_eq!(folders[1], PathBuf::from("/data/skills"));
        // [2] and [3] depend on HOME, just check they end correctly
        assert!(folders[2].to_string_lossy().ends_with("/.agents/skills"));
        assert!(folders[3].to_string_lossy().ends_with("/.claude/skills"));
    }
}

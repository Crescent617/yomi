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
/// (project-level skills are resolved per-session by the coordinator).
pub fn default_skill_folders(data_dir: &std::path::Path) -> Vec<PathBuf> {
    vec![data_dir.join("skills"), expand_tilde("~/.agents/skills")]
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
        let data = PathBuf::from("/data");
        let folders = default_skill_folders(&data);

        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0], PathBuf::from("/data/skills"));
        assert!(folders[1].to_string_lossy().ends_with("/.agents/skills"));
    }
}

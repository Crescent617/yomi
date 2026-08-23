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
    assert!(folders[0].to_string_lossy().ends_with("/.agents/skills"));
    assert_eq!(folders[1], PathBuf::from("/data/skills"));
}

#[test]
fn session_workspace_dir_prefers_working_dir() {
    let data = PathBuf::from("/data");
    assert_eq!(
        session_workspace_dir(&data, Some(PathBuf::from("/proj"))),
        PathBuf::from("/proj")
    );
}

#[test]
fn session_workspace_dir_falls_back_to_data_workspace() {
    assert_eq!(
        session_workspace_dir(std::path::Path::new("/data"), None),
        PathBuf::from("/data/workspace")
    );
}

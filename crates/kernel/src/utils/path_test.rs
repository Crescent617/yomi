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

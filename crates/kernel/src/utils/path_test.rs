use super::*;

#[test]
fn test_expand_tilde() {
    let home = HOME_DIR.as_ref().expect("home dir must resolve");

    // Test tilde expansion
    assert_eq!(expand_tilde("~/foo"), home.join("foo"));
    assert_eq!(expand_tilde("~/.yomi"), home.join(".yomi"));

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
    let home = HOME_DIR.as_ref().expect("home dir must resolve");
    assert_eq!(config, home.join(".yomi"));
}

#[test]
fn test_default_skill_folders() {
    let data = PathBuf::from("/data");
    let folders = default_skill_folders(&data);

    assert_eq!(folders.len(), 2);
    assert_eq!(folders[0], expand_tilde("~/.agents/skills"));
    assert_eq!(folders[1], data.join("skills"));
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

#[test]
fn prepend_path_dir_prepends_once_and_is_idempotent() {
    let dir = std::path::Path::new("/Applications/Yomi.app/Contents/MacOS");
    let sep = if cfg!(windows) { ';' } else { ':' };
    let path = format!("/usr/bin{sep}/opt/homebrew/bin");

    let once = super::prepend_path_dir(&path, dir);
    assert_eq!(
        once,
        format!("/Applications/Yomi.app/Contents/MacOS{sep}/usr/bin{sep}/opt/homebrew/bin")
    );
    // 幂等：目录已在 PATH 中（如重入/重启）时原样返回。
    assert_eq!(super::prepend_path_dir(&once, dir), once);
}

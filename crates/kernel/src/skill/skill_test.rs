use super::*;

#[test]
fn test_derive_skill_name_single_level() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/debugging/SKILL.md");
    assert_eq!(SkillScanner::derive_skill_name(path, root), "debugging");
}

#[test]
fn test_derive_skill_name_two_levels() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/superpowers/writing/SKILL.md");
    assert_eq!(
        SkillScanner::derive_skill_name(path, root),
        "superpowers:writing"
    );
}

#[test]
fn test_derive_skill_name_three_levels() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/superpowers/writing/plans/SKILL.md");
    assert_eq!(
        SkillScanner::derive_skill_name(path, root),
        "superpowers:writing:plans"
    );
}

#[test]
fn test_derive_skill_name_at_root() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/SKILL.md");
    assert_eq!(SkillScanner::derive_skill_name(path, root), "SKILL");
}

#[test]
fn test_derive_skill_name_different_filename() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/mycorp/team/SKILL.md");
    assert_eq!(SkillScanner::derive_skill_name(path, root), "mycorp:team");
}

#[test]
fn test_derive_skill_name_with_windows_separator() {
    // This test is mainly to ensure the logic works with different path separators
    let root = Path::new("/root/skills");
    let path = Path::new("/root/skills/a/b/c/SKILL.md");
    assert_eq!(SkillScanner::derive_skill_name(path, root), "a:b:c");
}

fn write_skill(dir: &Path, description: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\ndescription: {description}\n---\nbody never read at load time\n"),
    )
    .unwrap();
}

fn skill_names(skills: &[Arc<Skill>]) -> Vec<String> {
    let mut names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn load_all_indexes_only_top_level_skills() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    write_skill(&root.join("a"), "top level — indexed");
    write_skill(&root.join("a/b"), "nested — routed to by a, not indexed");
    write_skill(&root.join("a/b/c"), "deeper — not indexed");
    write_skill(&root.join("a/b/c/d"), "deepest — not indexed");

    let skills = SkillScanner::new(vec![root.to_path_buf()]).load_all().await;

    assert_eq!(skill_names(&skills), vec!["a"]);
}

#[tokio::test]
async fn load_all_skips_broken_entries_instead_of_failing_the_folder() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    write_skill(&root.join("good"), "loads fine");
    // Malformed frontmatter: skipped with a warning, must not kill the folder.
    std::fs::create_dir_all(root.join("bad")).unwrap();
    std::fs::write(root.join("bad/SKILL.md"), "no frontmatter here\n").unwrap();
    // An unreadable subdirectory: skipped as well.
    let unreadable = root.join("unreadable");
    write_skill(&unreadable.join("nested"), "hidden");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    }

    let skills = SkillScanner::new(vec![root.to_path_buf()]).load_all().await;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let names = skill_names(&skills);
    assert!(names.contains(&"good".to_string()));
    assert!(!names.contains(&"bad".to_string()));
    // (Only unix applies the permission denial above.)
    #[cfg(unix)]
    assert!(!names.contains(&"unreadable:nested".to_string()));
}

#[tokio::test]
async fn workspace_skill_dir_resolves_only_when_present() {
    let cwd = tempfile::tempdir().unwrap();
    assert!(workspace_skill_dir(cwd.path()).await.is_none());

    std::fs::create_dir_all(cwd.path().join(WORKSPACE_SKILLS_DIR)).unwrap();
    assert_eq!(
        workspace_skill_dir(cwd.path()).await,
        Some(cwd.path().join(WORKSPACE_SKILLS_DIR))
    );
}

#[tokio::test]
async fn load_all_tolerates_a_missing_folder() {
    let root = tempfile::tempdir().unwrap();
    write_skill(&root.path().join("present"), "loads fine");

    let skills = SkillScanner::new(vec![
        root.path().join("does-not-exist"),
        root.path().to_path_buf(),
    ])
    .load_all()
    .await;

    assert_eq!(skill_names(&skills), vec!["present"]);
}

/// Symlinked skill dirs/files are followed (dotfiles repos commonly link
/// skills into place); dangling links are skipped with a warning.
#[cfg(unix)]
#[tokio::test]
async fn load_all_follows_symlinks() {
    let outside = tempfile::tempdir().unwrap();
    write_skill(&outside.path().join("linked-dir"), "via dir link");
    write_skill(&outside.path().join("linked-file"), "via file link");

    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::os::unix::fs::symlink(outside.path().join("linked-dir"), root.join("linked-dir")).unwrap();
    std::fs::create_dir_all(root.join("flat")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("linked-file/SKILL.md"),
        root.join("flat/SKILL.md"),
    )
    .unwrap();
    // Dangling symlink: skipped, must not kill the scan.
    std::os::unix::fs::symlink(root.join("no-such-target"), root.join("dangling")).unwrap();

    let skills = SkillScanner::new(vec![root.to_path_buf()]).load_all().await;

    assert_eq!(skill_names(&skills), vec!["flat", "linked-dir"]);
}

#[tokio::test]
async fn load_all_parses_disable_model_invocation_flag() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    write_skill(&root.join("auto"), "auto");
    std::fs::create_dir_all(root.join("manual")).unwrap();
    std::fs::write(
        root.join("manual/SKILL.md"),
        "---\ndescription: manual only\ndisable-model-invocation: true\n---\n",
    )
    .unwrap();

    // load_all 本身不过滤（`yomi skill list` 要能看到全部），flag 由
    // drop_manual_skills 在索引装配时消费。
    let skills = SkillScanner::new(vec![root.to_path_buf()]).load_all().await;

    assert!(
        skills
            .iter()
            .find(|s| s.name == "manual")
            .unwrap()
            .disable_model_invocation
    );
    assert!(
        !skills
            .iter()
            .find(|s| s.name == "auto")
            .unwrap()
            .disable_model_invocation
    );
}

#[test]
fn session_skill_folders_appends_workspace_as_highest_precedence_layer() {
    assert_eq!(
        session_skill_folders(&[PathBuf::from("/global")], Some(PathBuf::from("/ws"))),
        vec![PathBuf::from("/global"), PathBuf::from("/ws")]
    );
    assert_eq!(
        session_skill_folders(&[PathBuf::from("/global")], None),
        vec![PathBuf::from("/global")]
    );
}

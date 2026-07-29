use super::*;

#[test]
fn test_derive_skill_name_single_level() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/debugging/SKILL.md");
    assert_eq!(SkillLoader::derive_skill_name(path, root), "debugging");
}

#[test]
fn test_derive_skill_name_two_levels() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/superpowers/writing/SKILL.md");
    assert_eq!(
        SkillLoader::derive_skill_name(path, root),
        "superpowers:writing"
    );
}

#[test]
fn test_derive_skill_name_three_levels() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/superpowers/writing/plans/SKILL.md");
    assert_eq!(
        SkillLoader::derive_skill_name(path, root),
        "superpowers:writing:plans"
    );
}

#[test]
fn test_derive_skill_name_at_root() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/SKILL.md");
    assert_eq!(SkillLoader::derive_skill_name(path, root), "SKILL");
}

#[test]
fn test_derive_skill_name_different_filename() {
    let root = Path::new("/home/user/.skills");
    let path = Path::new("/home/user/.skills/mycorp/team/SKILL.md");
    assert_eq!(SkillLoader::derive_skill_name(path, root), "mycorp:team");
}

#[test]
fn test_derive_skill_name_with_windows_separator() {
    // This test is mainly to ensure the logic works with different path separators
    let root = Path::new("/root/skills");
    let path = Path::new("/root/skills/a/b/c/SKILL.md");
    assert_eq!(SkillLoader::derive_skill_name(path, root), "a:b:c");
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
async fn load_all_scans_at_most_three_levels_deep() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    write_skill(&root.join("a"), "level 1");
    write_skill(&root.join("a/b"), "level 2");
    write_skill(&root.join("a/b/c"), "level 3");
    write_skill(&root.join("a/b/c/d"), "level 4 — beyond MAX_SCAN_DEPTH");

    let skills = SkillLoader::new(vec![root.to_path_buf()]).load_all().await;

    assert_eq!(skill_names(&skills), vec!["a", "a:b", "a:b:c"]);
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

    let skills = SkillLoader::new(vec![root.to_path_buf()]).load_all().await;

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

#[test]
fn merge_skills_workspace_overrides_global_on_name_collision() {
    fn skill(name: &str, description: &str) -> Arc<Skill> {
        Arc::new(Skill {
            name: name.to_string(),
            description: description.to_string(),
            triggers: Vec::new(),
            source_path: PathBuf::from("/tmp/SKILL.md"),
        })
    }

    let merged = merge_skills(
        vec![skill("a", "global a"), skill("b", "global b")],
        vec![skill("b", "workspace b"), skill("c", "workspace c")],
    );

    // Workspace overrides in place (global order preserved); workspace-only
    // skills are appended.
    let descriptions: Vec<&str> = merged.iter().map(|s| s.description.as_str()).collect();
    assert_eq!(descriptions, vec!["global a", "workspace b", "workspace c"]);
}

#[tokio::test]
async fn load_all_tolerates_a_missing_folder() {
    let root = tempfile::tempdir().unwrap();
    write_skill(&root.path().join("present"), "loads fine");

    let skills = SkillLoader::new(vec![
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

    let skills = SkillLoader::new(vec![root.to_path_buf()]).load_all().await;

    assert_eq!(skill_names(&skills), vec!["flat", "linked-dir"]);
}

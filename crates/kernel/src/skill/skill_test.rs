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

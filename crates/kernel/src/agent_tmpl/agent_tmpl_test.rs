use super::*;
use std::path::PathBuf;

#[test]
fn builtin_templates_load() {
    let templates = builtin();
    assert_eq!(templates.len(), 4);
    for t in &templates {
        assert_eq!(t.source, TemplateSource::Builtin);
        assert!(!t.body.is_empty(), "{} missing body", t.name);
    }
}

#[test]
fn parse_uses_whole_file_as_body() {
    let t = parse("raw", "  just a body\n", TemplateSource::Global);
    assert_eq!(t.body, "just a body");
}

fn workspace_with(tag: &str, templates: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yomi-tmpl-test-{tag}-{}", std::process::id()));
    for (name, content) in templates {
        let sub = dir.join(WORKSPACE_DIR).join(name);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(ROLE_FILE), content).unwrap();
    }
    dir
}

fn temp_global(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("yomi-tmpl-global-{tag}-{}", std::process::id()))
}

#[tokio::test]
async fn workspace_layer_overrides_builtin_by_name() {
    let dir = workspace_with("override", &[("verifier", "custom body\n")]);
    let global = temp_global("override");

    let t = resolve("verifier", &global, Some(&dir)).await.unwrap();
    assert_eq!(t.source, TemplateSource::Workspace);
    assert_eq!(t.body, "custom body");
    // 未被覆盖的 builtin 仍在
    assert!(
        resolve("planner", &global, Some(&dir))
            .await
            .unwrap()
            .source
            == TemplateSource::Builtin
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn resolve_unknown_returns_none() {
    let global = temp_global("none");
    assert!(resolve("no-such-template", &global, None).await.is_none());
}

#[tokio::test]
async fn load_dir_skips_flat_files_and_garbage() {
    let dir = workspace_with("filter", &[("good", "body\n")]);
    // 平铺 .md 不是约定结构（必须是 <name>/ROLE.md），不加载
    std::fs::write(dir.join(WORKSPACE_DIR).join("flat.md"), "x\n").unwrap();
    // 目录存在但没有 ROLE.md
    std::fs::create_dir_all(dir.join(WORKSPACE_DIR).join("empty")).unwrap();

    let global = temp_global("filter");
    let ws: Vec<_> = list(&global, Some(&dir))
        .await
        .into_iter()
        .filter(|t| t.source == TemplateSource::Workspace)
        .collect();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].name, "good");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn validate_name_accepts_kebab_case() {
    for name in ["planner", "my-role", "a", "0x", "release-checker-2"] {
        assert!(validate_name(name).is_ok(), "{name} should be valid");
    }
}

#[test]
fn validate_name_rejects_traversal_and_bad_chars() {
    for name in [
        "",
        "-lead",
        "UPPER",
        "has space",
        "../escape",
        "a/b",
        "a\\b",
        &"x".repeat(65),
    ] {
        assert!(validate_name(name).is_err(), "{name} should be invalid");
    }
}

#[tokio::test]
async fn save_then_resolve_roundtrip() {
    let global = temp_global("save");
    let file = save(&global, "my-role", "# My Role\nbody\n").await.unwrap();
    assert!(file.ends_with(format!("my-role/{ROLE_FILE}")));

    let t = resolve("my-role", &global, None).await.unwrap();
    assert_eq!(t.source, TemplateSource::Global);
    assert_eq!(t.body, "# My Role\nbody");
    std::fs::remove_dir_all(&global).unwrap();
}

#[tokio::test]
async fn save_rejects_empty_body() {
    let global = temp_global("empty");
    assert!(save(&global, "blank", "  \n").await.is_err());
}

#[tokio::test]
async fn delete_override_restores_lower_layer() {
    let global = temp_global("del");
    save(&global, "verifier", "custom body\n").await.unwrap();
    assert_eq!(
        resolve("verifier", &global, None).await.unwrap().source,
        TemplateSource::Global
    );

    delete(&global, "verifier").await.unwrap();
    let t = resolve("verifier", &global, None).await.unwrap();
    assert_eq!(t.source, TemplateSource::Builtin);
    std::fs::remove_dir_all(&global).unwrap();
}

#[tokio::test]
async fn delete_rejects_bad_name() {
    let global = temp_global("delbad");
    assert!(delete(&global, "../outside").await.is_err());
}

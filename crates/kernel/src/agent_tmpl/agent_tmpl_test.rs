use super::*;
use std::path::PathBuf;

#[test]
fn builtin_templates_load() {
    let templates = builtin();
    assert_eq!(templates.len(), 3);
    for t in &templates {
        assert_eq!(t.source, TemplateSource::Builtin);
        assert!(!t.description.is_empty(), "{} missing description", t.name);
        assert!(!t.body.is_empty(), "{} missing body", t.name);
    }
    let reviewer = templates.iter().find(|t| t.name == "reviewer").unwrap();
    // 内置模板不设 tools_block（约束走 prompt，机制留给自定义模板）
    assert!(reviewer.tools_block.is_empty());
}

#[test]
fn parse_tolerates_missing_frontmatter() {
    let t = parse("raw", "just a body", TemplateSource::Global);
    assert_eq!(t.body, "just a body");
    assert!(t.description.is_empty());
    assert!(t.tools_block.is_empty());
}

#[test]
fn parse_tolerates_unknown_fields() {
    // model_key / skills 等字段当前刻意全继承、解析忽略；写入不破坏解析。
    let content = "---\ndescription: 快速执行者\ntools_block: [shell]\nmodel_key: fast-model\nskills: [task-board]\n---\n\nbody text\n";
    let t = parse("fast", content, TemplateSource::Workspace);
    assert_eq!(t.description, "快速执行者");
    assert_eq!(t.tools_block, vec!["shell"]);
    assert_eq!(t.body, "body text");
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

#[tokio::test]
async fn workspace_layer_overrides_builtin_by_name() {
    let dir = workspace_with(
        "override",
        &[(
            "reviewer",
            "---\ndescription: 项目定制验收者\n---\n\ncustom body\n",
        )],
    );
    let global = std::env::temp_dir().join(format!("yomi-tmpl-global-{}", std::process::id()));
    let t = resolve("reviewer", &global, Some(&dir)).await.unwrap();
    assert_eq!(t.source, TemplateSource::Workspace);
    assert_eq!(t.body, "custom body");
    // 内置 reviewer 无 tools_block，覆盖后亦为空
    assert!(t.tools_block.is_empty());
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
async fn override_can_only_add_tool_blocks() {
    // 全局层有收窄，workspace 覆盖只能加不能减
    let dir = workspace_with(
        "union",
        &[(
            "strict-reviewer",
            "---\ndescription: 更严的验收者\ntools_block: [shell]\n---\n\nbody\n",
        )],
    );
    let global =
        std::env::temp_dir().join(format!("yomi-tmpl-global-union-{}", std::process::id()));
    std::fs::create_dir_all(global.join("strict-reviewer")).unwrap();
    std::fs::write(
        global.join("strict-reviewer").join(ROLE_FILE),
        "---\ndescription: 全局验收者\ntools_block: [write, edit]\n---\n\nbody\n",
    )
    .unwrap();

    let t = resolve("strict-reviewer", &global, Some(&dir))
        .await
        .unwrap();
    assert_eq!(t.tools_block, vec!["edit", "shell", "write"]);

    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&global).unwrap();
}

#[test]
fn unclosed_frontmatter_falls_back_to_whole_body() {
    let t = parse(
        "broken",
        "---\ndescription: [unclosed\nbody survives\n",
        TemplateSource::Global,
    );
    assert!(t.body.contains("body survives"));
    assert!(t.description.is_empty());
}

#[tokio::test]
async fn resolve_unknown_returns_none() {
    let global = std::env::temp_dir().join(format!("yomi-tmpl-global-{}", std::process::id()));
    assert!(resolve("no-such-template", &global, None).await.is_none());
}

#[tokio::test]
async fn load_dir_skips_flat_files_and_garbage() {
    let dir = workspace_with("filter", &[("good", "body\n")]);
    // 平铺 .md 不是约定结构（必须是 <name>/ROLE.md），不加载
    std::fs::write(dir.join(WORKSPACE_DIR).join("flat.md"), "x\n").unwrap();
    // 目录存在但没有 ROLE.md
    std::fs::create_dir_all(dir.join(WORKSPACE_DIR).join("empty")).unwrap();

    let global = std::env::temp_dir().join(format!("yomi-tmpl-global-{}", std::process::id()));
    let ws: Vec<_> = list(&global, Some(&dir))
        .await
        .into_iter()
        .filter(|t| t.source == TemplateSource::Workspace)
        .collect();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].name, "good");
    std::fs::remove_dir_all(&dir).unwrap();
}

use super::*;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("yomi-prompt-test-{tag}-{}", std::process::id()))
}

#[tokio::test]
async fn memory_pointer_injected_when_project_index_exists() {
    let dir = temp_dir("exists");
    let mem_dir = dir.join(".agents/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    std::fs::write(mem_dir.join("MEMORY.md"), "- fact\n").unwrap();

    let prompt = SystemPromptBuilder::new()
        .base_prompt("base")
        .with_working_dir(&dir)
        .build()
        .await;

    assert!(prompt.contains("# Memory"));
    assert!(prompt.contains(&format!(
        "- Project: {}",
        mem_dir.join("MEMORY.md").display()
    )));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn memory_pointer_absent_without_project_index() {
    let dir = temp_dir("absent");
    std::fs::create_dir_all(&dir).unwrap();

    let prompt = SystemPromptBuilder::new()
        .base_prompt("base")
        .with_working_dir(&dir)
        .build()
        .await;

    // 项目索引不存在时不出现 Project 行；全局索引存在与否取决于环境，不断言。
    assert!(!prompt.contains("- Project:"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn contract_sections_matrix() {
    // (attachments on, channel-routed) → 两段都在
    let both = contract_sections(true, true);
    assert!(both.contains("# Attachments"));
    assert!(both.contains("# Mentions"));

    // 本地会话：只有 attachments
    let local = contract_sections(true, false);
    assert!(local.contains("# Attachments"));
    assert!(!local.contains("# Mentions"));

    // attachments feature 关闭：channel 会话只有 mentions
    let no_attach = contract_sections(false, true);
    assert!(!no_attach.contains("# Attachments"));
    assert!(no_attach.contains("# Mentions"));

    // 全关：空
    assert_eq!(contract_sections(false, false), "");
}

#[test]
fn contract_sections_append_verbatim() {
    // 每段自带前导空行，直接拼在 base 后即为合法 prompt
    let prompt = format!("base{}", contract_sections(true, true));
    assert!(prompt.starts_with("base\n\n# Attachments"));
    assert!(prompt.contains("\n\n# Mentions"));
}

#[tokio::test]
async fn skill_section_indexes_only_top_level_skills() {
    // loader → prompt 链路：套件子 skill 不进索引，由父级 SKILL.md 路由。
    let dir = temp_dir("skills-top-level");
    let skills_dir = dir.join("skills");
    let suite = skills_dir.join("suite");
    std::fs::create_dir_all(suite.join("child")).unwrap();
    std::fs::write(
        suite.join("SKILL.md"),
        "---\ndescription: parent router\n---\n",
    )
    .unwrap();
    std::fs::write(
        suite.join("child/SKILL.md"),
        "---\ndescription: nested child\n---\n",
    )
    .unwrap();

    let skills = crate::skill::SkillScanner::new(vec![skills_dir])
        .load_all()
        .await;
    let prompt = SystemPromptBuilder::new()
        .base_prompt("base")
        .with_skills(&skills)
        .build()
        .await;

    assert!(prompt.contains("name: suite\n"));
    assert!(prompt.contains("parent router"));
    assert!(!prompt.contains("suite:child"));
    assert!(!prompt.contains("nested child"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn session_rules_section_reads_verbatim_or_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&dir).unwrap();

    // No file → no injection (zero prompt noise).
    assert!(crate::prompt::session_rules_section(tmp.path(), "sess_x")
        .await
        .is_none());

    // Empty/whitespace-only file → no injection.
    std::fs::write(dir.join("sess_empty.md"), "  \n ").unwrap();
    assert!(
        crate::prompt::session_rules_section(tmp.path(), "sess_empty")
            .await
            .is_none()
    );

    // Present → verbatim content (outer whitespace trimmed).
    std::fs::write(dir.join("sess_a.md"), "用中文回答。\n第二行规则\n").unwrap();
    assert_eq!(
        crate::prompt::session_rules_section(tmp.path(), "sess_a")
            .await
            .as_deref(),
        Some("用中文回答。\n第二行规则")
    );

    // Oversize → truncated at a char boundary with a marker.
    let big = "规".repeat(5000); // 15000 bytes > 4096
    std::fs::write(dir.join("sess_big.md"), &big).unwrap();
    let out = crate::prompt::session_rules_section(tmp.path(), "sess_big")
        .await
        .unwrap();
    assert!(out.ends_with("(truncated)"));
    assert!(out.len() <= crate::prompt::SESSION_RULES_MAX_BYTES + 20);
}

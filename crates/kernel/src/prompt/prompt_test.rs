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

#[test]
fn watch_section_states_the_contract() {
    let section = crate::prompt::watch_section("feishu", "oc_1", false);
    // Sole-listener identity + the chat it watches.
    assert!(section.contains("sole listener"));
    assert!(section.contains("oc_1"));
    assert!(section.contains("feishu"));
    // The channel delivers nothing; the skill is the only voice.
    assert!(section.contains("delivers NOTHING"));
    assert!(section.contains("skill"));
    // Mentions are the agent's own to answer (or not); silence default otherwise.
    assert!(section.contains("usually respond"));
    assert!(section.contains("silence is the default"));
    assert!(section.contains("no separate conversation session"));
    // Commands are never mirrored — the intake clause must not overpromise.
    assert!(section.contains("non-command message"));
    assert!(!section.contains("PAUSED"));
}

#[test]
fn watch_section_paused_variant_drops_intake_promise() {
    let paused = crate::prompt::watch_section("feishu", "oc_1", true);
    assert!(paused.contains("PAUSED"));
    assert!(!paused.contains("non-command message"));
    // Delivery suppression still stated — a paused observer must not
    // believe its text gets posted either.
    assert!(paused.contains("delivers NOTHING"));
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

#[tokio::test]
async fn session_rules_section_rejects_unsafe_session_ids() {
    // Session ids may come from client RPC strings: anything outside
    // [A-Za-z0-9_-] must never be used to build a path.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&dir).unwrap();

    // 路径穿越：即便目标文件真实存在，也绝不能被读进 prompt。
    // （rules 目录上两级即 tmp 根：`sessions/rules/../../secret.md`。）
    std::fs::write(tmp.path().join("secret.md"), "TOP SECRET").unwrap();
    for evil in [
        "../../secret",
        "../rules/sess_a",
        "../../etc/passwd",
        "..",
        "a/b",
        "a\\b",
        "a:b",
        "a.b",
        "a b",
        "",
    ] {
        assert!(
            crate::prompt::session_rules_section(tmp.path(), evil)
                .await
                .is_none(),
            "id {evil:?} must be rejected"
        );
    }

    // 正常 ULID 风格 id（sess_/sub_ 前缀 + Crockford base32）不受影响。
    std::fs::write(dir.join("sess_01J8QK7V3X.md"), "规则").unwrap();
    assert_eq!(
        crate::prompt::session_rules_section(tmp.path(), "sess_01J8QK7V3X")
            .await
            .as_deref(),
        Some("规则")
    );
}

#[tokio::test]
async fn session_rules_section_treats_non_utf8_as_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sess_bin.md"), [0x66, 0x80, 0x81, 0xfe]).unwrap();

    assert!(crate::prompt::session_rules_section(tmp.path(), "sess_bin")
        .await
        .is_none());
}

#[tokio::test]
async fn compose_system_prompt_sub_agent_gets_rules_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sub_1.md"), "别跟用户抢话。").unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: None,
        is_sub_agent: true,
        enable_attachments: true,
        channel_routed: false,
        watch: None,
        data_dir: tmp.path(),
        session_id: "sub_1",
    })
    .await;

    // 无契约段（输出不出 parent），RULE.md 原文照注。
    assert_eq!(prompt, "BASE\n\n别跟用户抢话。");
}

#[tokio::test]
async fn compose_system_prompt_main_session_full_stack() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("watch_oc_1.md"), "群里少说话。").unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: None,
        is_sub_agent: false,
        enable_attachments: true,
        channel_routed: true,
        watch: Some(("feishu", "oc_1", false)),
        data_dir: tmp.path(),
        session_id: "watch_oc_1",
    })
    .await;

    assert!(prompt.starts_with("BASE\n\n# Attachments"));
    assert!(prompt.contains("\n\n# Mentions"));
    assert!(prompt.contains("\n\n# Watch mode"));
    assert!(prompt.ends_with("\n\n群里少说话。"));
}

#[tokio::test]
async fn compose_system_prompt_template_wins_rules_still_appended() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sub_t.md"), "模板也有规矩。").unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: Some("TEMPLATE".into()),
        is_sub_agent: true,
        enable_attachments: true,
        channel_routed: false,
        watch: None,
        data_dir: tmp.path(),
        session_id: "sub_t",
    })
    .await;

    assert_eq!(prompt, "TEMPLATE\n\n模板也有规矩。");
}

#[tokio::test]
async fn compose_system_prompt_without_rules_file_leaves_prompt_untouched() {
    let tmp = tempfile::TempDir::new().unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: None,
        is_sub_agent: true,
        enable_attachments: false,
        channel_routed: false,
        watch: None,
        data_dir: tmp.path(),
        session_id: "ghost",
    })
    .await;

    assert_eq!(prompt, "BASE");
}

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
    let section = crate::prompt::watch_section("feishu", "oc_1");
    // The mode + the chat it watches (commands never mirror).
    assert!(section.contains("watch mode"));
    assert!(section.contains("oc_1"));
    assert!(section.contains("feishu"));
    assert!(section.contains("non-command message"));
    // The hard boundary: nothing it outputs reaches the chat.
    assert!(section.contains("never posted"));
    // The only way out: speak via skill — no operational hints (the
    // skill list is in the prompt, headers carry the anchors), and no
    // scripted defaults for when to speak.
    assert!(section.contains("speak via skill"));
    assert!(!section.contains("[msg_id:"));
    assert!(!section.contains("usually respond"));
    assert!(!section.contains("silence"));
}

#[tokio::test]
async fn channel_rules_section_reads_verbatim_or_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("channels").join("rules");
    std::fs::create_dir_all(&dir).unwrap();

    // No file → no injection (zero prompt noise).
    assert!(crate::prompt::channel_rules_section(tmp.path(), "oc_x")
        .await
        .is_none());

    // Empty/whitespace-only file → no injection.
    std::fs::write(dir.join("oc_empty.md"), "  \n ").unwrap();
    assert!(crate::prompt::channel_rules_section(tmp.path(), "oc_empty")
        .await
        .is_none());

    // Present → verbatim content (outer whitespace trimmed).
    std::fs::write(dir.join("oc_a.md"), "用中文回答。\n第二行规则\n").unwrap();
    assert_eq!(
        crate::prompt::channel_rules_section(tmp.path(), "oc_a")
            .await
            .as_deref(),
        Some("用中文回答。\n第二行规则")
    );

    // Oversize → truncated at a char boundary with a marker.
    let big = "规".repeat(5000); // 15000 bytes > 4096
    std::fs::write(dir.join("oc_big.md"), &big).unwrap();
    let out = crate::prompt::channel_rules_section(tmp.path(), "oc_big")
        .await
        .unwrap();
    assert!(out.ends_with("(truncated)"));
    assert!(out.len() <= crate::prompt::SESSION_RULES_MAX_BYTES + 20);
}

#[tokio::test]
async fn channel_rules_section_rejects_unsafe_chat_ids() {
    // Chat ids may come from platform payloads: anything outside
    // [A-Za-z0-9_-] must never be used to build a path.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("channels").join("rules");
    std::fs::create_dir_all(&dir).unwrap();

    // 路径穿越：即便目标文件真实存在，也绝不能被读进 prompt。
    // （rules 目录上两级即 tmp 根：`channels/rules/../../secret.md`。）
    std::fs::write(tmp.path().join("secret.md"), "TOP SECRET").unwrap();
    for evil in [
        "../../secret",
        "../rules/oc_a",
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
            crate::prompt::channel_rules_section(tmp.path(), evil)
                .await
                .is_none(),
            "id {evil:?} must be rejected"
        );
    }

    // 正常平台 chat id（oc_ 前缀 / telegram 数字含负号）不受影响。
    std::fs::write(dir.join("oc_01J8QK7V3X.md"), "规则").unwrap();
    assert_eq!(
        crate::prompt::channel_rules_section(tmp.path(), "oc_01J8QK7V3X")
            .await
            .as_deref(),
        Some("规则")
    );
    std::fs::write(dir.join("-100123456.md"), "tg 规则").unwrap();
    assert_eq!(
        crate::prompt::channel_rules_section(tmp.path(), "-100123456")
            .await
            .as_deref(),
        Some("tg 规则")
    );
}

#[tokio::test]
async fn channel_rules_section_treats_non_utf8_as_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("channels").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("oc_bin.md"), [0x66, 0x80, 0x81, 0xfe]).unwrap();

    assert!(crate::prompt::channel_rules_section(tmp.path(), "oc_bin")
        .await
        .is_none());
}

#[tokio::test]
async fn compose_system_prompt_without_rules_chat_ignores_rules_file() {
    // Sub-agents and local sessions get no `rules_chat` from the
    // conductor: even when a rules file exists it must not leak in.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("channels").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("oc_1.md"), "别跟用户抢话。").unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: None,
        is_sub_agent: true,
        enable_attachments: true,
        channel_routed: false,
        watch: None,
        rules_chat: None,
        rules_session: None,
        data_dir: tmp.path(),
    })
    .await;

    assert_eq!(prompt, "BASE");
}

#[tokio::test]
async fn compose_system_prompt_main_session_full_stack() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("channels").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("oc_1.md"), "群里少说话。").unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: None,
        is_sub_agent: false,
        enable_attachments: true,
        channel_routed: true,
        watch: Some(("feishu", "oc_1")),
        rules_chat: Some("oc_1"),
        rules_session: None,
        data_dir: tmp.path(),
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
    let dir = tmp.path().join("channels").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("oc_t.md"), "模板也有规矩。").unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: Some("TEMPLATE".into()),
        is_sub_agent: true,
        enable_attachments: true,
        channel_routed: false,
        watch: None,
        rules_chat: Some("oc_t"),
        rules_session: None,
        data_dir: tmp.path(),
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
        is_sub_agent: false,
        enable_attachments: false,
        channel_routed: false,
        watch: None,
        rules_chat: Some("oc_ghost"),
        rules_session: None,
        data_dir: tmp.path(),
    })
    .await;

    assert_eq!(prompt, "BASE");
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

    // Present → verbatim content (outer whitespace trimmed).
    std::fs::write(dir.join("sess_a.md"), "只回答本话题。\n").unwrap();
    assert_eq!(
        crate::prompt::session_rules_section(tmp.path(), "sess_a")
            .await
            .as_deref(),
        Some("只回答本话题。")
    );
}

#[tokio::test]
async fn session_rules_section_rejects_unsafe_ids() {
    // Session ids are ULIDs, but the path builder must never trust that:
    // anything outside [A-Za-z0-9_-] gets no file.
    let tmp = tempfile::TempDir::new().unwrap();
    for evil in ["../../etc/passwd", "..", "a/b", "a.md", "a b", ""] {
        assert!(
            crate::prompt::session_rules_section(tmp.path(), evil)
                .await
                .is_none(),
            "id {evil:?} must be rejected"
        );
    }
}

#[tokio::test]
async fn compose_system_prompt_session_rules_speak_after_channel_rules() {
    let tmp = tempfile::TempDir::new().unwrap();
    let chat_dir = tmp.path().join("channels").join("rules");
    let session_dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&chat_dir).unwrap();
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(chat_dir.join("oc_1.md"), "群规则").unwrap();
    std::fs::write(session_dir.join("sess_1.md"), "会话规则").unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: None,
        is_sub_agent: false,
        enable_attachments: false,
        channel_routed: false,
        watch: None,
        rules_chat: Some("oc_1"),
        rules_session: Some("sess_1"),
        data_dir: tmp.path(),
    })
    .await;

    // Both layers appended, channel first, session last (the narrower
    // scope wins conflicts).
    let chat_at = prompt.find("群规则").expect("channel rules appended");
    let session_at = prompt.find("会话规则").expect("session rules appended");
    assert!(
        chat_at < session_at,
        "session rules must speak last: {prompt}"
    );
    assert!(prompt.ends_with("会话规则"));
}

#[tokio::test]
async fn compose_system_prompt_without_rules_session_ignores_session_file() {
    // Sub-agents get `rules_session: None` from the conductor: even when
    // a session rules file exists it must not leak in.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sess_sub.md"), "不该出现。").unwrap();

    let prompt = compose_system_prompt(SystemPromptParts {
        base_prompt: "BASE".into(),
        template_body: None,
        is_sub_agent: true,
        enable_attachments: false,
        channel_routed: false,
        watch: None,
        rules_chat: None,
        rules_session: None,
        data_dir: tmp.path(),
    })
    .await;

    assert_eq!(prompt, "BASE");
}

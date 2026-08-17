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

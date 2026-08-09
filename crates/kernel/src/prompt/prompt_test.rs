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

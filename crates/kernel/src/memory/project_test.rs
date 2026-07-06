use super::*;

use std::io::Write;
use tempfile::TempDir;

#[tokio::test]
async fn test_load_claude_md() {
    let temp = TempDir::new().unwrap();
    let mut file = std::fs::File::create(temp.path().join("CLAUDE.md")).unwrap();
    writeln!(file, "# Test Instructions").unwrap();

    let files = load(temp.path()).await.unwrap();
    assert_eq!(files.len(), 1);
}

#[tokio::test]
async fn test_build_system_prompt() {
    let temp = TempDir::new().unwrap();
    let mut file = std::fs::File::create(temp.path().join("CLAUDE.md")).unwrap();
    writeln!(file, "Be helpful").unwrap();

    let files = load(temp.path()).await.unwrap();
    let prompt = files.build_system_prompt("You are a coding assistant.");
    assert!(prompt.contains("You are a coding assistant."));
    assert!(prompt.contains("Be helpful"));
}

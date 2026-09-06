//! tools/ 目录外挂的扫描与执行测试。

use std::io::Write as _;

use super::{scan, ENTRY_FILE, MANIFEST_FILE};
use crate::tools::ToolExecCtx;

fn write_tool(
    data_dir: &std::path::Path,
    name: &str,
    manifest: &str,
    run_body: Option<&str>,
    exec: bool,
) {
    let dir = data_dir.join("tools").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(MANIFEST_FILE), manifest).unwrap();
    if let Some(body) = run_body {
        let run = dir.join(ENTRY_FILE);
        let mut f = std::fs::File::create(&run).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        write!(f, "{body}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = if exec { 0o755 } else { 0o644 };
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }
}

const MANIFEST: &str = r#"{"desc":"demo tool","schema":{"type":"object"}}"#;

fn ctx(dir: &std::path::Path) -> ToolExecCtx<'static> {
    ToolExecCtx::new("call_1", dir, "sess_test")
}

fn text_of(out: &crate::types::ToolOutput) -> String {
    out.contents
        .iter()
        .map(|b| match b {
            crate::types::ToolOutputBlock::Text { text } => text.clone(),
            crate::types::ToolOutputBlock::Image { .. } => String::new(),
        })
        .collect()
}

#[tokio::test]
async fn scan_missing_dir_is_empty() {
    let dir = tempfile::TempDir::new().unwrap();
    assert!(scan(dir.path()).await.is_empty());
}

#[tokio::test]
async fn scan_picks_valid_tool() {
    let dir = tempfile::TempDir::new().unwrap();
    write_tool(dir.path(), "demo", MANIFEST, Some("echo hi\n"), true);
    let tools = scan(dir.path()).await;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "demo");
    assert_eq!(tools[0].desc(), "demo tool");
    assert_eq!(tools[0].level(), Some(crate::permission::Level::Caution));
}

#[tokio::test]
async fn scan_skips_invalid_entries() {
    let dir = tempfile::TempDir::new().unwrap();
    write_tool(dir.path(), ".hidden", MANIFEST, Some("true\n"), true);
    write_tool(dir.path(), "9bad", MANIFEST, Some("true\n"), true);
    write_tool(dir.path(), "bad-json", "{not json", Some("true\n"), true);
    write_tool(dir.path(), "no-run", MANIFEST, None, true);
    write_tool(dir.path(), "off", MANIFEST, Some("true\n"), false);
    assert!(scan(dir.path()).await.is_empty());
}

#[tokio::test]
async fn scan_manifest_level_and_timeout_clamp() {
    let dir = tempfile::TempDir::new().unwrap();
    write_tool(
        dir.path(),
        "danger",
        r#"{"desc":"d","schema":{},"level":"dangerous","timeout_secs":99999}"#,
        Some("true\n"),
        true,
    );
    let tools = scan(dir.path()).await;
    assert_eq!(tools[0].level(), Some(crate::permission::Level::Dangerous));
}

#[tokio::test]
async fn exec_success_returns_stdout_and_delivers_contract() {
    let dir = tempfile::TempDir::new().unwrap();
    // 回显 stdin + 关键 env，验证契约一次到位。
    write_tool(
        dir.path(),
        "probe",
        MANIFEST,
        Some(
            "cat\n\
             printf 'EVENT=%s SID=%s STATE=%s\\n' \"$YOMI_EVENT\" \"$YOMI_SESSION_ID\" \"$YOMI_STATE_DIR\" >&2\n",
        ),
        true,
    );
    let tools = scan(dir.path()).await;
    let out = tools[0]
        .exec(serde_json::json!({"x": 1}), ctx(dir.path()))
        .await
        .unwrap();
    let text = text_of(&out);
    assert!(
        text.contains(r#""tool_name":"probe""#),
        "stdin payload: {text}"
    );
    assert!(text.contains(r#""args":{"x":1}"#), "stdin payload: {text}");
    assert!(text.contains(r#""event":"tool""#), "stdin payload: {text}");
    // state 目录已惰性创建。
    assert!(dir.path().join("state/tools/probe").is_dir());
}

#[tokio::test]
async fn exec_failure_is_error_with_prefix_and_stderr() {
    let dir = tempfile::TempDir::new().unwrap();
    write_tool(
        dir.path(),
        "boom",
        MANIFEST,
        Some("echo why >&2\nexit 4\n"),
        true,
    );
    let tools = scan(dir.path()).await;
    let out = tools[0]
        .exec(serde_json::json!({}), ctx(dir.path()))
        .await
        .unwrap();
    assert!(out.is_error);
    let text = text_of(&out);
    assert!(text.contains("[ext:boom]"), "{text}");
    assert!(text.contains("why"), "{text}");
}

#[tokio::test]
async fn exec_timeout_is_error() {
    let dir = tempfile::TempDir::new().unwrap();
    write_tool(
        dir.path(),
        "slow",
        r#"{"desc":"d","schema":{},"timeout_secs":1}"#,
        Some("sleep 60\n"),
        true,
    );
    let tools = scan(dir.path()).await;
    let out = tools[0]
        .exec(serde_json::json!({}), ctx(dir.path()))
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(text_of(&out).contains("timed out"));
}

#[tokio::test]
async fn exec_stdout_respects_budget() {
    let dir = tempfile::TempDir::new().unwrap();
    write_tool(
        dir.path(),
        "chatty",
        MANIFEST,
        Some("dd if=/dev/zero bs=1024 count=100 2>/dev/null | tr '\\0' 'a'\n"),
        true,
    );
    let tools = scan(dir.path()).await;
    let mut c = ctx(dir.path());
    c.max_tool_output_length = 1000;
    let out = tools[0].exec(serde_json::json!({}), c).await.unwrap();
    assert!(text_of(&out).len() <= 1100);
}

/// scan 之后 run 被删：引擎 `SpawnError` → fail-closed 的 tool error
///（`[ext:]` 前缀 + spawn failed），不是 panic、不是挂起。
#[tokio::test]
async fn exec_entry_deleted_after_scan_is_tool_error() {
    let dir = tempfile::TempDir::new().unwrap();
    write_tool(dir.path(), "gone", MANIFEST, Some("echo hi\n"), true);
    let tools = scan(dir.path()).await;
    std::fs::remove_file(dir.path().join("tools/gone").join(ENTRY_FILE)).unwrap();
    let out = tools[0]
        .exec(serde_json::json!({}), ctx(dir.path()))
        .await
        .unwrap();
    assert!(out.is_error);
    let text = text_of(&out);
    assert!(text.contains("[ext:gone]"), "{text}");
    assert!(text.contains("spawn failed"), "{text}");
}

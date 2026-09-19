use super::{
    extract_log_body, format_background_result, format_log_note, format_sync_output, ShellTool,
};
use crate::tools::format_shell_message;
use std::ffi::OsStr;
use std::path::Path;

#[test]
fn sync_output_single_stream_is_bare() {
    // Only one non-empty stream: no label, no extra newline.
    assert_eq!(format_sync_output("hello", "", 1000), "hello");
    assert_eq!(format_sync_output("", "warn", 1000), "warn");
    assert_eq!(format_sync_output("", "", 1000), "");
}

#[test]
fn sync_output_both_streams_labeled() {
    assert_eq!(
        format_sync_output("hello", "warn", 1000),
        "[stdout]\nhello\n\n[stderr]\nwarn"
    );
}

#[test]
fn sync_output_truncates_over_budget_stream() {
    let out = "a".repeat(5000);
    let result = format_sync_output(&out, "", 200);
    assert!(result.contains("[truncated]"));
    assert!(result.len() < 5000);
}

#[test]
fn background_shell_messages_include_task_id_once() {
    let result = format_background_result(
        Ok::<_, &str>((0, false, false)),
        Path::new("/tmp/task.log"),
        "hello",
        1000,
    );

    assert_eq!(
        format_shell_message("sh_123", result),
        "[From Shell: sh_123] [Task completed] Exit code: 0 · Log file: /tmp/task.log\n[output]\nhello"
    );
}

#[test]
fn format_background_success_result() {
    assert_eq!(
        format_background_result(
            Ok::<_, &str>((0, false, false)),
            Path::new("task.log"),
            "done",
            1000,
        ),
        "[Task completed] Exit code: 0 · Log file: task.log\n[output]\ndone"
    );
    assert_eq!(
        format_background_result(
            Ok::<_, &str>((7, false, false)),
            Path::new("task.log"),
            "boom",
            1000,
        ),
        "[Task failed] Exit code: 7 · Log file: task.log\n[output]\nboom"
    );
}

#[test]
fn format_background_cancelled_result() {
    assert_eq!(
        format_background_result(
            Ok::<_, &str>((-1, false, true)),
            Path::new("task.log"),
            "partial",
            1000,
        ),
        "[Task cancelled] · Log file: task.log\n[output]\npartial"
    );
}

#[test]
fn format_background_timeout_result() {
    assert_eq!(
        format_background_result(
            Ok::<_, &str>((-1, true, false)),
            Path::new("task.log"),
            "partial",
            1000,
        ),
        "[Task timed_out] · Log file: task.log\n[output]\npartial"
    );
}

#[test]
fn format_background_error_result() {
    assert_eq!(
        format_background_result(Err("process unavailable"), Path::new("task.log"), "", 1000,),
        "[Task failed] Error: process unavailable · Log file: task.log\n[No output]"
    );
}

#[test]
fn format_background_empty_output() {
    assert_eq!(
        format_background_result(
            Ok::<_, &str>((0, false, false)),
            Path::new("task.log"),
            "",
            1000,
        ),
        "[Task completed] Exit code: 0 · Log file: task.log\n[No output]"
    );
}

#[test]
fn format_background_truncates_long_output() {
    let output = "a".repeat(5000);
    let result = format_background_result(
        Ok::<_, &str>((0, false, false)),
        Path::new("task.log"),
        &output,
        200,
    );

    assert!(result.contains("[truncated]"));
    assert!(result.starts_with("[Task completed] Exit code: 0 · Log file: task.log\n[output]\n"));
    assert!(result.len() <= 250);
}

#[test]
fn extract_log_body_strips_header_and_footer() {
    let log = "# Command: echo hi\n# Timeout: 5s\n\nhello\n[stderr] warn\n\n# Task timed out after 5s\n\n# Exit: -1\n";
    assert_eq!(extract_log_body(log), "hello\n[stderr] warn");
}

#[test]
fn extract_log_body_empty_output() {
    let log = "# Command: true\n\n\n# Exit: 0\n";
    assert_eq!(extract_log_body(log), "");
}

#[test]
fn extract_log_body_keeps_inner_hash_lines() {
    let log = "# Command: cat README.md\n\n# Title\nsome text\n\n# Exit: 0\n";
    assert_eq!(extract_log_body(log), "# Title\nsome text");
}

#[test]
fn build_command_disables_interactive_prompters() {
    let cmd = ShellTool::build_command(
        "true",
        Path::new("/tmp"),
        "sess_test",
        Some(Path::new("/data")),
    );
    let env = |key: &str| {
        cmd.as_std()
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    };

    assert_eq!(env("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    assert_eq!(env("SSH_ASKPASS_REQUIRE").as_deref(), Some("never"));
    assert_eq!(env("GIT_PAGER").as_deref(), Some("cat"));
    assert_eq!(env("YOMI_SESSION_ID").as_deref(), Some("sess_test"));
    assert_eq!(env("YOMI_DATA_DIR").as_deref(), Some("/data"));

    // GIT_SSH_COMMAND is only injected when the user hasn't set their own.
    match std::env::var("GIT_SSH_COMMAND") {
        Ok(user_value) => {
            assert!(
                env("GIT_SSH_COMMAND").is_none() || env("GIT_SSH_COMMAND") == Some(user_value),
                "user GIT_SSH_COMMAND must not be overridden"
            );
        }
        Err(_) => {
            assert_eq!(
                env("GIT_SSH_COMMAND").as_deref(),
                Some("ssh -oBatchMode=yes")
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn spawned_command_cannot_open_controlling_tty() {
    // setsid（由 spawn_in_new_tree 建立）detaches the child from the
    // controlling terminal, so opening /dev/tty fails — this is what makes
    // sudo/ssh/gpg fail fast instead of blocking on a hidden password prompt.
    let mut cmd =
        ShellTool::build_command("echo x < /dev/tty", Path::new("/tmp"), "sess_test", None);
    let (child, _tree) = crate::utils::process::spawn_in_new_tree(&mut cmd).unwrap();
    let output = child.wait_with_output().await.unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("/dev/tty"), "unexpected stderr: {stderr}");
}

#[cfg(unix)]
#[tokio::test]
async fn spawned_command_reads_eof_on_stdin() {
    let mut cmd = ShellTool::build_command(
        "read line; echo \"got:[$line]\"",
        Path::new("/tmp"),
        "sess_test",
        None,
    );
    let (child, _tree) = crate::utils::process::spawn_in_new_tree(&mut cmd).unwrap();
    let output = child.wait_with_output().await.unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "got:[]");
}

#[cfg(unix)]
#[tokio::test]
async fn spawned_command_runs_normally() {
    let mut cmd = ShellTool::build_command("echo hello", Path::new("/tmp"), "sess_test", None);
    let (child, _tree) = crate::utils::process::spawn_in_new_tree(&mut cmd).unwrap();
    let output = child.wait_with_output().await.unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
}

#[test]
fn log_note_lists_files_only_when_present() {
    assert_eq!(format_log_note(&[]), "");
    let note = format_log_note(&[(std::path::PathBuf::from("/tmp/yomi_sh-x_stdout.log"), 40960)]);
    assert!(
        note.contains("Log file: /tmp/yomi_sh-x_stdout.log (40960 bytes)"),
        "{note}"
    );
    assert!(note.contains("truncated above"), "{note}");
}

#[test]
fn desc_is_platform_independent() {
    let tool = ShellTool::new();
    let desc = crate::tools::Tool::desc(&tool);
    // 解释器路径/种类走 base prompt 的 Environment 段，desc 保持静态。
    assert!(desc.contains("shell command"), "desc: {desc}");
    assert!(desc.contains("background"), "desc: {desc}");
    assert!(!desc.contains("interpreter:"), "desc: {desc}");
    assert!(
        desc.contains(crate::tools::ASYNC_LAUNCH_GUIDE),
        "desc: {desc}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn exec_sync_image_marker_attaches_image() {
    // 协议见 utils::image_marker：marker 独占一行即随结果回传图片，
    // 相对路径按本次调用的工作目录解析。
    let dir = tempfile::TempDir::new().unwrap();
    let png = crate::utils::image::test_utils::noisy_png(16, 16);
    tokio::fs::write(dir.path().join("shot.png"), &png)
        .await
        .unwrap();

    let tool = ShellTool::new();
    let ctx = crate::tools::ToolExecCtx::new("call_1", dir.path(), "sess_test");
    let out = crate::tools::Tool::exec(
        &tool,
        serde_json::json!({"command": "echo yomi://image/shot.png"}),
        ctx,
    )
    .await
    .unwrap();

    assert!(out.success(), "{}", out.text_content());
    let text = out.text_content();
    let abs = dir.path().join("shot.png");
    assert!(
        text.contains(&format!(
            "[Image: {} | Size: {} bytes]",
            abs.display(),
            png.len()
        )),
        "{text}"
    );
    assert!(text.contains("exit code: 0"), "{text}");
    assert!(out.contents.iter().any(|b| b.is_image()), "{text}");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_sync_without_marker_has_no_image_block() {
    let dir = tempfile::TempDir::new().unwrap();
    let tool = ShellTool::new();
    let ctx = crate::tools::ToolExecCtx::new("call_1", dir.path(), "sess_test");
    let out = crate::tools::Tool::exec(&tool, serde_json::json!({"command": "echo plain"}), ctx)
        .await
        .unwrap();

    assert!(out.success());
    assert!(!out.contents.iter().any(|b| b.is_image()));
    assert!(out.text_content().contains("plain"));
}

#[cfg(unix)]
#[tokio::test]
async fn exec_sync_marker_survives_text_truncation() {
    // marker 提取在文本截断之前：输出噪声把文本顶到截断，行尾 marker
    // 仍应交出图片 block。
    let dir = tempfile::TempDir::new().unwrap();
    let png = crate::utils::image::test_utils::noisy_png(16, 16);
    tokio::fs::write(dir.path().join("shot.png"), &png)
        .await
        .unwrap();

    let tool = ShellTool::new();
    let mut ctx = crate::tools::ToolExecCtx::new("call_1", dir.path(), "sess_test");
    ctx.max_tool_output_length = 1000;
    let out = crate::tools::Tool::exec(
        &tool,
        serde_json::json!({"command": "python3 -c \"print('a'*5000)\"; echo yomi://image/shot.png"}),
        ctx,
    )
    .await
    .unwrap();

    assert!(out.success(), "{}", out.text_content());
    assert!(
        out.contents.iter().any(|b| b.is_image()),
        "{}",
        out.text_content()
    );
    assert!(
        out.text_content().len() <= 1200,
        "{}",
        out.text_content().len()
    );
}

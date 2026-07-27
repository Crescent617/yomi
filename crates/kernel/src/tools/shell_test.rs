use super::{extract_log_body, format_background_result, format_sync_output};
use crate::tools::format_shell_message;
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

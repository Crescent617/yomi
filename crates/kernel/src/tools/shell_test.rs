use super::{format_background_result, strip_ansi};
use crate::tools::format_shell_message;
use std::path::Path;

#[test]
fn background_shell_messages_include_task_id_once() {
    let result =
        format_background_result(Ok::<_, &str>((0, false, false)), Path::new("/tmp/task.log"));

    assert_eq!(
        format_shell_message("sh_123", result),
        "[From Shell: sh_123] [Task completed] Exit code: 0 · Output: /tmp/task.log"
    );
}

#[test]
fn format_background_success_result() {
    assert_eq!(
        format_background_result(Ok::<_, &str>((0, false, false)), Path::new("task.log")),
        "[Task completed] Exit code: 0 · Output: task.log"
    );
    assert_eq!(
        format_background_result(Ok::<_, &str>((7, false, false)), Path::new("task.log")),
        "[Task failed] Exit code: 7 · Output: task.log"
    );
}

#[test]
fn format_background_cancelled_result() {
    assert_eq!(
        format_background_result(Ok::<_, &str>((-1, false, true)), Path::new("task.log")),
        "[Task cancelled] Partial output: task.log"
    );
}

#[test]
fn format_background_timeout_result() {
    assert_eq!(
        format_background_result(Ok::<_, &str>((-1, true, false)), Path::new("task.log")),
        "[Task timed_out] Partial output: task.log"
    );
}

#[test]
fn format_background_error_result() {
    assert_eq!(
        format_background_result(Err("process unavailable"), Path::new("task.log")),
        "[Task failed] Error: process unavailable · Output: task.log"
    );
}

#[test]
fn test_strip_ansi_colors() {
    // Red text
    let input = "\x1b[31mred text\x1b[0m";
    assert_eq!(strip_ansi(input), "red text");

    // Green text
    let input = "\x1b[32mgreen text\x1b[0m";
    assert_eq!(strip_ansi(input), "green text");

    // Bold + blue
    let input = "\x1b[1;34mbold blue\x1b[0m";
    assert_eq!(strip_ansi(input), "bold blue");
}

#[test]
fn test_strip_ansi_cursor_control() {
    // Clear screen
    let input = "\x1b[2Jcleared";
    assert_eq!(strip_ansi(input), "cleared");

    // Cursor up
    let input = "\x1b[Aup";
    assert_eq!(strip_ansi(input), "up");
}

#[test]
fn test_strip_ansi_mixed_content() {
    let input = "normal \x1b[31mred\x1b[0m normal \x1b[32mgreen\x1b[0m";
    assert_eq!(strip_ansi(input), "normal red normal green");
}

#[test]
fn test_strip_ansi_no_escape() {
    let input = "no escape codes here";
    assert_eq!(strip_ansi(input), "no escape codes here");
}

#[test]
fn test_strip_ansi_empty() {
    assert_eq!(strip_ansi(""), "");
}

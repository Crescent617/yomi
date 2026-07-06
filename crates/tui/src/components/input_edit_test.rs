use super::*;

#[test]
fn test_basic_editing() {
    let mut buf = TextBuffer::new();

    buf.insert_str("hello world");
    assert_eq!(buf.content(), "hello world");
    assert_eq!(buf.cursor_pos(), 11);

    buf.move_to_start();
    assert_eq!(buf.cursor_pos(), 0);

    buf.move_word_right();
    assert_eq!(buf.cursor_pos(), 6); // Start of "world" (after "hello ")

    buf.move_word_right();
    assert_eq!(buf.cursor_pos(), 11); // End of "world"
}

#[test]
fn test_delete_word() {
    let mut buf = TextBuffer::with_content("hello world test");
    buf.move_to_end();

    buf.delete_word_backward();
    assert_eq!(buf.content(), "hello world ");

    buf.delete_word_backward();
    assert_eq!(buf.content(), "hello ");
}

#[test]
fn test_kill_line() {
    let mut buf = TextBuffer::with_content("hello\nworld");

    // Position cursor at end of first line (after "hello")
    buf.set_cursor_pos(5);
    assert_eq!(buf.cursor_pos(), 5);

    // Nothing after cursor on first line, so kill_to_end_of_line does nothing
    buf.kill_to_end_of_line();
    assert_eq!(buf.content(), "hello\nworld");

    // Move to start of line and kill - already at start of line, does nothing
    buf.move_to_start_of_line();
    assert_eq!(buf.cursor_pos(), 0);
    buf.kill_to_start_of_line();
    assert_eq!(buf.content(), "hello\nworld");

    // Move to end of first line and kill to start
    buf.move_to_end_of_line();
    assert_eq!(buf.cursor_pos(), 5); // End of "hello"
    buf.kill_to_start_of_line();
    assert_eq!(buf.content(), "\nworld");
}

#[test]
fn test_kill_to_start_fallback_to_backspace() {
    let mut buf = TextBuffer::with_content("line1\nline2");

    // Cursor at start of second line
    buf.set_cursor_pos(6);
    assert_eq!(buf.cursor_pos(), 6);

    // kill_to_start_of_line should fall back to backspace (delete newline)
    buf.kill_to_start_of_line();
    assert_eq!(buf.content(), "line1line2");
    assert_eq!(buf.cursor_pos(), 5); // At end of "line1"
}

#[test]
fn test_delete_word_backward_at_line_start() {
    let mut buf = TextBuffer::with_content("line1\nline2");

    // Cursor at start of second line
    buf.set_cursor_pos(6);
    assert_eq!(buf.cursor_pos(), 6);

    // delete_word_backward should fall back to backspace (delete newline)
    buf.delete_word_backward();
    assert_eq!(buf.content(), "line1line2");
    assert_eq!(buf.cursor_pos(), 5);
}

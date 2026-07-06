use super::strip_ansi;

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

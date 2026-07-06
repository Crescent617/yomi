use super::*;

#[test]
fn test_input_selection_normalized() {
    let sel = InputSelection { start: 10, end: 5 };
    let norm = sel.normalized();
    assert_eq!(norm.start, 5);
    assert_eq!(norm.end, 10);

    let sel2 = InputSelection { start: 5, end: 10 };
    let norm2 = sel2.normalized();
    assert_eq!(norm2.start, 5);
    assert_eq!(norm2.end, 10);
}

#[test]
fn test_input_selection_contains() {
    let sel = InputSelection { start: 5, end: 10 };
    assert!(sel.contains(5));
    assert!(sel.contains(9));
    assert!(!sel.contains(10));
    assert!(!sel.contains(4));
}

#[test]
fn test_display_col_to_byte_pos_ascii() {
    let text = "hello world";
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 0), 0);
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 5), 5);
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 100), 11);
}

#[test]
fn test_display_col_to_byte_pos_unicode() {
    // CJK characters are typically 2 display columns wide
    let text = "你好世界"; // Each char is 2-3 bytes (UTF-8) and 2 display columns

    // At column 0, should be at start
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 0), 0);

    // At column 1 (middle of first char), should still be at first char
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 1), 0);

    // At column 2 (end of first char), should move to second char
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 2), "你".len());

    // At column 4 (end of second char)
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 4), "你好".len());
}

#[test]
fn test_display_col_to_byte_pos_mixed() {
    // Mixed ASCII and Unicode
    let text = "hi你好";
    // h(0)i(1)你(2-4)好(5-7)
    // Display: h(0)i(1)你(2-3)好(4-5)

    assert_eq!(InputEditor::display_col_to_byte_pos(text, 0), 0); // Before 'h'
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 1), 1); // After 'h', at 'i'
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 2), 2); // After 'i', at '你'
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 3), 2); // Middle of '你'
    assert_eq!(InputEditor::display_col_to_byte_pos(text, 4), 5); // After '你', at '好'
}

#[test]
fn test_select_word_at() {
    let mut input = InputEditor::new();
    input.insert_str("hello world test");

    // Click on 'w' in "world"
    input.select_word_at(6);
    let sel = input.selection().unwrap();
    assert_eq!(sel.start, 6);
    assert_eq!(sel.end, 11); // "world" is 5 chars

    // Click on 'o' in "hello"
    input.select_word_at(4);
    let sel2 = input.selection().unwrap();
    assert_eq!(sel2.start, 0);
    assert_eq!(sel2.end, 5); // "hello" is 5 chars
}

#[test]
fn test_delete_selection() {
    let mut input = InputEditor::new();
    input.insert_str("hello world");
    input.start_selection(0);
    input.update_selection(5); // Select "hello"
    input.delete_selection();

    assert_eq!(input.content(), " world");
    assert_eq!(input.cursor_pos(), 0);
}

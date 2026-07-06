use super::*;

fn make_component() -> InputComponent {
    InputComponent::new()
}

#[test]
fn test_unknown_bracket_not_split() {
    let comp = make_component();
    let blocks = comp.convert_to_content_blocks("hello [world] test");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].as_text(), Some("hello [world] test"));
}

#[test]
fn test_multiple_unknown_brackets_not_split() {
    let comp = make_component();
    let blocks = comp.convert_to_content_blocks("a [b] c [d] e");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].as_text(), Some("a [b] c [d] e"));
}

#[test]
fn test_unclosed_bracket_not_split() {
    let comp = make_component();
    let blocks = comp.convert_to_content_blocks("hello [world");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].as_text(), Some("hello [world"));
}

#[test]
fn test_standalone_brackets_not_split() {
    let comp = make_component();
    let blocks = comp.convert_to_content_blocks("[world]");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].as_text(), Some("[world]"));
}

#[test]
fn test_pasted_text_placeholder_still_works() {
    let mut comp = make_component();
    comp.pasted_contents.insert(
        "[Pasted #1 text]".to_string(),
        "large pasted content".to_string(),
    );
    let blocks = comp.convert_to_content_blocks("hello [Pasted #1 text] world");
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].as_text(), Some("hello "));
    assert_eq!(blocks[1].as_text(), Some("large pasted content"));
    assert_eq!(blocks[2].as_text(), Some(" world"));
}

#[test]
fn test_mixed_known_and_unknown_brackets() {
    let mut comp = make_component();
    comp.pasted_contents
        .insert("[Pasted #1 text]".to_string(), "REPLACED".to_string());
    let blocks = comp.convert_to_content_blocks("a [x] [Pasted #1 text] b");
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].as_text(), Some("a [x] "));
    assert_eq!(blocks[1].as_text(), Some("REPLACED"));
    assert_eq!(blocks[2].as_text(), Some(" b"));
}

#[test]
fn test_empty_input() {
    let comp = make_component();
    let blocks = comp.convert_to_content_blocks("");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].as_text(), Some(""));
}

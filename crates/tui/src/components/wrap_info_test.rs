use super::*;

#[test]
fn test_wrap_info_basic() {
    let mut info = WrapInfo::new();
    let lines = vec![Arc::new(Line::from("Hello World"))];
    info.rebuild(&lines, 5, 0);
    // "Hello World" at width 5 wraps to 3 visual rows
    assert_eq!(info.total_lines(), 3);
    assert_eq!(info.height(0), 3);
    assert_eq!(info.visual_to_logical(0), (0, 0));
    assert_eq!(info.visual_to_logical(2), (0, 2));
    assert_eq!(info.visual_to_logical(3), (1, 0)); // past last visual row of line 0
    assert_eq!(info.viewport_start(2), (0, 2));
}

#[test]
fn test_wrap_info_cjk() {
    let mut info = WrapInfo::new();
    let lines = vec![Arc::new(Line::from("你好世界"))];
    info.rebuild(&lines, 4, 0);
    // CJK chars are width 2, so 4 chars = 2 visual rows at width 4
    assert_eq!(info.total_lines(), 2);
    assert_eq!(info.height(0), 2);
    let boundaries = info.get_boundaries(0).unwrap();
    assert_eq!(boundaries, &[0, 6]); // 4 CJK chars, 3 bytes each, 2 per row at width 4
    let char_boundaries = info.get_char_boundaries(0).unwrap();
    assert_eq!(char_boundaries, &[0, 2]);
    assert_eq!(info.char_count(0), 4);
    assert_eq!(info.get_span_char_counts(0).unwrap(), &[4]);
}

#[test]
fn test_wrap_info_incremental_rebuild() {
    let mut info = WrapInfo::new();
    let lines = vec![Arc::new(Line::from("Hello"))];
    info.rebuild(&lines, 5, 0);
    assert_eq!(info.total_lines(), 1);

    let lines2 = vec![Arc::new(Line::from("Hello")), Arc::new(Line::from("World"))];
    info.rebuild(&lines2, 5, 1);
    assert_eq!(info.total_lines(), 2);
}

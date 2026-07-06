use super::*;

#[test]
fn test_cell_align_format() {
    assert_eq!(CellAlign::Left.format("hi", 5), "hi   ");
    assert_eq!(CellAlign::Right.format("hi", 5), "   hi");
    assert_eq!(CellAlign::Center.format("hi", 5), "  hi ");
    assert_eq!(CellAlign::Center.format("hi", 4), " hi ");
}

#[test]
fn test_parse_align() {
    assert_eq!(parse_align(":---"), CellAlign::Left);
    assert_eq!(parse_align("---"), CellAlign::Left);
    assert_eq!(parse_align(":--:"), CellAlign::Center);
    assert_eq!(parse_align("---:"), CellAlign::Right);
}

#[test]
fn debug_multi_column() {
    let mut renderer = StreamingTableRenderer::new();

    renderer.start_table();
    renderer.start_head();
    renderer.start_row();

    renderer.start_cell();
    renderer.append_text("Name");
    renderer.end_cell();

    renderer.start_cell();
    renderer.append_text("Status");
    renderer.end_cell();

    renderer.start_cell();
    renderer.append_text("Size");
    renderer.end_cell();

    renderer.end_row();
    renderer.end_head();

    // Separator
    renderer.start_row();
    renderer.start_cell();
    renderer.append_text("------");
    renderer.end_cell();
    renderer.start_cell();
    renderer.append_text("--------");
    renderer.end_cell();
    renderer.start_cell();
    renderer.append_text("------");
    renderer.end_cell();
    renderer.end_row();

    // Data row
    renderer.start_row();
    renderer.start_cell();
    renderer.append_text("file.txt");
    renderer.end_cell();
    renderer.start_cell();
    renderer.append_text("done");
    renderer.end_cell();
    renderer.start_cell();
    renderer.append_text("1.5KB");
    renderer.end_cell();
    renderer.end_row();

    // Debug
    println!("column_count: {:?}", renderer.column_count);
    println!("rows.len(): {}", renderer.rows.len());
    for (i, row) in renderer.rows.iter().enumerate() {
        println!("row[{}]: {:?} cells: {:?}", i, row.is_header, row.cells);
    }
    println!("current_row: {:?}", renderer.current_row);
    println!("current_cell: {:?}", renderer.current_cell);
    println!("aligns: {:?}", renderer.aligns);

    let lines = renderer.render(80);
    println!("\nOutput:");
    for line in &lines {
        println!("'{line}'");
    }

    // Check column count in output
    for line in &lines {
        let s = line.to_string();
        if s.contains('│') {
            let count = s.matches('│').count();
            println!("Line has {count} │: '{s}'");
        }
    }
}

#[test]
fn test_wrap_text_to_width() {
    // Simple word wrap
    let lines = wrap_text_to_width("hello world", 5);
    assert_eq!(lines, vec!["hello", "world"]);

    // No wrap needed
    let lines = wrap_text_to_width("hi", 10);
    assert_eq!(lines, vec!["hi"]);

    // Long word break
    let lines = wrap_text_to_width("abcdefghij", 5);
    assert_eq!(lines, vec!["abcde", "fghij"]);

    // Empty string
    let lines = wrap_text_to_width("", 5);
    assert_eq!(lines, vec![""]);

    // Multiple spaces
    let lines = wrap_text_to_width("a   b", 3);
    assert_eq!(lines, vec!["a b"]);
}

#[test]
fn test_wrap_with_unicode() {
    // Chinese characters (each is width 2)
    let lines = wrap_text_to_width("你好世界", 4);
    assert_eq!(lines, vec!["你好", "世界"]);

    // Mixed content
    let lines = wrap_text_to_width("hello 你好", 6);
    assert_eq!(lines, vec!["hello", "你好"]);
}

#[test]
fn test_streaming_table_with_wrapped_content() {
    let mut renderer = StreamingTableRenderer::new();

    renderer.start_table();
    renderer.start_head();
    renderer.start_row();
    renderer.start_cell();
    renderer.append_text("Name");
    renderer.end_cell();
    renderer.start_cell();
    renderer.append_text("Description");
    renderer.end_cell();
    renderer.end_row();
    renderer.end_head();

    // Separator
    renderer.start_row();
    renderer.start_cell();
    renderer.append_text("---");
    renderer.end_cell();
    renderer.start_cell();
    renderer.append_text("---");
    renderer.end_cell();
    renderer.end_row();

    // Data row with long content
    renderer.start_row();
    renderer.start_cell();
    renderer.append_text("Item1");
    renderer.end_cell();
    renderer.start_cell();
    renderer.append_text("This is a very long description");
    renderer.end_cell();
    renderer.end_row();

    // Render with narrow width to force wrapping
    let lines = renderer.render(30);

    assert!(!lines.is_empty(), "Table should produce output");

    // The description should be wrapped across multiple lines
    let output: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        output.contains('│'),
        "Table should contain vertical borders"
    );
    // Should have multiple lines due to wrapping
    assert!(
        lines.len() > 4,
        "Wrapped table should have more lines: got {} lines",
        lines.len()
    );
}

#[test]
fn test_complete_table_with_wrapping() {
    let table = Table {
        header: Some(TableRow {
            cells: vec!["Name".to_string(), "Description".to_string()],
            is_header: true,
        }),
        rows: vec![TableRow {
            cells: vec![
                "Item".to_string(),
                "This is a very long description that needs wrapping".to_string(),
            ],
            is_header: false,
        }],
        aligns: vec![CellAlign::Left, CellAlign::Left],
    };

    // Render with narrow width to force wrapping
    let lines = table.render(40);

    assert!(!lines.is_empty(), "Table should render");
    // Should have header + separator + wrapped data rows + borders
    assert!(lines.len() >= 4, "Wrapped table should have multiple lines");

    // Verify borders are present
    let output = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(output.contains('│'), "Should have vertical borders");
    assert!(output.contains('─'), "Should have horizontal borders");
}

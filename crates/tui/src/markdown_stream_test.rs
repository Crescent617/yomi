use super::*;

#[test]
fn test_table_rendering() {
    let mut renderer = StreamingMarkdownRenderer::new();
    let content = "| Name | Status |\n|------|--------|\n| foo  | done   |\n| bar  | pending|";
    let lines = renderer.set_content(content.to_string());

    // Should have at least header row + separator + data rows
    assert!(!lines.is_empty(), "Table should produce output");

    // Check that lines contain table borders
    let output = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        output.contains('│'),
        "Table should contain vertical borders"
    );
    assert!(
        output.contains('─'),
        "Table should contain horizontal borders"
    );
}

#[test]
fn test_streaming_table() {
    let mut renderer = StreamingMarkdownRenderer::new();

    // Simulate streaming a table piece by piece
    renderer.append("| Name |");
    renderer.append(" Status |\n");
    renderer.append("|------|--------|\n");
    renderer.append("| foo  |");
    renderer.append(" done   |");

    let lines = renderer.lines();
    assert!(!lines.is_empty(), "Streaming table should produce output");

    let output = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        output.contains('│'),
        "Streaming table should contain borders"
    );
}

#[test]
fn test_table_with_inline_code() {
    let mut renderer = StreamingMarkdownRenderer::new();
    // Table with inline code in cells
    let content = "| Command | Description |\n|---------|-------------|\n| `ls`    | List files  |\n| `cd`    | Change dir  |";
    let lines = renderer.set_content(content.to_string());

    assert!(!lines.is_empty(), "Table should produce output");

    let output = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    // Check that inline code content is present (without backticks in this case,
    // since the Code event provides just the content)
    assert!(output.contains("ls"), "Should contain code content 'ls'");
    assert!(output.contains("cd"), "Should contain code content 'cd'");
    assert!(
        output.contains('│'),
        "Table should contain vertical borders"
    );
}

#[test]
fn test_italic_code_combination() {
    let mut renderer = StreamingMarkdownRenderer::new();
    // Italic with inline code: *`斜体代码`*
    let content = "*`斜体代码`*";
    let lines = renderer.set_content(content.to_string());

    // The code content should be present with both bold and italic
    let output = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(output.contains("斜体代码"), "Should contain code content");

    // Check the actual spans to verify styling
    for line in lines {
        for span in &line.spans {
            if span.content.contains("斜体代码") {
                let style = &span.style;
                assert!(
                    style.add_modifier.contains(Modifier::BOLD),
                    "Should have bold"
                );
                assert!(
                    style.add_modifier.contains(Modifier::ITALIC),
                    "Should have italic"
                );
            }
        }
    }
}

#[test]
fn test_bold_underline_combination() {
    let mut renderer = StreamingMarkdownRenderer::new();
    // Bold + underline: **<u>粗体加下划线</u>**
    let content = "**<u>粗体加下划线</u>**";
    let lines = renderer.set_content(content.to_string());

    let output = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(output.contains("粗体加下划线"), "Should contain content");

    // Check the styling has both bold and underline
    for line in lines {
        for span in &line.spans {
            if span.content.contains("粗体加下划线") {
                let style = &span.style;
                assert!(
                    style.add_modifier.contains(Modifier::BOLD),
                    "Should have bold"
                );
                assert!(
                    style.add_modifier.contains(Modifier::UNDERLINED),
                    "Should have underline"
                );
            }
        }
    }
}

#[test]
fn test_table_respects_set_width() {
    let mut renderer = StreamingMarkdownRenderer::new();
    renderer.set_width(40);
    let content = "| Name | Description |\n|------|-------------|\n| foo  | a somewhat long description that must be wrapped |\n| bar  | pending |";
    let lines = renderer.set_content(content.to_string());

    for line in lines {
        let w = unicode_width::UnicodeWidthStr::width(line.to_string().as_str());
        assert!(w <= 40, "table line width {w} exceeds 40: {line}");
    }
}

#[test]
fn test_width_change_marks_dirty_and_renders() {
    let mut renderer = StreamingMarkdownRenderer::new();
    let content = "| A | B |\n|---|---|\n| x | y |";
    renderer.set_content(content.to_string());

    renderer.set_width(30);
    let narrow = renderer.lines();
    assert!(!narrow.is_empty());
    for line in narrow {
        let w = unicode_width::UnicodeWidthStr::width(line.to_string().as_str());
        assert!(w <= 30, "table line width {w} exceeds 30: {line}");
    }
}

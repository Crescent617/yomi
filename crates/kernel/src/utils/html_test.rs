use super::*;

#[test]
fn test_extract_content_removes_scripts() {
    let html = r"
            <html>
            <head><script>alert('xss');</script></head>
            <body>
                <p>Hello World</p>
                <script>var x = 1;</script>
            </body>
            </html>
        ";
    let result = extract_content(html);
    assert!(!result.contains("alert"));
    assert!(!result.contains("var x"));
    assert!(result.contains("Hello World"));
}

#[test]
fn test_extract_content_removes_nav() {
    let html = r#"
            <html>
            <body>
                <nav><a href="/">Home</a></nav>
                <main><p>Main content</p></main>
            </body>
            </html>
        "#;
    let result = extract_content(html);
    assert!(!result.contains("Home"));
    assert!(result.contains("Main content"));
}

#[test]
fn test_extract_content_prefers_main() {
    let html = r"
            <html>
            <body>
                <header>Site header</header>
                <main><p>The real content</p></main>
                <footer>Site footer</footer>
            </body>
            </html>
        ";
    let result = extract_content(html);
    assert!(result.contains("The real content"));
    assert!(!result.contains("Site header"));
    assert!(!result.contains("Site footer"));
}

#[test]
fn test_extract_content_filters_by_class() {
    let html = r#"
            <html>
            <body>
                <div class="content"><p>Keep this</p></div>
                <div class="sidebar-ad">Remove this ad</div>
                <div id="comment-section">Remove comments</div>
            </body>
            </html>
        "#;
    let result = extract_content(html);
    assert!(result.contains("Keep this"));
    assert!(!result.contains("ad"));
    assert!(!result.contains("comment"));
}

#[test]
fn test_normalize_whitespace() {
    assert_eq!(normalize_whitespace("hello   world"), "hello world");
    assert_eq!(normalize_whitespace("  hello  world  "), "hello world");
    assert_eq!(normalize_whitespace("hello\n\n\nworld"), "hello world");
    assert_eq!(normalize_whitespace("hello\tworld"), "hello world");
}

#[test]
fn test_extract_content_empty_html() {
    let result = extract_content("");
    assert!(result.is_empty());
}

#[test]
fn test_extract_content_no_body() {
    let html = "<html><head><title>Title</title></head></html>";
    let result = extract_content(html);
    // Should fall back to simple_html_to_text or return empty
    assert!(!result.contains("<html>"));
}

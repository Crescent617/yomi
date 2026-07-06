use super::*;

#[test]
fn test_validate_url_valid() {
    let url = "https://example.com/path?query=1";
    let result = WebFetchTool::validate_url(url);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), url);
}

#[test]
fn test_validate_url_different_schemes() {
    // HTTP URLs are kept as-is (no auto-upgrade)
    let http_url = "http://example.com/path";
    let result = WebFetchTool::validate_url(http_url);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), http_url);

    // FTP and other schemes are also allowed
    let ftp_url = "ftp://example.com/file";
    let result = WebFetchTool::validate_url(ftp_url);
    assert!(result.is_ok());

    // URLs with credentials are allowed
    let url_with_creds = "https://user:pass@example.com";
    let result = WebFetchTool::validate_url(url_with_creds);
    assert!(result.is_ok());
}

#[test]
fn test_validate_url_too_long() {
    let url = &format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));
    let result = WebFetchTool::validate_url(url);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("maximum length"));
}

#[test]
fn test_extract_content() {
    let html = r"<!DOCTYPE html>
<html>
<head><title>Test Article</title></head>
<body>
    <header><nav>Navigation noise</nav></header>
    <main>
        <article>
            <h1>Test Article Title</h1>
            <p>This is the main content paragraph with <b>bold</b> text.</p>
            <p>Another paragraph with important information.</p>
        </article>
    </main>
    <footer>Footer noise</footer>
</body>
</html>";
    let markdown = WebFetchTool::extract_content(html, "https://example.com/article");

    // Should extract the main content
    assert!(markdown.len() > 50, "Content should be extracted");

    // Should NOT contain nav/footer noise
    assert!(
        !markdown.contains("Navigation noise"),
        "Should filter out nav"
    );
    assert!(
        !markdown.contains("Footer noise"),
        "Should filter out footer"
    );

    // Should contain the main content
    assert!(
        markdown.contains("main content paragraph"),
        "Should contain main content"
    );
}

#[test]
fn test_extract_content_fallback_when_too_little() {
    // HTML where readability might extract very little (mostly navigation-like content)
    let html = r"<!DOCTYPE html>
<html>
<head><title>My Page</title></head>
<body>
    <div>
        <h2>Section A</h2>
        <p>Content for section A with detailed information.</p>
    </div>
    <div>
        <h2>Section B</h2>
        <p>Content for section B with more detailed information.</p>
    </div>
    <div>
        <h2>Section C</h2>
        <p>Content for section C with even more detailed information.</p>
    </div>
</body>
</html>";
    let markdown = WebFetchTool::extract_content(html, "https://example.com/page");

    // Should still have substantial content (fallback to full HTML if needed)
    assert!(markdown.len() > 100, "Should have substantial content");

    // Should include title since it's meaningful
    assert!(
        markdown.contains("My Page") || markdown.contains("Section"),
        "Should have either title or content"
    );
}

#[test]
fn test_validate_url_file_scheme() {
    let url = "file:///home/user/doc.html";
    let result = WebFetchTool::validate_url(url);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), url);

    let url_no_host = "file:///etc/passwd";
    let result = WebFetchTool::validate_url(url_no_host);
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_fetch_content_file() {
    let tool = WebFetchTool::new();
    let temp = tempfile::NamedTempFile::with_suffix(".html").unwrap();
    let html = r"<!DOCTYPE html>
<html><body><h1>Hello</h1><p>World</p></body></html>";
    tokio::fs::write(temp.path(), html).await.unwrap();

    let url = format!("file://{}", temp.path().display());
    let (content, bytes) = tool.fetch_content(&url).await.unwrap();
    assert!(content.contains("Hello"));
    assert_eq!(bytes, html.len());
}

#[tokio::test]
async fn test_fetch_content_plain_text_file() {
    let tool = WebFetchTool::new();
    let temp = tempfile::NamedTempFile::with_suffix(".txt").unwrap();
    let text = "Plain text content\nSecond line";
    tokio::fs::write(temp.path(), text).await.unwrap();

    let url = format!("file://{}", temp.path().display());
    let (content, bytes) = tool.fetch_content(&url).await.unwrap();
    assert!(content.contains("Plain text content"));
    assert_eq!(bytes, text.len());
}

#[test]
fn test_cache_entry_expiration() {
    let entry = CacheEntry {
        content: "test".to_string(),
        bytes: 4,
        fetched_at: Instant::now()
            .checked_sub(CACHE_TTL + Duration::from_secs(1))
            .unwrap(),
    };
    assert!(entry.is_expired());

    let fresh = CacheEntry {
        content: "test".to_string(),
        bytes: 4,
        fetched_at: Instant::now(),
    };
    assert!(!fresh.is_expired());
}

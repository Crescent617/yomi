//! `WebFetch` tool - fetches content from URLs and extracts article content
//!
//! Filters out scripts, styles, navigation, and other noise before converting
//! to clean text using html2text.

use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use crate::utils::strs::truncate_with_suffix;
use anyhow::Result;
use async_trait::async_trait;
use lru::LruCache;
use serde_json::Value;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub const WEBFETCH_TOOL_NAME: &str = "webfetch";

// 15 minute cache TTL
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
// Max cache entries
const MAX_CACHE_ENTRIES: NonZeroUsize = NonZeroUsize::new(100).unwrap();
// Max content size (10MB)
const MAX_CONTENT_LENGTH: usize = 10 * 1024 * 1024;
// Max URL length
const MAX_URL_LENGTH: usize = 2000;
// Max markdown output length
const MAX_RESULT_LENGTH: usize = 10_000;
// Request timeout
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// Cache entry for fetched content
#[derive(Clone)]
struct CacheEntry {
    content: String,
    content_type: String,
    bytes: usize,
    fetched_at: Instant,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.fetched_at.elapsed() > CACHE_TTL
    }
}

/// Thread-safe cache for web fetch results using async-aware mutex
type FetchCache = Arc<Mutex<LruCache<String, CacheEntry>>>;

fn create_cache() -> FetchCache {
    Arc::new(Mutex::new(LruCache::new(MAX_CACHE_ENTRIES)))
}

/// Global cache instance
static CACHE: std::sync::OnceLock<FetchCache> = std::sync::OnceLock::new();

fn get_cache() -> &'static FetchCache {
    CACHE.get_or_init(create_cache)
}

/// HTTP client with connection pooling for efficient concurrent requests
static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

/// Get shared HTTP client instance
pub fn get_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("Failed to build HTTP client")
    })
}

/// Tool for fetching web content and extracting article content
///
/// Features:
/// - 15-minute LRU cache for repeated URLs
/// - Uses readability to extract main content (filters nav, ads, etc.)
/// - Converts extracted content to Markdown
/// - Content size limits (10MB max)
/// - Connection pooling via shared reqwest Client
pub struct WebFetchTool;

impl WebFetchTool {
    /// Create a new `WebFetchTool` instance
    pub fn new() -> Self {
        Self
    }

    /// Validate URL format and constraints
    fn validate_url(url: &str) -> Result<String, String> {
        if url.len() > MAX_URL_LENGTH {
            return Err(format!(
                "URL exceeds maximum length of {MAX_URL_LENGTH} characters"
            ));
        }

        let parsed: reqwest::Url = match url.parse() {
            Ok(u) => u,
            Err(e) => return Err(format!("Invalid URL: {e}")),
        };

        // Basic hostname validation
        let host = parsed.host_str().ok_or("URL must have a hostname")?;

        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() < 2 {
            return Err("Invalid hostname".to_string());
        }

        Ok(parsed.to_string())
    }

    /// Extract main content from HTML by filtering noise and converting to text
    ///
    /// Delegates to the shared `html_extractor` module
    fn extract_content(html: &str, _url: &str) -> String {
        super::html_extractor::extract_content(html)
    }

    /// Fetch content from URL
    async fn fetch_content(&self, url: &str) -> Result<(String, String, usize), String> {
        // Check cache first
        {
            let mut cache = get_cache().lock().await;
            if let Some(entry) = cache.get(url) {
                if !entry.is_expired() {
                    return Ok((
                        entry.content.clone(),
                        entry.content_type.clone(),
                        entry.bytes,
                    ));
                }
                // Remove expired entry
                cache.pop(url);
            }
        }

        // Make request using shared client
        let response = get_client()
            .get(url)
            .header("Accept", "text/html, text/plain, application/json, */*")
            .header("User-Agent", "Mozilla/5.0 (compatible; Yomi/1.0)")
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "HTTP error: {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            ));
        }

        // Read response body
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response body: {e}"))?;

        if bytes.len() > MAX_CONTENT_LENGTH {
            return Err(format!(
                "Content too large: {} bytes (max: {})",
                bytes.len(),
                MAX_CONTENT_LENGTH
            ));
        }

        // Get content type
        let content_type = get_content_type(&bytes);

        // Convert to string
        let content = String::from_utf8_lossy(&bytes).to_string();

        // Extract main content and convert to markdown if HTML
        let processed_content = if content_type.contains("text/html") {
            Self::extract_content(&content, url)
        } else {
            content
        };

        // Truncate if too long (UTF-8 safe)
        let final_content = truncate_with_suffix(
            &processed_content,
            MAX_RESULT_LENGTH,
            &format!("\n\n[Content truncated - original length: {} characters]", processed_content.len()),
        );

        // Cache the result
        let entry = CacheEntry {
            content: final_content.clone(),
            content_type: content_type.clone(),
            bytes: bytes.len(),
            fetched_at: Instant::now(),
        };
        {
            let mut cache = get_cache().lock().await;
            cache.put(url.to_string(), entry);
        }

        Ok((final_content, content_type, bytes.len()))
    }
}

/// Detect content type from bytes or default to text/plain
fn get_content_type(bytes: &[u8]) -> String {
    // Simple content type detection based on magic bytes
    let starts_with_tag =
        |tag: &[u8]| bytes.len() >= tag.len() && bytes[..tag.len()].eq_ignore_ascii_case(tag);
    if starts_with_tag(b"<!DOCTYPE")
        || starts_with_tag(b"<html")
        || starts_with_tag(b"<head")
        || starts_with_tag(b"<body")
    {
        return "text/html".to_string();
    }
    if bytes.starts_with(b"{") || bytes.starts_with(b"[") {
        return "application/json".to_string();
    }
    "text/plain".to_string()
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        WEBFETCH_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Fetches content from a URL, extracts the main article content (removing navigation, ads, etc.), and converts to markdown."
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from. Must be a fully-formed valid URL."
                }
            },
            "required": ["url"]
        })
    }

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'url' argument"))?;

        // Validate URL
        let validated_url = match Self::validate_url(url) {
            Ok(u) => u,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        // Fetch content
        let (content, content_type, bytes) = match self.fetch_content(&validated_url).await {
            Ok(result) => result,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        // Format output
        let output = format!(
            "Fetched: {validated_url}\nContent-Type: {content_type}\nSize: {bytes} bytes\n\n{content}"
        );

        Ok(ToolOutput::text(output))
    }
}

#[cfg(test)]
mod tests {
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
        let html = r#"<!DOCTYPE html>
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
</html>"#;
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
        let html = r#"<!DOCTYPE html>
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
</html>"#;
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
    fn test_cache_entry_expiration() {
        let entry = CacheEntry {
            content: "test".to_string(),
            content_type: "text/plain".to_string(),
            bytes: 4,
            fetched_at: Instant::now()
                .checked_sub(CACHE_TTL + Duration::from_secs(1))
                .unwrap(),
        };
        assert!(entry.is_expired());

        let fresh = CacheEntry {
            content: "test".to_string(),
            content_type: "text/plain".to_string(),
            bytes: 4,
            fetched_at: Instant::now(),
        };
        assert!(!fresh.is_expired());
    }

    #[test]
    fn test_content_type_detection() {
        assert_eq!(get_content_type(b"<!DOCTYPE html><html>"), "text/html");
        assert_eq!(get_content_type(b"<html lang=\"en\">"), "text/html");
        assert_eq!(
            get_content_type(b"{\"key\": \"value\"}"),
            "application/json"
        );
        assert_eq!(get_content_type(b"[1, 2, 3]"), "application/json");
        assert_eq!(get_content_type(b"plain text"), "text/plain");
    }
}

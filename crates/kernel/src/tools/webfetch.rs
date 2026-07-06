//! `WebFetch` tool - fetches content from URLs and extracts article content
//!
//! Filters out scripts, styles, navigation, and other noise before converting
//! to clean text using html2text.

use crate::tools::helper::truncate::truncate_output;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use async_trait::async_trait;
use lru::LruCache;
use serde_json::Value;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub const WEBFETCH_TOOL_NAME: &str = "webFetch";

// 15 minute cache TTL
const CACHE_TTL: Duration = Duration::from_mins(15);
// Max cache entries
const MAX_CACHE_ENTRIES: NonZeroUsize = NonZeroUsize::new(100).unwrap();
// Max content size (10MB)
const MAX_CONTENT_LENGTH: usize = 10 * 1024 * 1024;
// Max URL length
const MAX_URL_LENGTH: usize = 2000;
// Max markdown output length
const MAX_RESULT_LENGTH: usize = 10_000;
// Request timeout
const FETCH_TIMEOUT: Duration = Duration::from_mins(1);

/// Cache entry for fetched content
#[derive(Clone)]
struct CacheEntry {
    content: String,
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
    fn validate_url(url: &str) -> std::result::Result<String, String> {
        if url.len() > MAX_URL_LENGTH {
            return Err(format!(
                "URL exceeds maximum length of {MAX_URL_LENGTH} characters"
            ));
        }

        let parsed: reqwest::Url = match url.parse() {
            Ok(u) => u,
            Err(e) => return Err(format!("Invalid URL: {e}")),
        };

        // Allow file:// URLs for local file access
        if parsed.scheme() == "file" {
            return Ok(parsed.to_string());
        }

        // Basic hostname validation for network URLs
        let host = parsed.host_str().ok_or("URL must have a hostname")?;

        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() < 2 && !host.eq_ignore_ascii_case("localhost") && host != "127.0.0.1" {
            return Err("Invalid hostname".to_string());
        }

        Ok(parsed.to_string())
    }

    /// Extract main content from HTML by filtering noise and converting to text
    ///
    /// Delegates to the shared `html` utility module
    fn extract_content(html: &str, _url: &str) -> String {
        crate::utils::html::extract_content(html)
    }

    /// Fetch content from URL (HTTP or local file)
    async fn fetch_content(&self, url: &str) -> std::result::Result<(String, usize), String> {
        // Check cache first
        {
            let mut cache = get_cache().lock().await;
            if let Some(entry) = cache.get(url) {
                if !entry.is_expired() {
                    return Ok((entry.content.clone(), entry.bytes));
                }
                cache.pop(url);
            }
        }

        let parsed: reqwest::Url = url.parse().map_err(|e| format!("Invalid URL: {e}"))?;

        let bytes = match parsed.scheme() {
            "file" => tokio::fs::read(parsed.path())
                .await
                .map_err(|e| format!("Failed to read file: {e}"))?,
            _ => {
                let response = get_client()
                    .get(url)
                    .header("Accept", "text/html, text/plain, application/json, */*")
                    .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36")
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

                response
                    .bytes()
                    .await
                    .map(|b| b.to_vec())
                    .map_err(|e| format!("Failed to read response body: {e}"))?
            }
        };

        if bytes.len() > MAX_CONTENT_LENGTH {
            return Err(format!(
                "Content too large: {} bytes (max: {})",
                bytes.len(),
                MAX_CONTENT_LENGTH
            ));
        }

        let content = String::from_utf8_lossy(&bytes).to_string();

        let processed_content = if content.trim().starts_with('<') {
            Self::extract_content(&content, url)
        } else {
            content
        };

        let final_content = truncate_output(
            &processed_content,
            MAX_RESULT_LENGTH,
            &format!(
                "\n\n[Content truncated - original length: {} characters]",
                processed_content.len()
            ),
        );

        let entry = CacheEntry {
            content: final_content.clone(),
            bytes: bytes.len(),
            fetched_at: Instant::now(),
        };
        {
            let mut cache = get_cache().lock().await;
            cache.put(url.to_string(), entry);
        }

        Ok((final_content, bytes.len()))
    }
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
        "Fetches content from a URL, extracts the main article content (removing navigation, ads, etc.), and converts to text."
    }

    fn schema(&self) -> Value {
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
            .ok_or_else(|| KernelError::tool("Missing 'url' argument"))?;

        // Validate URL
        let validated_url = match Self::validate_url(url) {
            Ok(u) => u,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        // Fetch content
        let (content, bytes) = match self.fetch_content(&validated_url).await {
            Ok(result) => result,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        // Format output
        let output = format!("Fetched: {validated_url}\nSize: {bytes} bytes\n\n{content}");

        Ok(ToolOutput::text(output))
    }
}

#[cfg(test)]
#[path = "webfetch_test.rs"]
mod tests;

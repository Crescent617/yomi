//! `WebSearch` tool - searches the web using `Bing` and `DuckDuckGo`
//!
//! Uses both search engines' HTML interfaces for free web search, merging
//! and deduplicating results by URL.

use crate::tools::webfetch::get_client;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::strs::truncate_with_suffix;
use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write as _;
use std::time::Duration;

pub const WEBSEARCH_TOOL_NAME: &str = "websearch";

// Max query length
const MAX_QUERY_LENGTH: usize = 1000;
// Max results to fetch content from
const MAX_CONTENT_RESULTS: usize = 3;
// Max content per page (in characters)
const MAX_CONTENT_LENGTH: usize = 5_000;

/// Search result from `DuckDuckGo`
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Tool for searching the web using `DuckDuckGo` (no API key required)
///
/// Features:
/// - Free search using `DuckDuckGo` HTML interface
/// - No API key required
/// - Fetches content from top results automatically
/// - Returns formatted search results with snippets and page content
pub struct WebSearchTool;

impl WebSearchTool {
    /// Create a new `WebSearchTool` instance
    pub fn new() -> Self {
        Self
    }

    /// Validate search query
    fn validate_query(query: &str) -> std::result::Result<String, String> {
        if query.is_empty() {
            return Err("Search query cannot be empty".to_string());
        }
        if query.len() > MAX_QUERY_LENGTH {
            return Err(format!(
                "Query exceeds maximum length of {MAX_QUERY_LENGTH} characters"
            ));
        }
        Ok(query.to_string())
    }

    /// Perform web search using both `Bing` and `DuckDuckGo` in parallel,
    /// interleaving results by rank and deduplicating by URL.
    async fn search(
        &self,
        query: &str,
        num_results: usize,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let (bing, ddg) = futures::future::join(
            self.search_bing(query, num_results),
            self.search_ddg(query, num_results),
        )
        .await;

        let bing = bing.unwrap_or_default();
        let ddg = ddg.unwrap_or_default();

        if bing.is_empty() && ddg.is_empty() {
            return Err("No search results found".to_string());
        }

        // Interleave by rank: Bing #1, DDG #1, Bing #2, DDG #2, ...
        let mut merged = Vec::with_capacity(num_results);
        let mut seen = std::collections::HashSet::with_capacity(num_results);
        let max_len = bing.len().max(ddg.len());

        for i in 0..max_len {
            for source in [&bing, &ddg] {
                if let Some(r) = source.get(i) {
                    if seen.insert(r.url.clone()) {
                        merged.push(r.clone());
                        if merged.len() >= num_results {
                            return Ok(merged);
                        }
                    }
                }
            }
        }

        Ok(merged)
    }

    /// Search request to `DuckDuckGo` HTML interface
    async fn search_ddg(
        &self,
        query: &str,
        num_results: usize,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let client = get_client();

        // Use DuckDuckGo HTML interface
        // Use DuckDuckGo HTML interface — keep %3A encoded for form bodies.
        let form_body = format!("q={}&kl=us-en", urlencoding::encode(query));

        let response = client
            .post("https://html.duckduckgo.com/html/")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36",
            )
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.5")
            .header("Referer", "https://html.duckduckgo.com/")
            .header("Origin", "https://html.duckduckgo.com")
            .header("DNT", "1")
            .body(form_body)
            .send()
            .await
            .map_err(|e| format!("Search request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("Search failed: HTTP {}", response.status()));
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        Self::parse_ddg_results(&html, num_results)
    }

    /// Parse `DuckDuckGo` HTML results
    fn parse_ddg_results(
        html: &str,
        limit: usize,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let document = scraper::Html::parse_document(html);

        // DuckDuckGo result selector
        let result_selector = scraper::Selector::parse(".result").expect("static CSS selector");
        let title_selector = scraper::Selector::parse(".result__title a").expect("static CSS selector");
        let snippet_selector = scraper::Selector::parse(".result__snippet").expect("static CSS selector");
        let url_selector = scraper::Selector::parse(".result__url").expect("static CSS selector");

        let mut results = Vec::new();

        for result in document.select(&result_selector).take(limit) {
            // Extract title
            let title = result
                .select(&title_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            // Extract URL
            let url = result
                .select(&title_selector)
                .next()
                .and_then(|el| el.value().attr("href"))
                .map(Self::decode_ddg_url)
                .unwrap_or_default();

            // Extract snippet
            let snippet = result
                .select(&snippet_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .or_else(|| {
                    result
                        .select(&url_selector)
                        .next()
                        .map(|el| el.text().collect::<String>().trim().to_string())
                })
                .unwrap_or_default();

            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }

        if results.is_empty() {
            return Err("No search results found".to_string());
        }

        Ok(results)
    }

    /// Decode `DuckDuckGo` redirect URLs to get the actual URL
    fn decode_ddg_url(url: &str) -> String {
        // DuckDuckGo sometimes uses redirect URLs like:
        // //duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&...
        if let Some(pos) = url.find("uddg=") {
            let encoded = &url[pos + 5..];
            let end_pos = encoded.find('&').unwrap_or(encoded.len());
            let encoded_url = &encoded[..end_pos];
            return urlencoding::decode(encoded_url)
                .unwrap_or(std::borrow::Cow::Borrowed(encoded_url))
                .to_string();
        }

        // Handle protocol-relative URLs
        if url.starts_with("//") {
            return format!("https:{url}");
        }

        url.to_string()
    }

    /// Encode a query string for use in search URLs (Bing prefers bare colons).
    fn encode_query_for_url(query: &str) -> String {
        urlencoding::encode(query).replace("%3A", ":")
    }

    /// Perform web search using Bing HTML interface
    async fn search_bing(
        &self,
        query: &str,
        num_results: usize,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let client = get_client();

        let url = format!(
            "https://www.bing.com/search?q={}&setmkt=en-us&setlang=en&count={}",
            Self::encode_query_for_url(query),
            num_results
        );

        let response = client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36",
            )
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.5")
            .header("Referer", "https://www.bing.com/")
            .version(reqwest::Version::HTTP_11)
            .send()
            .await
            .map_err(|e| format!("Bing search request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("Bing search failed: HTTP {}", response.status()));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Bing response: {e}"))?;

        let html = String::from_utf8_lossy(&bytes).to_string();

        Self::parse_bing_results(&html, num_results)
    }

    /// Parse Bing HTML results
    fn parse_bing_results(
        html: &str,
        limit: usize,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let document = scraper::Html::parse_document(html);

        let result_selector = scraper::Selector::parse(".b_algo").expect("static CSS selector");
        let title_selector = scraper::Selector::parse(".b_algo h2 a").expect("static CSS selector");
        let snippet_selector = scraper::Selector::parse(".b_algo .b_caption p").expect("static CSS selector");

        let mut results = Vec::new();

        for result in document.select(&result_selector).take(limit) {
            // Extract title
            let title = result
                .select(&title_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            // Extract URL
            let url = result
                .select(&title_selector)
                .next()
                .and_then(|el| el.value().attr("href"))
                .map(Self::decode_bing_url)
                .unwrap_or_default();

            // Extract snippet
            let snippet = result
                .select(&snippet_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }

        if results.is_empty() {
            return Err("No search results found".to_string());
        }

        Ok(results)
    }

    /// Decode Bing redirect URL to get the actual URL
    fn decode_bing_url(url: &str) -> String {
        // Bing uses redirect URLs like:
        // https://www.bing.com/ck/a?...&u=a1aHR0cHM6Ly9leGFtcGxlLmNvbQ...
        if let Some(pos) = url.find("u=") {
            let encoded = &url[pos + 2..];
            let end_pos = encoded.find('&').unwrap_or(encoded.len());
            let encoded_url = &encoded[..end_pos];

            // Bing URLs start with "a1" prefix before base64
            let b64_part = if let Some(stripped) = encoded_url.strip_prefix("a1") {
                stripped
            } else {
                encoded_url
            };

            // Pad base64 if needed
            let mut padded = b64_part.to_string();
            while padded.len() % 4 != 0 {
                padded.push('=');
            }

            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&padded) {
                if let Ok(decoded_str) = String::from_utf8(decoded) {
                    // Handle protocol-relative URLs
                    if decoded_str.starts_with("//") {
                        return format!("https:{decoded_str}");
                    }
                    if !decoded_str.is_empty() {
                        return decoded_str;
                    }
                }
            }
        }

        // Fallback: return original URL (Bing redirect works too)
        url.to_string()
    }

    /// Fetch content from a URL
    async fn fetch_content(url: &str) -> std::result::Result<String, String> {
        let client = get_client();

        let response = client
            .get(url)
            .header("Accept", "text/html, text/plain, */*")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36",
            )
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let content = String::from_utf8_lossy(&bytes);

        // Extract main content and convert to markdown
        let text = if content.trim().starts_with('<') {
            crate::utils::html::extract_content(&content)
        } else {
            content.to_string()
        };

        // Truncate if too long (UTF-8 safe)
        let truncated = truncate_with_suffix(
            &text,
            MAX_CONTENT_LENGTH,
            &format!(
                "\n\n[Content truncated - original length: {} characters]",
                text.len()
            ),
        );

        Ok(truncated)
    }

    /// Format search results with optional content
    fn format_results(results: &[SearchResult], contents: &[(usize, String)]) -> String {
        // Build a lookup map from result index to content for O(1) access.
        let content_map: std::collections::HashMap<usize, &str> = contents
            .iter()
            .map(|(idx, text)| (*idx, text.as_str()))
            .collect();

        let mut output = String::new();

        for (i, result) in results.iter().enumerate() {
            let _ = writeln!(output, "{}. {}", i + 1, result.title);
            let _ = writeln!(output, "   URL: {}", result.url);
            let _ = writeln!(output, "   Snippet: {}", result.snippet);

            // Add full content if available
            if let Some(content) = content_map.get(&i) {
                let _ = writeln!(output, "   Content:");
                let mut exceeded = false;
                for (line_idx, line) in content.lines().enumerate() {
                    if line_idx >= 30 {
                        exceeded = true;
                        break;
                    }
                    let _ = writeln!(output, "     {line}");
                }
                if exceeded {
                    let _ = writeln!(output, "     [...]");
                }
            }

            output.push('\n');
        }

        output
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        WEBSEARCH_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Searches the web for information using DuckDuckGo. Returns search results with titles, URLs, snippets, and optionally fetches content from top results"
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to execute."
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of search results to return (1-10, default: 5)",
                    "default": 5
                },
                "fetch_content": {
                    "type": "boolean",
                    "description": "Whether to fetch full content from top results (default: true)",
                    "default": true
                }
            },
            "required": ["query"]
        })
    }

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // Extract and validate query
        let query = args["query"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'query' argument"))?;

        let validated_query = match Self::validate_query(query) {
            Ok(q) => q,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        // Get optional parameters
        let num_results = args["num_results"].as_u64().unwrap_or(5) as usize;
        let fetch_content = args["fetch_content"].as_bool().unwrap_or(true);

        // Perform search
        let results = match self.search(&validated_query, num_results).await {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        // Fetch content from top results concurrently if requested
        let contents = if fetch_content {
            let content_limit = num_results.min(MAX_CONTENT_RESULTS);
            let futures: Vec<_> = results
                .iter()
                .take(content_limit)
                .enumerate()
                .map(|(i, result)| async move {
                    match Self::fetch_content(&result.url).await {
                        Ok(content) => Some((i, content)),
                        Err(_) => None, // Silently skip failed fetches
                    }
                })
                .collect();

            futures::future::join_all(futures)
                .await
                .into_iter()
                .flatten()
                .collect()
        } else {
            Vec::new()
        };

        // Format output
        let output = Self::format_results(&results, &contents);
        let summary = format!(
            "Search results for: '{}' ({} results{})",
            validated_query,
            results.len(),
            if fetch_content && !contents.is_empty() {
                format!(", content fetched from {} pages", contents.len())
            } else {
                String::new()
            }
        );

        Ok(ToolOutput::text_with_summary(output, summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_query_valid() {
        let query = "rust programming language";
        let result = WebSearchTool::validate_query(query);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), query);
    }

    #[test]
    fn test_validate_query_empty() {
        let result = WebSearchTool::validate_query("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_query_too_long() {
        let query = "a".repeat(MAX_QUERY_LENGTH + 1);
        let result = WebSearchTool::validate_query(&query);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("maximum length"));
    }

    #[test]
    fn test_decode_ddg_url() {
        // Test protocol-relative URL
        assert_eq!(
            WebSearchTool::decode_ddg_url("//example.com"),
            "https://example.com"
        );

        // Test plain URL
        assert_eq!(
            WebSearchTool::decode_ddg_url("https://example.com"),
            "https://example.com"
        );
    }

    #[test]
    fn test_parse_ddg_results() {
        // Sample DuckDuckGo HTML response
        let html = r#"
<!DOCTYPE html>
<html>
<body>
    <div class="result">
        <h2 class="result__title"><a href="https://example.com/1">Test Title 1</a></h2>
        <div class="result__snippet">Test snippet 1</div>
    </div>
    <div class="result">
        <h2 class="result__title"><a href="https://example.com/2">Test Title 2</a></h2>
        <div class="result__snippet">Test snippet 2</div>
    </div>
</body>
</html>
        "#;

        let results = WebSearchTool::parse_ddg_results(html, 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Test Title 1");
        assert_eq!(results[0].url, "https://example.com/1");
        assert_eq!(results[0].snippet, "Test snippet 1");
    }

    #[test]
    fn test_format_results() {
        let results = vec![
            SearchResult {
                title: "Test Title 1".to_string(),
                url: "https://example.com/1".to_string(),
                snippet: "Snippet 1".to_string(),
            },
            SearchResult {
                title: "Test Title 2".to_string(),
                url: "https://example.com/2".to_string(),
                snippet: "Snippet 2".to_string(),
            },
        ];

        let contents = vec![(0, "Full content for page 1".to_string())];

        let output = WebSearchTool::format_results(&results, &contents);

        assert!(output.contains("Test Title 1"));
        assert!(output.contains("Test Title 2"));
        assert!(output.contains("https://example.com/1"));
        assert!(output.contains("Full content for page 1"));
    }

    #[test]
    #[ignore = "requires /tmp/bing.html from real Bing response"]
    fn test_parse_bing_real_html() {
        let html = std::fs::read_to_string("/tmp/bing.html").unwrap();
        let results = WebSearchTool::parse_bing_results(&html, 3).unwrap();
        assert!(!results.is_empty(), "Expected non-empty Bing results");
        println!("Bing parsed {} results", results.len());
        for (i, r) in results.iter().take(3).enumerate() {
            println!("{}: {} -> {}", i, r.title, r.url);
        }
    }

    #[tokio::test]
    #[ignore = "live network test"]
    async fn test_search_bing_live() {
        let tool = WebSearchTool::new();
        let results = tool.search_bing("rust:ownership", 3).await;
        println!("search_bing result: {results:?}");
        assert!(results.is_ok(), "search_bing failed: {results:?}");
        let results = results.unwrap();
        assert!(
            !results.is_empty(),
            "search_bing returned empty results: {results:?}"
        );
        for (i, r) in results.iter().take(3).enumerate() {
            println!("{}: {} -> {}", i, r.title, r.url);
        }
    }
}

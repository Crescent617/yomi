//! `WebSearch` tool - searches the web using `DuckDuckGo` (no API key required)
//!
//! Uses `DuckDuckGo`'s HTML interface for free web search.

use crate::tools::webfetch::get_client;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::strs::truncate_with_suffix;
use async_trait::async_trait;
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

    /// Perform web search using `DuckDuckGo` HTML interface
    async fn search(
        &self,
        query: &str,
        num_results: usize,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let client = get_client();

        // Use DuckDuckGo HTML interface
        let form_body = format!("q={}&kl=us-en", urlencoding::encode(query));

        let response = client
            .post("https://html.duckduckgo.com/html/")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header("Accept", "text/html,application/xhtml+xml")
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

        Self::parse_duckduckgo_results(&html, num_results)
    }

    /// Parse `DuckDuckGo` HTML results
    fn parse_duckduckgo_results(
        html: &str,
        limit: usize,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let document = scraper::Html::parse_document(html);

        // DuckDuckGo result selector
        let result_selector = scraper::Selector::parse(".result").unwrap();
        let title_selector = scraper::Selector::parse(".result__title a").unwrap();
        let snippet_selector = scraper::Selector::parse(".result__snippet").unwrap();
        let url_selector = scraper::Selector::parse(".result__url").unwrap();

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
                .map(Self::clean_duckduckgo_url)
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

    /// Clean `DuckDuckGo` redirect URLs to get the actual URL
    fn clean_duckduckgo_url(url: &str) -> String {
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

    /// Extract main content from HTML by filtering noise and converting to text
    ///
    /// Delegates to the shared `html` utility module
    fn extract_content(html: &str, _url: &str) -> String {
        crate::utils::html::extract_content(html)
    }

    /// Fetch content from a URL
    async fn fetch_content(url: &str) -> std::result::Result<String, String> {
        let client = get_client();

        let response = client
            .get(url)
            .header("Accept", "text/html, text/plain, */*")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
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
            Self::extract_content(&content, url)
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
        let mut output = String::new();

        for (i, result) in results.iter().enumerate() {
            let _ = writeln!(output, "{}. {}", i + 1, result.title);
            let _ = writeln!(output, "   URL: {}", result.url);
            let _ = writeln!(output, "   Snippet: {}", result.snippet);

            // Add full content if available
            if let Some((_, content)) = contents.iter().find(|(idx, _)| *idx == i) {
                let _ = writeln!(output, "   Content:");
                for line in content.lines().take(30) {
                    let _ = writeln!(output, "     {line}");
                }
                if content.lines().count() > 30 {
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
    fn test_clean_duckduckgo_url() {
        // Test protocol-relative URL
        assert_eq!(
            WebSearchTool::clean_duckduckgo_url("//example.com"),
            "https://example.com"
        );

        // Test plain URL
        assert_eq!(
            WebSearchTool::clean_duckduckgo_url("https://example.com"),
            "https://example.com"
        );
    }

    #[test]
    fn test_parse_duckduckgo_results() {
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

        let results = WebSearchTool::parse_duckduckgo_results(html, 10).unwrap();
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
}

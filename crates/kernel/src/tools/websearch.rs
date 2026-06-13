//! `WebSearch` tool — searches the web and returns results with titles, URLs,
//! snippets, and optionally fetches content from top results.
//!
//! Queries multiple sources in parallel, merges by rank, and deduplicates by URL.
//! If all sources fail, the error includes the specific failure reason from each.

use crate::tools::webfetch::get_client;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::search::{available_engines, merge_results, SearchEngine, SearchResult};
use crate::utils::strs::truncate_with_suffix;
use async_trait::async_trait;
use serde_json::Value;
use std::fmt::Write as _;
use std::time::Duration;

pub const WEBSEARCH_TOOL_NAME: &str = "websearch";

const MAX_QUERY_LENGTH: usize = 1000;
const MAX_CONTENT_RESULTS: usize = 3;
const MAX_CONTENT_LENGTH: usize = 5_000;

pub struct WebSearchTool {
    engines: Vec<Box<dyn SearchEngine>>,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            engines: available_engines(),
        }
    }

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

    /// Query all available engines in parallel, merge and deduplicate.
    async fn search(
        &self,
        query: &str,
        num_results: usize,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        if self.engines.is_empty() {
            return Err("No search sources are available".to_string());
        }

        let mut errors: Vec<String> = Vec::new();
        let mut all_results: Vec<Vec<SearchResult>> = Vec::new();

        // Run every source in parallel.
        let futures: Vec<_> = self
            .engines
            .iter()
            .map(|engine| async move {
                match engine.search(query, num_results).await {
                    Ok(res) => Ok(res),
                    Err(e) => Err(format!("{}: {e}", engine.name())),
                }
            })
            .collect();

        let outcomes = futures::future::join_all(futures).await;

        for outcome in outcomes {
            match outcome {
                Ok(results) => all_results.push(results),
                Err(err) => errors.push(err),
            }
        }

        if all_results.is_empty() {
            let msg = if errors.len() == 1 {
                errors.into_iter().next().unwrap()
            } else {
                format!("All sources failed: {}", errors.join("; "))
            };
            return Err(msg);
        }

        let merged = merge_results(&all_results, num_results);
        if merged.is_empty() {
            return Err("No search results found".to_string());
        }

        Ok(merged)
    }

    /// Fetch raw page content from a URL and convert to clean text.
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

        let text = if content.trim().starts_with('<') {
            crate::utils::html::extract_content(&content)
        } else {
            content.into_owned()
        };

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

    /// Format search results with optional page content.
    fn format_results(results: &[SearchResult], contents: &[(usize, String)]) -> String {
        let content_map: std::collections::HashMap<usize, &str> = contents
            .iter()
            .map(|(idx, text)| (*idx, text.as_str()))
            .collect();

        let mut output = String::new();

        for (i, result) in results.iter().enumerate() {
            let _ = writeln!(output, "{}. {}", i + 1, result.title);
            let _ = writeln!(output, "   URL: {}", result.url);
            let _ = writeln!(output, "   Snippet: {}", result.snippet);
            let _ = writeln!(output, "   Source: {}", result.source);

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
        "Searches the web and returns results with titles, URLs, snippets, and optionally fetches content from top results."
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
        let query = args["query"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'query' argument"))?;

        let validated_query = match Self::validate_query(query) {
            Ok(q) => q,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        let num_results = args["num_results"].as_u64().unwrap_or(5) as usize;
        let fetch_content = args["fetch_content"].as_bool().unwrap_or(true);

        let results = match self.search(&validated_query, num_results).await {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        // Fetch content from top results concurrently if requested.
        let contents = if fetch_content {
            let content_limit = num_results.min(MAX_CONTENT_RESULTS);
            let futures: Vec<_> = results
                .iter()
                .take(content_limit)
                .enumerate()
                .map(|(i, result)| async move {
                    match Self::fetch_content(&result.url).await {
                        Ok(content) => Some((i, content)),
                        Err(_) => None, // Silently skip failed fetches.
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
    fn test_format_results() {
        let results = vec![
            SearchResult {
                title: "Test Title 1".to_string(),
                url: "https://example.com/1".to_string(),
                snippet: "Snippet 1".to_string(),
                source: "ddg",
            },
            SearchResult {
                title: "Test Title 2".to_string(),
                url: "https://example.com/2".to_string(),
                snippet: "Snippet 2".to_string(),
                source: "bing",
            },
        ];

        let contents = vec![(0, "Full content for page 1".to_string())];

        let output = WebSearchTool::format_results(&results, &contents);

        assert!(output.contains("Test Title 1"));
        assert!(output.contains("Test Title 2"));
        assert!(output.contains("https://example.com/1"));
        assert!(output.contains("Full content for page 1"));
        assert!(output.contains("Source: ddg"));
        assert!(output.contains("Source: bing"));
    }
}

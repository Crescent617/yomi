//! Search engine utilities — unified trait and multi-engine merging.
//!
//! Each engine is a struct implementing the [`SearchEngine`] trait.
//! The [`available_engines`] helper builds the runtime list from environment
//! variables. [`merge_results`] interleaves and deduplicates results by URL.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Write as _;

pub mod bing;
pub mod brave;
pub mod ddg;
pub mod searxng;

/// A single search result, engine-agnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source: &'static str,
}

/// Core trait for every search engine backend.
#[async_trait]
pub trait SearchEngine: Send + Sync {
    /// Short identifier used in diagnostics (e.g. `"ddg"`, `"brave"`).
    fn name(&self) -> &'static str;

    /// Execute a search and return up to `limit` results.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String>;
}

/// Encode a query for search URLs.
///
/// Most search engines prefer `+` for spaces (`application/x-www-form-urlencoded`).
/// We also keep bare `:` since many engines (especially Bing) handle it better.
pub fn encode_query(query: &str) -> String {
    urlencoding::encode(query)
        .replace("%20", "+")
        .replace("%3A", ":")
}

/// Build the list of engines to use based on environment variables.
///
/// Priority:
/// 1. Configured engines (Brave, `SearXNG`) — if their env vars are set.
///    Both generic names (e.g. `BRAVE_API_KEY`) and `YOMI_`-prefixed names
///    are supported.
/// 2. Free engines (`DuckDuckGo`, Bing) — always included as fallback.
pub(crate) fn available_engines() -> Vec<Box<dyn SearchEngine>> {
    use crate::config::env_names;
    use crate::utils::env::env_first;

    let mut engines: Vec<Box<dyn SearchEngine>> = Vec::new();

    if let Some(key) = env_first(&[env_names::BRAVE_API_KEY, env_names::YOMI_BRAVE_API_KEY]) {
        if !key.trim().is_empty() {
            engines.push(Box::new(brave::BraveEngine::new(key)));
        }
    }

    if let Some(url) = env_first(&[env_names::SEARXNG_URL, env_names::YOMI_SEARXNG_URL]) {
        let url = url.trim();
        if !url.is_empty() {
            engines.push(Box::new(searxng::SearxngEngine::new(url.to_string())));
        }
    }

    // Free engines always available as fallback.
    engines.push(Box::new(ddg::DdgEngine::new()));
    engines.push(Box::new(bing::BingEngine::new()));

    engines
}

/// Merge multiple ordered result lists, interleaving by rank and
/// deduplicating by URL.
///
/// Example: engine A [a1, a2, a3], engine B [b1, b2]
///   → [a1, b1, a2, b2, a3]
pub fn merge_results(sources: &[Vec<SearchResult>], limit: usize) -> Vec<SearchResult> {
    let mut merged = Vec::with_capacity(limit);
    let mut seen = HashSet::with_capacity(limit);

    let max_len = sources.iter().map(|v| v.len()).max().unwrap_or(0);

    for i in 0..max_len {
        for source in sources {
            if let Some(r) = source.get(i) {
                if seen.insert(r.url.clone()) {
                    merged.push(r.clone());
                    if merged.len() >= limit {
                        return merged;
                    }
                }
            }
        }
    }

    merged
}

/// Query all engines in parallel, merge and deduplicate.
///
/// If every engine fails, returns an error with each engine's failure reason.
pub async fn search_all(
    engines: &[Box<dyn SearchEngine>],
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    if engines.is_empty() {
        return Err("No search sources are available".to_string());
    }

    let mut errors: Vec<String> = Vec::new();
    let mut all_results: Vec<Vec<SearchResult>> = Vec::new();

    let futures: Vec<_> = engines
        .iter()
        .map(|engine| async move {
            match engine.search(query, limit).await {
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

    let merged = merge_results(&all_results, limit);
    if merged.is_empty() {
        return Err("No search results found".to_string());
    }

    Ok(merged)
}

/// Fetch raw page content from a URL and convert to clean text.
pub async fn fetch_content(url: &str) -> Result<String, String> {
    use crate::tools::webfetch::get_client;
    use crate::utils::strs::truncate_with_suffix;

    let client = get_client();

    let response = client
        .get(url)
        .header("Accept", "text/html, text/plain, */*")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(15))
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
        5_000,
        &format!(
            "\n\n[Content truncated - original length: {} characters]",
            text.len()
        ),
    );

    Ok(truncated)
}

/// Format search results with optional page content.
pub fn format_results(results: &[SearchResult], contents: &[(usize, String)]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_query() {
        assert_eq!(encode_query("hello world"), "hello+world");
        assert_eq!(encode_query("a:b"), "a:b");
    }

    #[test]
    fn test_merge_results() {
        let a = vec![SearchResult {
            title: "A1".to_string(),
            url: "https://a1".to_string(),
            snippet: String::new(),
            source: "a",
        }];
        let b = vec![SearchResult {
            title: "B1".to_string(),
            url: "https://b1".to_string(),
            snippet: String::new(),
            source: "b",
        }];
        let merged = merge_results(&[a, b], 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].url, "https://a1");
        assert_eq!(merged[1].url, "https://b1");
    }

    #[test]
    fn test_merge_results_dedup() {
        let a = vec![SearchResult {
            title: "A1".to_string(),
            url: "https://dup".to_string(),
            snippet: String::new(),
            source: "a",
        }];
        let b = vec![SearchResult {
            title: "B1".to_string(),
            url: "https://dup".to_string(),
            snippet: String::new(),
            source: "b",
        }];
        let merged = merge_results(&[a, b], 10);
        assert_eq!(merged.len(), 1);
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

        let output = format_results(&results, &contents);

        assert!(output.contains("Test Title 1"));
        assert!(output.contains("Test Title 2"));
        assert!(output.contains("https://example.com/1"));
        assert!(output.contains("Full content for page 1"));
        assert!(output.contains("Source: ddg"));
        assert!(output.contains("Source: bing"));
    }
}

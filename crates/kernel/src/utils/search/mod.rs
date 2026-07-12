//! Search engine utilities — unified trait and serial fallback.
//!
//! Each engine is a struct implementing the [`SearchEngine`] trait.
//! The [`available_engines`] helper builds the runtime list from environment
//! variables. Search engines are tried serially in priority order until one
//! returns results.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Write as _;

pub mod bing;
pub mod brave;
pub mod ddg;
pub mod searxng;
pub mod serper;

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
/// 1. `SearXNG`, when configured.
/// 2. Serper.dev, when configured.
/// 3. Brave, when configured.
/// 4. `DuckDuckGo`, then Bing, as free fallbacks.
pub(crate) fn available_engines() -> Vec<Box<dyn SearchEngine>> {
    use crate::config::env_names;
    use crate::utils::env::env_first;

    let mut engines: Vec<Box<dyn SearchEngine>> = Vec::new();

    if let Some(url) = env_first(&[env_names::SEARXNG_URL, env_names::YOMI_SEARXNG_URL]) {
        let url = url.trim();
        if !url.is_empty() {
            engines.push(Box::new(searxng::SearxngEngine::new(url.to_string())));
        }
    }

    if let Some(key) = env_first(&[env_names::SERPER_API_KEY]) {
        if !key.trim().is_empty() {
            engines.push(Box::new(serper::SerperEngine::new(key)));
        }
    }

    if let Some(key) = env_first(&[env_names::BRAVE_API_KEY, env_names::YOMI_BRAVE_API_KEY]) {
        if !key.trim().is_empty() {
            engines.push(Box::new(brave::BraveEngine::new(key)));
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

/// Query engines serially in priority order, returning the first non-empty result set.
///
/// If every engine fails or returns no results, returns an error with each engine's
/// failure reason.
pub async fn search_all(
    engines: &[Box<dyn SearchEngine>],
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    if engines.is_empty() {
        return Err("No search sources are available".to_string());
    }

    let mut errors = Vec::new();
    for engine in engines {
        match engine.search(query, limit).await {
            Ok(results) if !results.is_empty() => return Ok(results),
            Ok(_) => errors.push(format!("{}: no results", engine.name())),
            Err(error) => errors.push(format!("{}: {error}", engine.name())),
        }
    }

    let message = if errors.len() == 1 {
        errors.into_iter().next().unwrap()
    } else {
        format!("All sources failed: {}", errors.join("; "))
    };
    Err(message)
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
mod tests;

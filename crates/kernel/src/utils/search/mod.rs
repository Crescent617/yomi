//! Search engine utilities — unified trait and multi-engine merging.
//!
//! Each engine is a struct implementing the [`SearchEngine`] trait.
//! The [`available_engines`] helper builds the runtime list from environment
//! variables. [`merge_results`] interleaves and deduplicates results by URL.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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

    if let Some(key) = env_first(&[env_names::BRAVE_API_KEY, "YOMI_BRAVE_API_KEY"]) {
        if !key.trim().is_empty() {
            engines.push(Box::new(brave::BraveEngine::new(key)));
        }
    }

    if let Some(url) = env_first(&[env_names::SEARXNG_URL, "YOMI_SEARXNG_URL"]) {
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

/// Encode a query for search URLs.
///
/// Most search engines prefer `+` for spaces (`application/x-www-form-urlencoded`).
/// We also keep bare `:` since many engines (especially Bing) handle it better.
pub fn encode_query(query: &str) -> String {
    urlencoding::encode(query)
        .replace("%20", "+")
        .replace("%3A", ":")
}

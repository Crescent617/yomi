//! `DuckDuckGo` search engine using the `duckduckgo` crate.
//!
//! Uses the **Lite** backend (`lite.duckduckgo.com`) which is a minimal
//! HTML-only interface with less anti-bot friction than the full
//! `html.duckduckgo.com` endpoint.

use crate::utils::search::{SearchEngine, SearchResult};
use async_trait::async_trait;

pub struct DdgEngine;

impl DdgEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DdgEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchEngine for DdgEngine {
    fn name(&self) -> &'static str {
        "ddg"
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let browser = duckduckgo::browser::Browser::new();
        let ua = duckduckgo::user_agents::get("firefox")
            .unwrap_or("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36");

        let lite_results = browser
            .lite_search(query, "wt-wt", Some(limit), ua)
            .await
            .map_err(|e| format!("DuckDuckGo Lite search failed: {e}"))?;

        let mut results = Vec::with_capacity(lite_results.len());
        for r in lite_results {
            results.push(SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.snippet,
                source: "ddg",
            });
        }

        if results.is_empty() {
            return Err("DuckDuckGo Lite returned no results".to_string());
        }

        Ok(results)
    }
}

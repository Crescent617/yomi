//! Kimi Search API engine.
//!
//! Requires `KIMI_AGENT_API_KEY` to be set. The endpoint defaults to:
//! `https://api.kimi.com/coding/v1/search`
//!
//! (The old `agent-gw.kimi.com` gateway rejects Kimi-for-coding
//! subscription keys with 403; `api.kimi.com` accepts them.)
//!
//! Override with the optional `KIMI_SEARCH_ENDPOINT` environment variable.
//!
//! Page crawling is enabled server-side, so results may carry an extracted
//! `content` field that spares the client its own fetch.
//!
//! Request body:
//! ```json
//! {
//!   "text_query": "...",
//!   "limit": 20,
//!   "enable_page_crawling": true,
//!   "timeout_seconds": 15
//! }
//! ```
//!
//! Response body:
//! ```json
//! {
//!   "search_results": [
//!     {
//!       "title": "...",
//!       "url": "...",
//!       "snippet": "...",
//!       "content": "...",
//!       "date": "...",
//!       "icon": "...",
//!       "mime": "...",
//!       "site_name": "..."
//!     }
//!   ]
//! }
//! ```

use crate::utils::http::client as get_client;
use crate::utils::search::{SearchEngine, SearchResult};
use async_trait::async_trait;

const DEFAULT_ENDPOINT: &str = "https://api.kimi.com/coding/v1/search";

pub struct KimiEngine {
    api_key: String,
    endpoint: String,
}

impl KimiEngine {
    pub fn new(api_key: String, endpoint: Option<String>) -> Self {
        let endpoint = endpoint
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        Self { api_key, endpoint }
    }
}

#[async_trait]
impl SearchEngine for KimiEngine {
    fn name(&self) -> &'static str {
        "kimi"
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let response = get_client()
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&serde_json::json!({
                "text_query": query,
                "limit": limit.min(20),
                "enable_page_crawling": true,
                "timeout_seconds": 15,
            }))
            .send()
            .await
            .map_err(|e| format!("Kimi search request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("Kimi search failed: HTTP {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Kimi response: {e}"))?;

        parse_search_results(&json, limit)
    }
}

/// Parse the `search_results` array of a Kimi response into engine-agnostic
/// results, keeping the server-extracted `content` when present.
fn parse_search_results(
    json: &serde_json::Value,
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    let results = json
        .get("search_results")
        .and_then(|r| r.as_array())
        .ok_or("Kimi response missing search_results")?;

    let mut out = Vec::with_capacity(results.len());
    for item in results.iter().take(limit) {
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let snippet = item.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string);

        if !title.is_empty() && !url.is_empty() {
            out.push(SearchResult {
                title: title.to_string(),
                url: url.to_string(),
                snippet: snippet.to_string(),
                source: "kimi",
                content,
            });
        }
    }

    if out.is_empty() {
        return Err("Kimi returned no results".to_string());
    }

    Ok(out)
}

#[cfg(test)]
#[path = "kimi_test.rs"]
mod tests;

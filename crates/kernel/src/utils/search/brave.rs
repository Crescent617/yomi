//! Brave Search API engine.
//!
//! Requires `BRAVE_API_KEY` to be set. Returns structured JSON
//! results without any HTML scraping.

use crate::utils::http::client as get_client;
use crate::utils::search::{SearchEngine, SearchResult};
use async_trait::async_trait;

pub struct BraveEngine {
    api_key: String,
}

impl BraveEngine {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl SearchEngine for BraveEngine {
    fn name(&self) -> &'static str {
        "brave"
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let client = get_client();

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}&offset=0",
            crate::utils::search::encode_query(query),
            limit.min(20)
        );

        let response = client
            .get(&url)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("Brave search request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("Brave search failed: HTTP {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Brave response: {e}"))?;

        let results = json
            .get("web")
            .and_then(|w| w.get("results"))
            .and_then(|r| r.as_array())
            .ok_or("Brave response missing web.results")?;

        let mut out = Vec::with_capacity(results.len());
        for item in results.iter().take(limit) {
            let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let snippet = item
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if !title.is_empty() && !url.is_empty() {
                out.push(SearchResult {
                    title: title.to_string(),
                    url: url.to_string(),
                    snippet: snippet.to_string(),
                    source: "brave",
                });
            }
        }

        if out.is_empty() {
            return Err("Brave returned no results".to_string());
        }

        Ok(out)
    }
}

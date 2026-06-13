//! `SearXNG` search engine.
//!
//! Requires `YOMI_SEARXNG_URL` to be set (e.g. `https://searx.be`).
//! Uses the JSON API endpoint (`/search?format=json`).

use crate::tools::webfetch::get_client;
use crate::utils::search::{SearchEngine, SearchResult};
use async_trait::async_trait;

pub struct SearxngEngine {
    base_url: String,
}

impl SearxngEngine {
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }
}

#[async_trait]
impl SearchEngine for SearxngEngine {
    fn name(&self) -> &'static str {
        "searxng"
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let client = get_client();

        let base = self.base_url.trim_end_matches('/');
        let url = format!(
            "{}/search?q={}&format=json&categories=general",
            base,
            crate::utils::search::encode_query(query)
        );

        let response = client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("SearXNG request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("SearXNG failed: HTTP {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse SearXNG response: {e}"))?;

        let results = json
            .get("results")
            .and_then(|r| r.as_array())
            .ok_or("SearXNG response missing results")?;

        let mut out = Vec::with_capacity(results.len());
        for item in results.iter().take(limit) {
            let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let snippet = item.get("content").and_then(|v| v.as_str()).unwrap_or("");

            if !title.is_empty() && !url.is_empty() {
                out.push(SearchResult {
                    title: title.to_string(),
                    url: url.to_string(),
                    snippet: snippet.to_string(),
                    source: "searxng",
                });
            }
        }

        if out.is_empty() {
            return Err("SearXNG returned no results".to_string());
        }

        Ok(out)
    }
}

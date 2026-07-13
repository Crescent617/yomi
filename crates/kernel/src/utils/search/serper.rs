//! Serper.dev Google Search API engine.
//!
//! Requires `SERPER_API_KEY` to be set.

use crate::utils::http::client as get_client;
use crate::utils::search::{SearchEngine, SearchResult};
use async_trait::async_trait;

pub struct SerperEngine {
    api_key: String,
}

impl SerperEngine {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl SearchEngine for SerperEngine {
    fn name(&self) -> &'static str {
        "serper"
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let response = get_client()
            .post("https://google.serper.dev/search")
            .header("X-API-KEY", &self.api_key)
            .header("Accept", "application/json")
            .json(&serde_json::json!({
                "q": query,
                "num": limit,
            }))
            .send()
            .await
            .map_err(|e| format!("Serper search request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("Serper search failed: HTTP {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Serper response: {e}"))?;

        parse_results(&json, limit)
    }
}

fn parse_results(json: &serde_json::Value, limit: usize) -> Result<Vec<SearchResult>, String> {
    let results = json
        .get("organic")
        .and_then(|value| value.as_array())
        .ok_or("Serper response missing organic results")?;

    let out: Vec<_> = results
        .iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?;
            let url = item.get("link")?.as_str()?;
            if title.is_empty() || url.is_empty() {
                return None;
            }

            Some(SearchResult {
                title: title.to_string(),
                url: url.to_string(),
                snippet: item
                    .get("snippet")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
                source: "serper",
            })
        })
        .take(limit)
        .collect();

    if out.is_empty() {
        return Err("Serper returned no results".to_string());
    }

    Ok(out)
}

#[cfg(test)]
#[path = "serper_test.rs"]
mod tests;

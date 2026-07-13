//! Bing search engine — HTML interface scraping.
//!
//! This is the original Bing scraping logic, extracted into the util layer.
//! It uses `+` for spaces (not `%20`) to improve Bing compatibility.

use crate::utils::http::client as get_client;
use crate::utils::search::{SearchEngine, SearchResult};
use async_trait::async_trait;
use base64::Engine as Base64Engine;

pub struct BingEngine;

impl BingEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BingEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchEngine for BingEngine {
    fn name(&self) -> &'static str {
        "bing"
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let client = get_client();

        let url = format!(
            "https://www.bing.com/search?q={}&setmkt=en-us&setlang=en&count={}",
            crate::utils::search::encode_query(query),
            limit
        );

        let response = client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36",
            )
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.5")
            .header("Referer", "https://www.bing.com/")
            .version(reqwest::Version::HTTP_11)
            .send()
            .await
            .map_err(|e| format!("Bing search request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("Bing search failed: HTTP {}", response.status()));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Bing response: {e}"))?;

        let html = String::from_utf8_lossy(&bytes).into_owned();
        parse_results(&html, limit)
    }
}

fn parse_results(html: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
    let document = scraper::Html::parse_document(html);

    let result_selector = scraper::Selector::parse(".b_algo").expect("static selector");
    let title_selector = scraper::Selector::parse(".b_algo h2 a").expect("static selector");
    let snippet_selector =
        scraper::Selector::parse(".b_algo .b_caption p").expect("static selector");

    let mut results = Vec::new();

    for result in document.select(&result_selector).take(limit) {
        let title = result
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        let url = result
            .select(&title_selector)
            .next()
            .and_then(|el| el.value().attr("href"))
            .map(decode_url)
            .unwrap_or_default();

        let snippet = result
            .select(&snippet_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
                source: "bing",
            });
        }
    }

    if results.is_empty() {
        return Err("No Bing search results found".to_string());
    }

    Ok(results)
}

/// Decode Bing redirect URL to get the actual URL.
///
/// Bing wraps outbound links in `https://www.bing.com/ck/a?…&u=a1<base64>…`.
fn decode_url(url: &str) -> String {
    if let Some(pos) = url.find("u=") {
        let encoded = &url[pos + 2..];
        let end_pos = encoded.find('&').unwrap_or(encoded.len());
        let encoded_url = &encoded[..end_pos];

        let b64_part = if let Some(stripped) = encoded_url.strip_prefix("a1") {
            stripped
        } else {
            encoded_url
        };

        let mut padded = b64_part.to_string();
        while padded.len() % 4 != 0 {
            padded.push('=');
        }

        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&padded) {
            if let Ok(decoded_str) = String::from_utf8(decoded) {
                if decoded_str.starts_with("//") {
                    return format!("https:{decoded_str}");
                }
                if !decoded_str.is_empty() {
                    return decoded_str;
                }
            }
        }
    }

    url.to_string()
}

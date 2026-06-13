//! `WebSearch` tool — searches the web and returns results with titles, URLs,
//! snippets, and optionally fetches content from top results.

use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::search::{available_engines, format_results, search_all};
use async_trait::async_trait;
use serde_json::Value;

pub const WEBSEARCH_TOOL_NAME: &str = "websearch";

const MAX_QUERY_LENGTH: usize = 1000;

pub struct WebSearchTool {
    engines: Vec<Box<dyn crate::utils::search::SearchEngine>>,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            engines: available_engines(),
        }
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

        if query.is_empty() {
            return Ok(ToolOutput::error("Search query cannot be empty".to_string()));
        }
        if query.len() > MAX_QUERY_LENGTH {
            return Ok(ToolOutput::error(format!(
                "Query exceeds maximum length of {MAX_QUERY_LENGTH} characters"
            )));
        }

        let num_results = args["num_results"].as_u64().unwrap_or(5) as usize;
        let should_fetch = args["fetch_content"].as_bool().unwrap_or(true);

        let results = match search_all(&self.engines, query, num_results).await {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(e)),
        };

        // Fetch content from top results concurrently if requested.
        let contents = if should_fetch {
            let futures: Vec<_> = results
                .iter()
                .take(3)
                .enumerate()
                .map(|(i, result)| async move {
                    match crate::utils::search::fetch_content(&result.url).await {
                        Ok(content) => Some((i, content)),
                        Err(_) => None,
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

        let output = format_results(&results, &contents);
        let summary = format!(
            "Search results for: '{}' ({} results{})",
            query,
            results.len(),
            if should_fetch && !contents.is_empty() {
                format!(", content fetched from {} pages", contents.len())
            } else {
                String::new()
            }
        );

        Ok(ToolOutput::text_with_summary(output, summary))
    }
}

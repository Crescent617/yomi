//! `WebSearch` tool — searches the web and returns results with titles, URLs,
//! snippets, and optionally fetches content from top results.

use crate::tools::{Tool, ToolExecCtx, WEBSEARCH_TOOL_NAME};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::search::{available_engines, format_results, search_all};
use async_trait::async_trait;
use serde_json::Value;

const MAX_QUERY_LENGTH: usize = 1000;
/// Client-side page-fetch budget: at most this many results without
/// provider-supplied content are fetched per search.
const FETCH_BUDGET: usize = 3;

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
                    "description": "Whether to include page content (default: true). When enabled, content the search engine already extracted server-side is included for every result that has it; up to 3 pages without it are fetched directly.",
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
            return Ok(ToolOutput::error(
                "Search query cannot be empty".to_string(),
            ));
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

        // Fetch content from top results if requested. Provider-supplied
        // content (server-side page crawling) is used for every result that
        // has it, uncapped; the client-side fetch budget applies only to
        // results still missing content, and fetched text is truncated the
        // same way.
        let contents = if should_fetch {
            let (mut contents, missing) =
                crate::utils::search::plan_contents(&results, FETCH_BUDGET);
            let fetches: Vec<_> = missing
                .into_iter()
                .map(|(i, url)| async move {
                    match crate::utils::search::fetch_content(&url).await {
                        Ok(text) => Some((i, text)),
                        Err(_) => None,
                    }
                })
                .collect();
            contents.extend(
                futures::future::join_all(fetches)
                    .await
                    .into_iter()
                    .flatten(),
            );
            contents
        } else {
            Vec::new()
        };

        let output = format_results(&results, &contents);
        let summary = format!(
            "Search results for: '{}' ({} results{})",
            query,
            results.len(),
            if should_fetch && !contents.is_empty() {
                format!(", page content from {} results", contents.len())
            } else {
                String::new()
            }
        );

        Ok(ToolOutput::text_with_summary(output, summary))
    }
}

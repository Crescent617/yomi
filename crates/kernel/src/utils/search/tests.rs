use super::*;
use std::sync::{Arc, Mutex};

struct StubEngine {
    name: &'static str,
    outcome: Result<Vec<SearchResult>, String>,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl SearchEngine for StubEngine {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SearchResult>, String> {
        self.calls.lock().unwrap().push(self.name);
        self.outcome.clone()
    }
}

#[test]
fn test_encode_query() {
    assert_eq!(encode_query("hello world"), "hello+world");
    assert_eq!(encode_query("a:b"), "a:b");
}

#[test]
fn test_merge_results() {
    let a = vec![SearchResult {
        title: "A1".to_string(),
        url: "https://a1".to_string(),
        snippet: String::new(),
        source: "a",
        content: None,
    }];
    let b = vec![SearchResult {
        title: "B1".to_string(),
        url: "https://b1".to_string(),
        snippet: String::new(),
        source: "b",
        content: None,
    }];
    let merged = merge_results(&[a, b], 10);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].url, "https://a1");
    assert_eq!(merged[1].url, "https://b1");
}

#[test]
fn test_merge_results_dedup() {
    let a = vec![SearchResult {
        title: "A1".to_string(),
        url: "https://dup".to_string(),
        snippet: String::new(),
        source: "a",
        content: None,
    }];
    let b = vec![SearchResult {
        title: "B1".to_string(),
        url: "https://dup".to_string(),
        snippet: String::new(),
        source: "b",
        content: None,
    }];
    let merged = merge_results(&[a, b], 10);
    assert_eq!(merged.len(), 1);
}

#[test]
fn provider_content_preferred_and_truncated() {
    let with_content = SearchResult {
        title: "t".to_string(),
        url: "https://example.com".to_string(),
        snippet: String::new(),
        source: "kimi",
        content: Some("x".repeat(5_000)),
    };
    let text = provider_content(&with_content).expect("provider content should be used");
    assert!(text.len() < 5_000);
    assert!(text.contains("[Content truncated]"));

    let empty_content = SearchResult {
        content: Some("   ".to_string()),
        ..with_content.clone()
    };
    assert!(provider_content(&empty_content).is_none());

    let no_content = SearchResult {
        content: None,
        ..with_content
    };
    assert!(provider_content(&no_content).is_none());
}

#[test]
fn plan_contents_uncaps_provider_content_and_budgets_fetches() {
    let mk = |url: &str, content: Option<&str>| SearchResult {
        title: "t".to_string(),
        url: url.to_string(),
        snippet: String::new(),
        source: "test",
        content: content.map(str::to_string),
    };
    let results = vec![
        mk("https://a", Some("provider text")),
        mk("https://b", None),
        mk("https://c", Some("   ")), // whitespace counts as missing
        mk("https://d", None),
        mk("https://e", None),
        mk("https://f", None),
    ];

    let (contents, missing) = plan_contents(&results, 3);

    // Provider content included for every result that has it, not counted
    // against the fetch budget.
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0].0, 0);
    assert_eq!(contents[0].1, "provider text");
    // Fetch shortlist covers only the first 3 results missing content.
    assert_eq!(
        missing.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(missing[0].1, "https://b");

    let (contents, missing) = plan_contents(&results, 0);
    assert!(contents.len() == 1 && missing.is_empty());
}

#[test]
fn test_format_results() {
    let results = vec![
        SearchResult {
            title: "Test Title 1".to_string(),
            url: "https://example.com/1".to_string(),
            snippet: "Snippet 1".to_string(),
            source: "ddg",
            content: None,
        },
        SearchResult {
            title: "Test Title 2".to_string(),
            url: "https://example.com/2".to_string(),
            snippet: "Snippet 2".to_string(),
            source: "bing",
            content: None,
        },
    ];

    let contents = vec![(0, "Full content for page 1".to_string())];

    let output = format_results(&results, &contents);

    assert!(output.contains("Test Title 1"));
    assert!(output.contains("Test Title 2"));
    assert!(output.contains("https://example.com/1"));
    assert!(output.contains("Full content for page 1"));
    assert!(output.contains("Source: ddg"));
    assert!(output.contains("Source: bing"));
}

#[tokio::test]
async fn search_all_stops_after_first_success() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let engines: Vec<Box<dyn SearchEngine>> = vec![
        Box::new(StubEngine {
            name: "searxng",
            outcome: Err("unavailable".to_string()),
            calls: Arc::clone(&calls),
        }),
        Box::new(StubEngine {
            name: "serper",
            outcome: Ok(vec![SearchResult {
                title: "result".to_string(),
                url: "https://example.com".to_string(),
                snippet: String::new(),
                source: "serper",
                content: None,
            }]),
            calls: Arc::clone(&calls),
        }),
        Box::new(StubEngine {
            name: "ddg",
            outcome: Err("must not run".to_string()),
            calls: Arc::clone(&calls),
        }),
    ];

    let results = search_all(&engines, "query", 5).await.unwrap();

    assert_eq!(results[0].source, "serper");
    assert_eq!(*calls.lock().unwrap(), vec!["searxng", "serper"]);
}

#[tokio::test]
async fn search_all_falls_back_after_empty_results() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let engines: Vec<Box<dyn SearchEngine>> = vec![
        Box::new(StubEngine {
            name: "searxng",
            outcome: Ok(Vec::new()),
            calls: Arc::clone(&calls),
        }),
        Box::new(StubEngine {
            name: "bing",
            outcome: Err("blocked".to_string()),
            calls: Arc::clone(&calls),
        }),
    ];

    let error = search_all(&engines, "query", 5).await.unwrap_err();

    assert_eq!(*calls.lock().unwrap(), vec!["searxng", "bing"]);
    assert_eq!(
        error,
        "All sources failed: searxng: no results; bing: blocked"
    );
}

// -- Integration tests requiring network --

#[tokio::test]
#[ignore = "network dependent - DDG Lite may be blocked by anti-bot"]
async fn test_ddg_search_live() {
    let engine = ddg::DdgEngine::new();
    let results = engine.search("Rust programming language", 3).await;
    assert!(results.is_ok(), "DDG search failed: {:?}", results.err());
    let results = results.unwrap();
    assert!(!results.is_empty(), "DDG returned no results");
    for r in &results {
        assert!(!r.title.is_empty());
        assert!(!r.url.is_empty());
    }
    println!("DDG results: {results:#?}");
}

#[tokio::test]
#[ignore = "network dependent - Bing HTML scraping may break"]
async fn test_bing_search_live() {
    let engine = bing::BingEngine::new();
    let results = engine.search("Rust programming language", 3).await;
    assert!(results.is_ok(), "Bing search failed: {:?}", results.err());
    let results = results.unwrap();
    assert!(!results.is_empty(), "Bing returned no results");
    for r in &results {
        assert!(!r.title.is_empty());
        assert!(!r.url.is_empty());
    }
    println!("Bing results: {results:#?}");
}

#[tokio::test]
#[ignore = "network dependent - requires SearXNG or other configured engine"]
async fn test_search_all_live() {
    let engines = available_engines();
    assert!(!engines.is_empty(), "No engines available");
    let results = search_all(&engines, "Rust programming language", 5).await;
    assert!(results.is_ok(), "search_all failed: {:?}", results.err());
    let results = results.unwrap();
    assert!(!results.is_empty(), "search_all returned no results");
    println!("Merged results: {results:#?}");
}

#[tokio::test]
#[ignore = "network dependent - requires SearXNG or other configured engine"]
async fn test_websearch_tool_live() {
    use crate::tools::{Tool, ToolExecCtx};
    use crate::types::MessageId;
    use tokio_util::sync::CancellationToken;

    let tool = crate::tools::websearch::WebSearchTool::new();
    let args = serde_json::json!({
        "query": "Rust programming language",
        "num_results": 3,
        "fetch_content": false
    });

    let ctx = ToolExecCtx {
        tool_call_id: "test_call_1",
        cancel_token: Some(CancellationToken::new()),
        working_dir: std::env::current_dir().unwrap_or_default(),
        session_id: "test_session".to_string(),
        message_id: MessageId::new(),
        turn: None,
        max_tool_output_length: 40_000,
    };

    let output = tool.exec(args, ctx).await;
    assert!(output.is_ok(), "Tool execution failed: {:?}", output.err());
    let output = output.unwrap();
    assert!(!output.contents.is_empty(), "Tool returned empty content");
    println!("Tool contents: {:#?}", output.contents);
    println!("Is error: {}", output.is_error);
}

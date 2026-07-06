use super::*;

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
    }];
    let b = vec![SearchResult {
        title: "B1".to_string(),
        url: "https://b1".to_string(),
        snippet: String::new(),
        source: "b",
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
    }];
    let b = vec![SearchResult {
        title: "B1".to_string(),
        url: "https://dup".to_string(),
        snippet: String::new(),
        source: "b",
    }];
    let merged = merge_results(&[a, b], 10);
    assert_eq!(merged.len(), 1);
}

#[test]
fn test_format_results() {
    let results = vec![
        SearchResult {
            title: "Test Title 1".to_string(),
            url: "https://example.com/1".to_string(),
            snippet: "Snippet 1".to_string(),
            source: "ddg",
        },
        SearchResult {
            title: "Test Title 2".to_string(),
            url: "https://example.com/2".to_string(),
            snippet: "Snippet 2".to_string(),
            source: "bing",
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
        parent_messages: None,
        cancel_token: Some(CancellationToken::new()),
        working_dir: std::env::current_dir().unwrap_or_default(),
        session_id: "test_session".to_string(),
        message_id: MessageId::new(),
        turn: None,
        skills: vec![],
        max_tool_output_length: 40_000,
    };

    let output = tool.exec(args, ctx).await;
    assert!(output.is_ok(), "Tool execution failed: {:?}", output.err());
    let output = output.unwrap();
    assert!(!output.contents.is_empty(), "Tool returned empty content");
    println!("Tool contents: {:#?}", output.contents);
    println!("Is error: {}", output.is_error);
}

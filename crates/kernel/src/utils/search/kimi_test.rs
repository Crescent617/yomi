#[test]
fn parse_search_results_keeps_server_content() {
    use super::parse_search_results;
    let json = serde_json::json!({
        "search_results": [
            {
                "title": "Rust",
                "url": "https://www.rust-lang.org",
                "snippet": "A language",
                "content": "extracted page text",
                "date": "2026-10-01"
            },
            {
                "title": "Empty content",
                "url": "https://example.com/empty",
                "snippet": "s",
                "content": "   "
            },
            {
                "title": "No content",
                "url": "https://example.com/none",
                "snippet": "s"
            }
        ]
    });

    let results = parse_search_results(&json, 10).expect("parse should succeed");
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].content.as_deref(), Some("extracted page text"));
    assert!(
        results[1].content.is_none(),
        "whitespace-only content dropped"
    );
    assert!(results[2].content.is_none());
}

#[test]
fn parse_search_results_skips_entries_missing_title_or_url() {
    use super::parse_search_results;
    let json = serde_json::json!({
        "search_results": [
            { "title": "", "url": "https://example.com" },
            { "title": "No URL" },
            { "title": "Valid", "url": "https://example.com/valid", "content": "c" }
        ]
    });
    let results = parse_search_results(&json, 10).expect("parse should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Valid");
}

#[test]
fn test_kimi_engine_name() {
    use super::KimiEngine;
    use crate::utils::search::SearchEngine;

    let engine = KimiEngine::new("test-key".to_string(), None);
    assert_eq!(engine.name(), "kimi");
}

#[tokio::test]
#[ignore = "network dependent - requires KIMI_AGENT_API_KEY"]
async fn test_kimi_search_live() {
    use super::KimiEngine;
    use crate::utils::search::SearchEngine;
    use std::env;

    let api_key =
        env::var("KIMI_AGENT_API_KEY").expect("KIMI_AGENT_API_KEY must be set for this test");

    let engine = KimiEngine::new(api_key, env::var("KIMI_SEARCH_ENDPOINT").ok());
    let results = engine.search("Rust programming language", 3).await;
    assert!(results.is_ok(), "Kimi search failed: {:?}", results.err());
    let results = results.unwrap();
    assert!(!results.is_empty(), "Kimi returned no results");
    for r in &results {
        assert!(!r.title.is_empty());
        assert!(!r.url.is_empty());
    }
    println!("Kimi results: {results:#?}");
}

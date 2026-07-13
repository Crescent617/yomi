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

    let engine = KimiEngine::new(api_key, None);
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

//! DDG engine tests. The `duckduckgo` crate owns the response parsing, so
//! only a live smoke test is possible here.

#[tokio::test]
#[ignore = "network dependent - scrapes DuckDuckGo Lite"]
async fn ddg_lite_search_returns_results() {
    use crate::utils::search::{SearchEngine, SearchResult};

    let engine = super::DdgEngine::new();
    let results: Vec<SearchResult> = engine
        .search("Rust programming language", 3)
        .await
        .expect("DDG Lite search should succeed");

    assert!(!results.is_empty(), "DDG returned no results");
    for r in &results {
        assert!(!r.title.is_empty());
        assert!(!r.url.is_empty());
        assert_eq!(r.source, "ddg");
        assert!(r.content.is_none());
    }
}

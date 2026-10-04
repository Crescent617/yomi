use super::*;

#[test]
fn parses_results_with_content_as_snippet() {
    // SearXNG's JSON `content` field is the description text.
    let json = serde_json::json!({
        "results": [
            {
                "title": "Rust",
                "url": "https://www.rust-lang.org/",
                "content": "A language empowering everyone.",
                "engine": "google",
                "category": "general"
            },
            {
                "title": "Cargo Book",
                "url": "https://doc.rust-lang.org/cargo/",
                "content": ""
            }
        ]
    });

    let results = parse_results(&json, 10).expect("valid SearXNG response");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Rust");
    assert_eq!(results[0].url, "https://www.rust-lang.org/");
    assert_eq!(results[0].snippet, "A language empowering everyone.");
    assert_eq!(results[0].source, "searxng");
    assert!(
        results[0].content.is_none(),
        "SearXNG has no server-side page content"
    );
    assert_eq!(results[1].snippet, "");
}

#[test]
fn skips_entries_missing_title_or_url() {
    let json = serde_json::json!({
        "results": [
            { "title": "", "url": "https://example.com", "content": "c" },
            { "title": "No URL", "content": "c" },
            { "title": "Valid", "url": "https://example.com/valid", "content": "v" }
        ]
    });

    let results = parse_results(&json, 10).expect("valid SearXNG response");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Valid");
}

#[test]
fn rejects_missing_results_array() {
    let error = parse_results(&serde_json::json!({}), 5).expect_err("missing results must fail");

    assert_eq!(error, "SearXNG response missing results");
}

#[test]
fn rejects_empty_results() {
    let error = parse_results(&serde_json::json!({ "results": [] }), 5)
        .expect_err("empty results must fail");

    assert_eq!(error, "SearXNG returned no results");
}

use super::*;

#[test]
fn parses_web_results() {
    let json = serde_json::json!({
        "web": {
            "results": [
                {
                    "title": "Rust",
                    "url": "https://www.rust-lang.org/",
                    "description": "A language empowering everyone.",
                    "age": "2026-09-01"
                },
                {
                    "title": "No description",
                    "url": "https://example.com/2"
                }
            ]
        }
    });

    let results = parse_results(&json, 10).expect("valid Brave response");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Rust");
    assert_eq!(results[0].url, "https://www.rust-lang.org/");
    assert_eq!(results[0].snippet, "A language empowering everyone.");
    assert_eq!(results[0].source, "brave");
    assert!(
        results[0].content.is_none(),
        "Brave web search has no server-side page content"
    );
    assert_eq!(results[1].snippet, "");
}

#[test]
fn skips_entries_missing_title_or_url() {
    let json = serde_json::json!({
        "web": {
            "results": [
                { "title": "", "url": "https://example.com" },
                { "title": "No URL" },
                { "title": "Valid", "url": "https://example.com/valid" }
            ]
        }
    });

    let results = parse_results(&json, 10).expect("valid Brave response");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Valid");
}

#[test]
fn rejects_missing_web_results() {
    let error =
        parse_results(&serde_json::json!({}), 5).expect_err("missing web.results must fail");

    assert_eq!(error, "Brave response missing web.results");
}

#[test]
fn rejects_empty_web_results() {
    let error = parse_results(&serde_json::json!({ "web": { "results": [] } }), 5)
        .expect_err("empty web.results must fail");

    assert_eq!(error, "Brave returned no results");
}

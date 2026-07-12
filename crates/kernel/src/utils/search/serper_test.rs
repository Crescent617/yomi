use super::*;

#[test]
fn parses_organic_results() {
    let json = serde_json::json!({
        "organic": [
            {
                "title": "Rust",
                "link": "https://www.rust-lang.org/",
                "snippet": "A language empowering everyone."
            },
            {
                "title": "Cargo",
                "link": "https://doc.rust-lang.org/cargo/"
            }
        ]
    });

    let results = parse_results(&json, 1).expect("valid Serper response");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Rust");
    assert_eq!(results[0].url, "https://www.rust-lang.org/");
    assert_eq!(results[0].snippet, "A language empowering everyone.");
    assert_eq!(results[0].source, "serper");
}

#[test]
fn rejects_missing_organic_results() {
    let error = parse_results(&serde_json::json!({}), 5).expect_err("missing organic must fail");

    assert_eq!(error, "Serper response missing organic results");
}

#[test]
fn rejects_empty_organic_results() {
    let error = parse_results(&serde_json::json!({ "organic": [] }), 5)
        .expect_err("empty organic must fail");

    assert_eq!(error, "Serper returned no results");
}

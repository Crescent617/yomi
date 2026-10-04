use super::*;

const BING_HTML: &str = r#"
<html><body>
<ol id="b_results">
  <li class="b_algo">
    <h2><a href="https://www.rust-lang.org/">Rust Programming Language</a></h2>
    <div class="b_caption"><p>A language empowering everyone to build software.</p></div>
  </li>
  <li class="b_algo">
    <h2><a href="https://example.com/no-snippet">No Snippet Page</a></h2>
  </li>
  <li class="b_algo">
    <div class="b_caption"><p>Missing title and link</p></div>
  </li>
</ol>
</body></html>
"#;

#[test]
fn parses_bing_result_cards() {
    let results = parse_results(BING_HTML, 10).expect("valid Bing HTML");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Rust Programming Language");
    assert_eq!(results[0].url, "https://www.rust-lang.org/");
    assert_eq!(
        results[0].snippet,
        "A language empowering everyone to build software."
    );
    assert_eq!(results[0].source, "bing");
    assert!(results[0].content.is_none());
    assert_eq!(results[1].snippet, "");
}

#[test]
fn applies_limit() {
    let results = parse_results(BING_HTML, 1).expect("valid Bing HTML");
    assert_eq!(results.len(), 1);
}

#[test]
fn rejects_html_without_results() {
    let error = parse_results("<html><body><p>nothing</p></body></html>", 5)
        .expect_err("no result cards must fail");

    assert_eq!(error, "No Bing search results found");
}

#[test]
fn decodes_bing_redirect_urls() {
    // Bing wraps outbound links: u=a1<base64-url> (without the scheme padding).
    let wrapped = "https://www.bing.com/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS8=&ntb=1";
    assert_eq!(decode_url(wrapped), "https://example.com/");

    // Protocol-relative payloads get an https prefix.
    let relative = "https://www.bing.com/ck/a?u=a1Ly9leGFtcGxlLmNvbS8=";
    assert_eq!(decode_url(relative), "https://example.com/");

    // Undecodable wrappers fall back to the original URL.
    let plain = "https://example.com/direct";
    assert_eq!(decode_url(plain), plain);
}

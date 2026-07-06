//! HTML content extraction utilities
//!
//! Provides functions to extract clean text from HTML by filtering out
//! scripts, styles, navigation, ads, and other noise elements.

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::sync::LazyLock;

/// Pre-compiled regex patterns for noise removal
static NOISE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)<nav\b[^>]*>[\s\S]*?</nav\s*>",
        r"(?i)<header\b[^>]*>[\s\S]*?</header\s*>",
        r"(?i)<footer\b[^>]*>[\s\S]*?</footer\s*>",
        r"(?i)<aside\b[^>]*>[\s\S]*?</aside\s*>",
        r"(?i)<menu\b[^>]*>[\s\S]*?</menu\s*>",
        r"(?i)<!--[\s\S]*?-->", // HTML comments
    ]
    .iter()
    .filter_map(|p| Regex::new(p).ok())
    .collect()
});

/// Pre-compiled selectors for content extraction
static MAIN_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("main, article, [role='main']").expect("valid selector"));
static BODY_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("body").expect("valid selector"));

/// Extract main content from HTML by filtering noise and converting to text
///
/// # Arguments
/// * `html` - Raw HTML string
///
/// # Returns
/// Clean text content with scripts, styles, navigation removed
pub fn extract_content(html: &str) -> String {
    // First pass: regex-based removal of script/style/noscript tags and their contents
    let cleaned = remove_tags_with_content(html, &["script", "style", "noscript"]);

    // Remove navigation/footer/etc by pre-compiled regex patterns
    let mut cleaned_html = cleaned;
    for pattern in NOISE_PATTERNS.iter() {
        cleaned_html = pattern.replace_all(&cleaned_html, "").to_string();
    }

    // Try to extract main/article content first
    let document = Html::parse_document(&cleaned_html);

    // Try to find main content area
    let text = if let Some(main) = document.select(&MAIN_SELECTOR).next() {
        extract_text_from_element(&main)
    } else {
        // Fallback: extract from body
        document.select(&BODY_SELECTOR).next().map_or_else(
            || {
                // Last resort: use html2text
                html2text::from_read(cleaned_html.as_bytes(), 120)
                    .unwrap_or_else(|_| simple_html_to_text(&cleaned_html))
            },
            |body| extract_text_from_element(&body),
        )
    };

    // Normalize whitespace (collapse multiple spaces)
    normalize_whitespace(&text)
}

/// Normalize whitespace in text (collapse multiple spaces into one)
fn normalize_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_was_space = true; // Start true to trim leading spaces

    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_was_space {
                result.push(' ');
                prev_was_space = true;
            }
        } else {
            result.push(c);
            prev_was_space = false;
        }
    }

    // Trim trailing space if any
    if result.ends_with(' ') {
        result.pop();
    }

    result
}

/// Extract text from element recursively
fn extract_text_from_element(element: &ElementRef) -> String {
    let mut parts = Vec::new();
    walk_element(element, &mut parts);
    parts.join(" ")
}

/// Recursive helper to walk element tree and extract text
fn walk_element(element: &ElementRef, output: &mut Vec<String>) {
    // Skip known noise elements by tag name
    let tag_str = element.value().name();
    if matches!(
        tag_str,
        "nav" | "header" | "footer" | "aside" | "menu" | "script" | "style"
    ) {
        return;
    }

    // Check class/id attributes for noise indicators
    if has_noise_class_or_id(element) {
        return;
    }

    // Process children
    for child in element.children() {
        match child.value() {
            scraper::Node::Text(text) => {
                let trimmed = text.text.trim();
                if !trimmed.is_empty() {
                    output.push(trimmed.to_string());
                }
            }
            scraper::Node::Element(_) => {
                if let Some(child_elem) = ElementRef::wrap(child) {
                    walk_element(&child_elem, output);
                }
            }
            _ => {}
        }
    }
}

/// Check if element has noise-related class or id attributes
fn has_noise_class_or_id(element: &ElementRef) -> bool {
    const NOISE_KEYWORDS: &[&str] = &[
        "nav",
        "menu",
        "sidebar",
        "ad",
        "advertisement",
        "cookie",
        "popup",
        "comment",
    ];

    let elem_val = element.value();
    for (name, value) in elem_val.attrs() {
        if name == "class" || name == "id" {
            let v_lower = value.to_lowercase();
            if NOISE_KEYWORDS.iter().any(|&kw| v_lower.contains(kw)) {
                return true;
            }
        }
    }
    false
}

/// Remove specific tags and their content using regex
fn remove_tags_with_content(html: &str, tags: &[&str]) -> String {
    let mut result = html.to_string();

    for tag in tags {
        // Pattern to match opening tag, content, and closing tag (case-insensitive)
        let pattern = format!(
            "(?i)<{}\\b[^>]*>[\\s\\S]*?</{}\\s*>",
            regex::escape(tag),
            regex::escape(tag)
        );

        if let Ok(re) = Regex::new(&pattern) {
            result = re.replace_all(&result, "").to_string();
        }
    }

    result
}

/// Simple HTML to text fallback - strips tags and normalizes whitespace
fn simple_html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut prev_was_space = true;

    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !prev_was_space {
                    text.push(' ');
                    prev_was_space = true;
                }
            }
            _ if !in_tag => {
                if c.is_whitespace() {
                    if !prev_was_space {
                        text.push(' ');
                        prev_was_space = true;
                    }
                } else {
                    text.push(c);
                    prev_was_space = false;
                }
            }
            _ => {}
        }
    }

    text.trim().to_string()
}

#[cfg(test)]
#[path = "html_test.rs"]
mod tests;

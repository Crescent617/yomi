use super::*;

#[test]
fn input_uses_only_text_blocks() {
    let blocks = vec![
        ContentBlock::Text {
            text: " hello ".to_string(),
        },
        ContentBlock::ImageUrl {
            image_url: crate::types::ImageUrl {
                url: "data:image/png;base64,data".to_string(),
                detail: None,
            },
        },
        ContentBlock::Text {
            text: "world ".to_string(),
        },
    ];

    assert_eq!(input_from_blocks(&blocks).as_deref(), Some("hello world"));
}

#[test]
fn input_is_limited_to_200_chars() {
    let input = "你".repeat(201);
    let blocks = vec![ContentBlock::Text { text: input }];

    assert_eq!(input_from_blocks(&blocks).unwrap().chars().count(), 200);
}

#[test]
fn input_ignores_empty_text() {
    let blocks = vec![ContentBlock::Text {
        text: "  \n ".to_string(),
    }];

    assert_eq!(input_from_blocks(&blocks), None);
}

#[test]
fn generation_requires_more_than_10_chars() {
    assert!(!should_generate("1234567890"));
    assert!(should_generate("12345678901"));
    assert!(!should_generate("你".repeat(10).as_str()));
    assert!(should_generate("你".repeat(11).as_str()));
}

#[test]
fn generated_title_is_cleaned() {
    assert_eq!(
        clean_generated_title("# 标题：排查登录接口。\n其他内容"),
        "排查登录接口"
    );
    assert_eq!(
        clean_generated_title("\"Implement OAuth Login\""),
        "Implement OAuth Logi"
    );
}

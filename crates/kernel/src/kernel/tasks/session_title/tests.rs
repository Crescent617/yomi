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
fn generation_requires_only_the_feature_flag() {
    assert!(!should_generate(false));
    assert!(should_generate(true));
}

#[test]
fn generation_input_includes_current_title_and_latest_prompt() {
    assert_eq!(
        generation_input(Some("代码高亮"), "复制按钮只在 hover 显示"),
        "Current title:\n代码高亮\n\nLatest user prompt:\n复制按钮只在 hover 显示"
    );
    assert_eq!(
        generation_input(None, "设计代码高亮"),
        "Latest user prompt:\n设计代码高亮"
    );
}

#[test]
fn fallback_uses_the_latest_query() {
    assert_eq!(
        fallback_title("  fix   session title when model fails  "),
        "fix session title wh"
    );
    assert_eq!(fallback_title("你好"), "你好");
}

#[test]
fn title_model_config_disables_thinking_and_allows_output() {
    let source = ModelConfig {
        max_tokens: Some(1),
        thinking: ThinkingConfig {
            enabled: true,
            budget_tokens: 4096,
            effort: Some("high".to_string()),
        },
        ..ModelConfig::default()
    };

    let config = title_model_config(&source);

    assert_eq!(config.max_tokens, Some(64));
    assert!(!config.thinking.enabled);
    assert_eq!(config.thinking.effort, None);
    assert_eq!(config.thinking.budget_tokens, 2048);
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

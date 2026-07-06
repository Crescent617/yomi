use super::*;

#[test]
fn test_check_access_disabled() {
    let config = ChannelConfig {
        name: "test".to_string(),
        enabled: false,
        platform: PlatformConfig::Telegram {
            token: String::new(),
        },
        ..Default::default()
    };
    assert!(matches!(
        config.check_access("chat1", "user1"),
        Err(ChannelError::Disabled(_))
    ));
}

#[test]
fn test_check_access_blocked_user() {
    let config = ChannelConfig {
        name: "test".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: String::new(),
        },
        blocked_users: vec!["bad_user".to_string()],
        ..Default::default()
    };
    assert!(config.check_access("chat1", "bad_user").is_err());
    assert!(config.check_access("chat1", "good_user").is_ok());
}

#[test]
fn test_check_access_blocked_chat() {
    let config = ChannelConfig {
        name: "test".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: String::new(),
        },
        blocked_chats: vec!["bad_chat".to_string()],
        ..Default::default()
    };
    assert!(config.check_access("bad_chat", "user1").is_err());
    assert!(config.check_access("good_chat", "user1").is_ok());
}

#[test]
fn test_check_access_allowed_users() {
    let config = ChannelConfig {
        name: "test".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: String::new(),
        },
        allowed_users: vec!["alice".to_string()],
        ..Default::default()
    };
    assert!(config.check_access("chat1", "alice").is_ok());
    assert!(config.check_access("chat1", "bob").is_err());
}

#[test]
fn test_check_access_allowed_chats() {
    let config = ChannelConfig {
        name: "test".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: String::new(),
        },
        allowed_chats: vec!["group1".to_string()],
        ..Default::default()
    };
    assert!(config.check_access("group1", "user1").is_ok());
    assert!(config.check_access("group2", "user1").is_err());
}

#[test]
fn test_check_access_blocklist_wins() {
    // Blocked user should be denied even if in allowed_users
    let config = ChannelConfig {
        name: "test".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: String::new(),
        },
        allowed_users: vec!["alice".to_string()],
        blocked_users: vec!["alice".to_string()],
        ..Default::default()
    };
    assert!(config.check_access("chat1", "alice").is_err());
}

#[test]
fn test_blocks_to_text_text_only() {
    let blocks = vec![
        ContentBlock::Text {
            text: "hello".into(),
        },
        ContentBlock::Text {
            text: "world".into(),
        },
    ];
    assert_eq!(blocks_to_text(&blocks), "hello\nworld");
}

#[test]
fn test_blocks_to_text_mixed() {
    let blocks = vec![
        ContentBlock::Text {
            text: "text".into(),
        },
        ContentBlock::Thinking {
            thinking: "thinking".into(),
            signature: None,
        },
        ContentBlock::RedactedThinking {
            data: "redacted".into(),
        },
        ContentBlock::ImageUrl {
            image_url: crate::types::ImageUrl {
                url: "http://example.com/img.png".into(),
                detail: None,
            },
        },
        ContentBlock::Audio {
            audio: crate::types::AudioData {
                format: "mp3".into(),
                data: "data".into(),
            },
        },
    ];
    // Thinking/redacted blocks are stripped so they don't leak to external platforms
    assert_eq!(
        blocks_to_text(&blocks),
        "text\n[image: http://example.com/img.png]\n[audio: mp3]"
    );
}

#[test]
fn test_blocks_to_text_empty() {
    assert_eq!(blocks_to_text(&[]), "");
}

//! Tests for the watch tee's content assembly.

use super::*;

fn msg(content: Vec<ContentBlock>, image_keys: Vec<&str>) -> ChannelMessage {
    ChannelMessage {
        external_chat_id: "oc_chat".to_string(),
        external_user_id: "ou_user".to_string(),
        external_message_id: Some("om_msg".to_string()),
        is_mention: false,
        raw_text: None,
        content,
        image_keys: image_keys.into_iter().map(str::to_string).collect(),
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    }
}

#[test]
fn mirror_content_keeps_message_blocks_verbatim() {
    let blocks = vec![ContentBlock::Text {
        text:
            "[ts][from_user_id: ou_user][chat_id: oc_chat][msg_id: om_msg][platform: feishu]\nhello"
                .to_string(),
    }];
    let out = mirror_content(&msg(blocks.clone(), vec![]));
    assert_eq!(out, blocks);
}

#[test]
fn mirror_content_appends_image_refs_as_text() {
    let out = mirror_content(&msg(
        vec![ContentBlock::Text {
            text: "header".to_string(),
        }],
        vec!["img_k1", "img_k2"],
    ));
    assert_eq!(out.len(), 2);
    let ContentBlock::Text { text } = &out[1] else {
        panic!("expected text block");
    };
    assert_eq!(text, "[image: img_k1] [image: img_k2]");
}

#[test]
fn mirror_content_truncates_oversized_text_blocks() {
    let out = mirror_content(&msg(
        vec![ContentBlock::Text {
            text: "汉".repeat(5000),
        }],
        vec![],
    ));
    let ContentBlock::Text { text } = &out[0] else {
        panic!("expected text block");
    };
    let marker = "…(已截断)";
    assert!(text.ends_with(marker), "{text}");
    assert_eq!(
        text.chars().count(),
        MIRROR_TEXT_CAP + marker.chars().count(),
        "kept chars + marker"
    );
    assert!(text.starts_with(&"汉".repeat(100)), "head preserved");

    // Exactly at the cap: untouched, no marker (multibyte-safe either
    // way — `truncate_chars` works on chars, never bytes).
    let exact = "a".repeat(MIRROR_TEXT_CAP);
    let out = mirror_content(&msg(
        vec![ContentBlock::Text {
            text: exact.clone(),
        }],
        vec![],
    ));
    assert_eq!(
        out,
        vec![ContentBlock::Text { text: exact }],
        "cap-boundary text stays verbatim"
    );
}

#[test]
fn mapping_kind_roundtrip() {
    assert_eq!(MappingKind::Watch.as_str(), "watch");
    assert_eq!(MappingKind::Normal.as_str(), "normal");
    assert_eq!(MappingKind::from_str_lossy("watch"), MappingKind::Watch);
    assert_eq!(MappingKind::from_str_lossy("normal"), MappingKind::Normal);
    // Unknown/legacy values degrade to Normal.
    assert_eq!(MappingKind::from_str_lossy(""), MappingKind::Normal);
    assert_eq!(MappingKind::from_str_lossy("whatever"), MappingKind::Normal);
}

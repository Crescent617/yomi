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
fn watch_mapping_key_is_namespaced() {
    assert_eq!(
        crate::channels::watch_mapping_key("oc_chat"),
        "watch:oc_chat"
    );
    assert!(crate::channels::watch_mapping_key("oc_chat")
        .starts_with(crate::channels::WATCH_KEY_PREFIX));
}

#[test]
fn mapping_kind_roundtrip() {
    assert_eq!(MappingKind::Watch.as_str(), "watch");
    assert_eq!(MappingKind::WatchPaused.as_str(), "watch_off");
    assert_eq!(MappingKind::from_str_lossy("watch"), MappingKind::Watch);
    assert_eq!(
        MappingKind::from_str_lossy("watch_off"),
        MappingKind::WatchPaused
    );
    assert_eq!(MappingKind::from_str_lossy("normal"), MappingKind::Normal);
    // Unknown/legacy values degrade to Normal.
    assert_eq!(MappingKind::from_str_lossy(""), MappingKind::Normal);
    assert_eq!(MappingKind::from_str_lossy("whatever"), MappingKind::Normal);
}

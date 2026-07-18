use super::mark_user_steer;
use crate::types::{ContentBlock, ImageUrl};

#[test]
fn user_steer_prefixes_the_first_text_block() {
    assert_eq!(
        mark_user_steer(vec![ContentBlock::Text {
            text: "change direction".to_string(),
        }]),
        vec![ContentBlock::Text {
            text: "[From User] change direction".to_string(),
        }]
    );
}

#[test]
fn user_steer_inserts_prefix_before_non_text_content() {
    let image = ContentBlock::ImageUrl {
        image_url: ImageUrl {
            url: "data:image/png;base64,abc".to_string(),
            detail: None,
        },
    };

    assert_eq!(
        mark_user_steer(vec![image.clone()]),
        vec![
            ContentBlock::Text {
                text: "[From User] ".to_string(),
            },
            image,
        ]
    );
}

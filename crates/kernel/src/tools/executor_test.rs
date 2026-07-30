use super::*;
use crate::types::ToolOutputBlock;
use crate::utils::image::test_utils::{noisy_png, to_data_url};

fn image_url_of(msg: &Message) -> &str {
    msg.content
        .iter()
        .find_map(|b| {
            if let ContentBlock::ImageUrl { image_url } = b {
                Some(image_url.url.as_str())
            } else {
                None
            }
        })
        .expect("image block")
}

#[tokio::test]
async fn tool_result_image_stays_within_provider_cap() {
    let png = noisy_png(2000, 1300);
    let url = to_data_url("image/png", &png);
    let output = ToolOutput::image(url);

    let (_, mut msg) =
        build_tool_result("call-1", "screenshot", &output, 0, MessageId::new(), 10_000);
    // Mirror the tool_exec call site: normalization is a separate step.
    crate::utils::image::normalize_image_blocks(&mut msg.content).await;

    let url = image_url_of(&msg);
    assert!(
        url.starts_with("data:image/jpeg;base64,"),
        "recompressed: {}...",
        &url[..30]
    );
    let b64 = url.split_once(',').unwrap().1;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).unwrap();
    assert!(
        bytes.len() <= crate::utils::image::MAX_EMBED_IMAGE_BYTES,
        "{} bytes",
        bytes.len()
    );
}

#[tokio::test]
async fn tool_result_small_image_and_remote_url_untouched() {
    let small = to_data_url("image/png", &noisy_png(16, 16));
    let output = ToolOutput {
        contents: vec![
            ToolOutputBlock::Image {
                url: small.clone(),
                mime_type: None,
            },
            ToolOutputBlock::Image {
                url: "https://example.com/x.png".into(),
                mime_type: None,
            },
        ],
        is_error: false,
    };

    let (_, mut msg) =
        build_tool_result("call-1", "screenshot", &output, 0, MessageId::new(), 10_000);
    crate::utils::image::normalize_image_blocks(&mut msg.content).await;

    let urls: Vec<&str> = msg
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::ImageUrl { image_url } = b {
                Some(image_url.url.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(urls, [small.as_str(), "https://example.com/x.png"]);
}

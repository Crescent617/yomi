use super::*;
use crate::utils::image::test_utils::{decode_data_url, noisy_png, to_data_url};

#[tokio::test]
async fn extract_stores_compressed_asset() {
    let dir = tempfile::tempdir().unwrap();
    let mut msg = crate::types::Message::user_with_image(
        "look",
        to_data_url("image/png", &noisy_png(2000, 1300)),
    );

    extract_inline_image(&mut msg, dir.path()).await;

    let crate::types::ContentBlock::ImageUrl { image_url } = &msg.content[1] else {
        panic!("expected image block");
    };
    let asset_url = image_url.url.clone();
    assert!(asset_url.starts_with("asset://"), "url: {asset_url}");

    // The stored asset is the compressed form — jpeg, within the cap.
    let bytes = read_asset(&asset_url, dir.path()).await.expect("asset");
    assert!(
        bytes.len() as u64 <= crate::utils::image::MAX_EMBED_IMAGE_BYTES as u64,
        "{} bytes",
        bytes.len()
    );
    assert_eq!(
        crate::utils::image::detect_mime_type(&bytes),
        Some("image/jpeg")
    );
}

#[tokio::test]
async fn inline_recompresses_legacy_oversized_asset() {
    let dir = tempfile::tempdir().unwrap();
    // Simulate a pre-compression-era asset: stored raw, over the cap.
    let asset_url = store_inline_image(
        &to_data_url("image/png", &noisy_png(2000, 1300)),
        dir.path(),
    )
    .await
    .expect("stored");
    let mut msg = crate::types::Message::user_with_image("look", asset_url);

    inline_assets_in_message(&mut msg, dir.path()).await;

    let crate::types::ContentBlock::ImageUrl { image_url } = &msg.content[1] else {
        panic!("expected image block, got {:?}", msg.content);
    };
    assert!(
        image_url.url.starts_with("data:image/jpeg;base64,"),
        "recompressed on inline: {}...",
        &image_url.url[..30]
    );
    let bytes = decode_data_url(&image_url.url).1;
    assert!(
        bytes.len() <= crate::utils::image::MAX_EMBED_IMAGE_BYTES,
        "{} bytes",
        bytes.len()
    );
}

#[tokio::test]
async fn inline_keeps_small_asset_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let data_url = "data:image/png;base64,aGVsbG8=";
    let asset_url = store_inline_image(data_url, dir.path())
        .await
        .expect("stored");
    let mut msg = crate::types::Message::user_with_image("look", asset_url);

    inline_assets_in_message(&mut msg, dir.path()).await;

    let crate::types::ContentBlock::ImageUrl { image_url } = &msg.content[1] else {
        panic!("expected image block");
    };
    assert_eq!(image_url.url, data_url, "untouched");
}

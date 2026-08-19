use super::*;
use crate::utils::image::test_utils::{decode_data_url, noisy_png, to_data_url};

#[test]
fn asset_path_rejects_traversal() {
    let dir = std::path::Path::new("/data");
    assert!(asset_path("asset://abc123.png", dir).is_some());
    for url in [
        "asset://../secret",
        "asset://a/b.png",
        "asset://..",
        "asset://",
        "file:///etc/passwd",
    ] {
        assert!(asset_path(url, dir).is_none(), "{url} must not resolve");
    }
}

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
    let path = asset_path(&asset_url, dir.path()).expect("asset path");
    let bytes = tokio::fs::read(&path).await.expect("asset");
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

#[tokio::test]
async fn process_persists_data_url_and_annotates_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let blocks = vec![
        crate::types::ContentBlock::Text {
            text: "look".to_string(),
        },
        crate::types::ContentBlock::ImageUrl {
            image_url: to_data_url("image/png", b"hello-png").into(),
        },
        crate::types::ContentBlock::ImageUrl {
            image_url: to_data_url("image/png", b"hello-png-2").into(),
        },
    ];

    let out = process_image_blocks(blocks, dir.path()).await;

    // text, img, [image 1: …], img, [image 2: …]
    assert_eq!(out.len(), 5, "{out:?}");
    for (idx, marker) in [(2usize, "[image 1: "), (4usize, "[image 2: ")] {
        let crate::types::ContentBlock::Text { text } = &out[idx] else {
            panic!("expected annotation at {idx}, got {:?}", out[idx]);
        };
        assert!(text.starts_with(marker), "{text}");
        let path = text.trim_start_matches(marker).trim_end_matches(']');
        let path = std::path::Path::new(path);
        assert!(path.is_absolute(), "{path:?} must be absolute");
        assert!(path.exists(), "{path:?} must exist on disk");
        assert!(path.starts_with(dir.path().join("assets")));
    }
    // Images stay inline for vision.
    assert!(matches!(
        &out[1],
        crate::types::ContentBlock::ImageUrl { image_url } if image_url.url.starts_with("data:")
    ));
    // Dedup: same bytes → same asset file.
    let p1 = match &out[2] {
        crate::types::ContentBlock::Text { text } => text.clone(),
        _ => panic!(),
    };
    assert!(
        p1.contains(".png") || p1.contains(".jpg") || p1.contains(".jpeg"),
        "{p1}"
    );
}

#[tokio::test]
async fn process_resolves_asset_url_and_annotates() {
    let dir = tempfile::tempdir().unwrap();
    let asset_url = store_inline_image("data:image/png;base64,aGVsbG8=", dir.path())
        .await
        .expect("stored");
    let abs = asset_path(&asset_url, dir.path()).unwrap();
    let blocks = vec![crate::types::ContentBlock::ImageUrl {
        image_url: asset_url.into(),
    }];

    let out = process_image_blocks(blocks, dir.path()).await;

    assert_eq!(out.len(), 2, "{out:?}");
    let crate::types::ContentBlock::ImageUrl { image_url } = &out[0] else {
        panic!("expected image block");
    };
    assert!(image_url.url.starts_with("data:"), "resolved inline");
    let crate::types::ContentBlock::Text { text } = &out[1] else {
        panic!("expected annotation");
    };
    assert_eq!(*text, format!("[image 1: {}]", abs.display()));
}

#[tokio::test]
async fn process_marks_missing_asset_unavailable_without_annotation() {
    let dir = tempfile::tempdir().unwrap();
    let blocks = vec![crate::types::ContentBlock::ImageUrl {
        image_url: "asset://deadbeef0123456789abcdef0123456789abcdef0123456789abcdef0123456789.png"
            .to_string()
            .into(),
    }];

    let out = process_image_blocks(blocks, dir.path()).await;

    assert_eq!(out.len(), 1, "{out:?}");
    let crate::types::ContentBlock::Text { text } = &out[0] else {
        panic!("expected placeholder");
    };
    assert!(text.starts_with("[image unavailable:"), "{text}");
}

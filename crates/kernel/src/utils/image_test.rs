use super::test_utils::*;
use super::*;

#[test]
fn test_detect_mime_type() {
    assert_eq!(detect_mime_type(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
    assert_eq!(detect_mime_type(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
    assert_eq!(detect_mime_type(b"GIF87a"), Some("image/gif"));
    assert_eq!(detect_mime_type(b"GIF89a"), Some("image/gif"));
    assert_eq!(detect_mime_type(b"RIFF____WEBP"), Some("image/webp"));
    assert_eq!(detect_mime_type(b"not an image"), None);
}

#[test]
fn test_is_image_extension() {
    assert!(is_image_extension(Path::new("test.png")));
    assert!(is_image_extension(Path::new("test.jpg")));
    assert!(is_image_extension(Path::new("test.JPEG")));
    assert!(!is_image_extension(Path::new("test.txt")));
    assert!(!is_image_extension(Path::new("test.rs")));
}

#[test]
fn bytes_to_data_url_passes_small_images_through() {
    let png = noisy_png(32, 32);
    assert!(png.len() <= MAX_EMBED_IMAGE_BYTES);

    let url = bytes_to_data_url(&png).unwrap();

    let (mime, bytes) = decode_data_url(&url);
    assert_eq!(mime, "image/png");
    assert_eq!(bytes, png, "untouched bytes");
}

#[test]
fn bytes_to_data_url_rejects_non_images() {
    assert!(bytes_to_data_url(b"not an image").is_err());
}

#[test]
fn bytes_to_data_url_recompresses_oversized_images() {
    let png = noisy_png(3000, 2400);
    assert!(
        png.len() > MAX_EMBED_IMAGE_BYTES,
        "fixture must exceed the cap: {} bytes",
        png.len()
    );

    let url = bytes_to_data_url(&png).unwrap();

    let (mime, bytes) = decode_data_url(&url);
    assert_eq!(mime, "image/jpeg", "recompressed to jpeg");
    assert!(
        bytes.len() <= MAX_EMBED_IMAGE_BYTES,
        "fits provider cap: {} bytes",
        bytes.len()
    );
    // Still a valid, downscaled image.
    assert_eq!(detect_mime_type(&bytes), Some("image/jpeg"));
    let img = image::load_from_memory(&bytes).unwrap();
    assert!(img.width().max(img.height()) <= EMBED_MAX_DIMENSION);
    assert!(
        u64::from(img.width()) * u64::from(img.height()) <= u64::from(EMBED_MAX_PIXELS) + 10_000,
        "{}x{}",
        img.width(),
        img.height()
    );
}

#[test]
fn shrink_to_model_resolution_respects_both_caps() {
    // Square-ish: the megapixel cap binds (1568² = 2.46MP > 1.15MP).
    let img = shrink_to_model_resolution(noisy_dynamic_image(3000, 2400));
    assert!(
        u64::from(img.width()) * u64::from(img.height()) <= u64::from(EMBED_MAX_PIXELS) + 10_000,
        "{}x{}",
        img.width(),
        img.height()
    );
    let ratio = f64::from(img.width()) / f64::from(img.height());
    assert!((ratio - 3000.0 / 2400.0).abs() < 0.01, "aspect: {ratio}");

    // Wide and thin: the long-edge cap binds (pixels already < 1.15MP).
    let img = shrink_to_model_resolution(noisy_dynamic_image(4000, 600));
    assert!(img.width() <= EMBED_MAX_DIMENSION);
    assert!(img.width() >= EMBED_MAX_DIMENSION - 2, "{}", img.width());
    let ratio = f64::from(img.width()) / f64::from(img.height());
    assert!((ratio - 4000.0 / 600.0).abs() < 0.1, "aspect: {ratio}");

    // Already small: untouched.
    let img = shrink_to_model_resolution(noisy_dynamic_image(800, 600));
    assert_eq!((img.width(), img.height()), (800, 600));
}

fn noisy_dynamic_image(width: u32, height: u32) -> image::DynamicImage {
    image::load_from_memory(&noisy_png(width, height)).unwrap()
}

#[tokio::test]
async fn image_to_data_url_compresses_instead_of_failing() {
    // 5MB < file ≤ 10MB: previously passed through raw (providers could
    // reject), now recompressed under the embed cap.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.png");
    let png = noisy_png(2000, 1300);
    assert!(
        png.len() > MAX_EMBED_IMAGE_BYTES && png.len() as u64 <= MAX_IMAGE_SIZE,
        "fixture must sit between the two caps: {} bytes",
        png.len()
    );
    std::fs::write(&path, &png).unwrap();

    let url = image_to_data_url(&path).await.unwrap().expect("image");

    let (mime, bytes) = decode_data_url(&url);
    assert_eq!(mime, "image/jpeg");
    assert!(bytes.len() <= MAX_EMBED_IMAGE_BYTES);
}

#[test]
fn normalize_data_url_ignores_non_data_urls() {
    assert_eq!(normalize_data_url("https://example.com/x.png"), None);
    assert_eq!(normalize_data_url("asset://abc123.png"), None);
}

#[test]
fn normalize_data_url_keeps_small_images_untouched() {
    let png = noisy_png(32, 32);
    let url = bytes_to_data_url(&png).unwrap();

    assert_eq!(normalize_data_url(&url), None);
}

#[test]
fn normalize_data_url_recompresses_oversized_images() {
    let png = noisy_png(3000, 2400);
    let url = format!("data:image/png;base64,{}", encode_base64(&png));

    let normalized = normalize_data_url(&url).expect("recompressed");

    let (mime, bytes) = decode_data_url(&normalized);
    assert_eq!(mime, "image/jpeg");
    assert!(bytes.len() <= MAX_EMBED_IMAGE_BYTES);
}

#[tokio::test]
async fn normalize_image_blocks_rewrites_only_oversized_data_urls() {
    let small = bytes_to_data_url(&noisy_png(32, 32)).unwrap();
    let big = format!(
        "data:image/png;base64,{}",
        encode_base64(&noisy_png(3000, 2400))
    );
    let mut blocks = vec![
        crate::types::ContentBlock::ImageUrl {
            image_url: "https://example.com/remote.png".to_string().into(),
        },
        crate::types::ContentBlock::ImageUrl {
            image_url: small.clone().into(),
        },
        crate::types::ContentBlock::ImageUrl {
            image_url: big.into(),
        },
        crate::types::ContentBlock::Text { text: "hi".into() },
    ];

    normalize_image_blocks(&mut blocks).await;

    let urls: Vec<&str> = blocks
        .iter()
        .filter_map(|b| {
            if let crate::types::ContentBlock::ImageUrl { image_url } = b {
                Some(image_url.url.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        urls[0], "https://example.com/remote.png",
        "remote untouched"
    );
    assert_eq!(urls[1], small, "small untouched");
    assert!(
        urls[2].starts_with("data:image/jpeg;base64,"),
        "recompressed"
    );
}

#[test]
fn bytes_to_data_url_downscales_overresolution_png_losslessly() {
    let png = solid_image(4000, 3000, image::ImageFormat::Png);
    assert!(
        png.len() <= MAX_EMBED_IMAGE_BYTES,
        "fixture must be byte-compliant: {} bytes",
        png.len()
    );

    let url = bytes_to_data_url(&png).unwrap();

    // Resolution-only overage keeps PNG (lossless) — screenshots must
    // not go through the JPEG ladder.
    let (mime, bytes) = decode_data_url(&url);
    assert_eq!(mime, "image/png");
    let (w, h) = read_dimensions(&bytes).unwrap();
    assert!(within_model_resolution(w, h), "{w}x{h}");
}

#[test]
fn bytes_to_data_url_downscales_overresolution_jpeg() {
    let jpeg = solid_image(4000, 3000, image::ImageFormat::Jpeg);

    let url = bytes_to_data_url(&jpeg).unwrap();

    let (mime, bytes) = decode_data_url(&url);
    assert_eq!(mime, "image/jpeg");
    let (w, h) = read_dimensions(&bytes).unwrap();
    assert!(within_model_resolution(w, h), "{w}x{h}");
}

#[test]
fn normalize_data_url_downscales_overresolution_images() {
    let url = format!(
        "data:image/png;base64,{}",
        encode_base64(&solid_image(4000, 3000, image::ImageFormat::Png))
    );

    let normalized = normalize_data_url(&url).expect("downscaled");

    let (mime, bytes) = decode_data_url(&normalized);
    assert_eq!(mime, "image/png");
    let (w, h) = read_dimensions(&bytes).unwrap();
    assert!(within_model_resolution(w, h), "{w}x{h}");
}

#[test]
fn needs_compression_flags_both_triggers() {
    assert!(
        needs_compression(&solid_image(4000, 3000, image::ImageFormat::Png)),
        "over-resolution"
    );
    assert!(needs_compression(&noisy_png(2000, 1300)), "over bytes");
    assert!(
        !needs_compression(&solid_image(800, 600, image::ImageFormat::Png)),
        "compliant"
    );
}

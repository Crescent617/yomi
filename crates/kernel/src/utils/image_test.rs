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

/// Build an animated GIF from solid-color 64x48 frames (`delay_ms` each).
fn animated_gif(colors: &[[u8; 4]], delay_ms: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = image::codecs::gif::GifEncoder::new(&mut buf);
    let frames: Vec<image::Frame> = colors
        .iter()
        .map(|&rgba| {
            image::Frame::from_parts(
                image::ImageBuffer::from_pixel(64, 48, image::Rgba(rgba)),
                0,
                0,
                image::Delay::from_numer_denom_ms(delay_ms, 1),
            )
        })
        .collect();
    enc.encode_frames(frames).unwrap();
    drop(enc); // flush the trailer before reading buf
    buf
}

#[test]
fn probe_gif_info_counts_frames_and_delays() {
    let gif = animated_gif(&[[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]], 120);

    let info = probe_gif_info(&gif).unwrap();

    assert_eq!((info.width, info.height), (64, 48));
    assert_eq!(info.frames, 3);
    assert_eq!(info.duration_ms, 360);

    let single = animated_gif(&[[255, 0, 0, 255]], 120);
    assert_eq!(probe_gif_info(&single).unwrap().frames, 1);
}

#[test]
fn probe_gif_info_rejects_non_gif_and_truncated_data() {
    assert_eq!(probe_gif_info(b"not a gif"), None);
    assert_eq!(probe_gif_info(b"GIF89a"), None);
    // Header + screen descriptor only: no image descriptor yet.
    assert_eq!(probe_gif_info(b"GIF89a\x01\x00\x01\x00\x00\x00\x00"), None);
    // Truncated after the first frame: keeps the frames counted so far.
    let gif = animated_gif(&[[255, 0, 0, 255], [0, 0, 255, 255]], 100);
    let cut = gif.len() * 3 / 4;
    let info = probe_gif_info(&gif[..cut]);
    assert!(info.is_none() || info.unwrap().frames >= 1);
}

#[test]
fn gif_first_frame_flattens_animation() {
    let gif = animated_gif(&[[255, 0, 0, 255], [0, 0, 255, 255]], 100);

    let url = gif_first_frame_to_data_url(&gif).unwrap();

    let (mime, bytes) = decode_data_url(&url);
    assert_eq!(mime, "image/jpeg", "opaque frame -> jpeg");
    let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
    let p = img.get_pixel(32, 24).0;
    assert!(
        p[0] > 200 && p[1] < 80 && p[2] < 80,
        "first frame is red, not later blue: {p:?}"
    );
}

#[test]
fn gif_first_frame_downscales_to_model_resolution() {
    let mut buf = Vec::new();
    let mut enc = image::codecs::gif::GifEncoder::new(&mut buf);
    let frames: Vec<image::Frame> = (0..2)
        .map(|_| {
            image::Frame::new(image::ImageBuffer::from_pixel(
                3000,
                2400,
                image::Rgba([200, 10, 20, 255]),
            ))
        })
        .collect();
    enc.encode_frames(frames).unwrap();
    drop(enc);

    let url = gif_first_frame_to_data_url(&buf).unwrap();

    let (mime, bytes) = decode_data_url(&url);
    assert_eq!(mime, "image/jpeg");
    let (w, h) = read_dimensions(&bytes).unwrap();
    assert!(within_model_resolution(w, h), "{w}x{h}");
}

#[test]
fn normalize_data_url_flattens_animated_gif() {
    let gif = animated_gif(&[[255, 0, 0, 255], [0, 0, 255, 255]], 100);
    let url = format!("data:image/gif;base64,{}", encode_base64(&gif));

    let normalized = normalize_data_url(&url).expect("animated gif is normalized");

    let (mime, bytes) = decode_data_url(&normalized);
    assert_eq!(mime, "image/jpeg", "flattened to a static jpeg");
    let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
    let p = img.get_pixel(32, 24).0;
    assert!(
        p[0] > 200 && p[2] < 80,
        "frame 1 (red), not later blue: {p:?}"
    );
}

#[test]
fn needs_compression_flags_animated_gif() {
    let anim = animated_gif(&[[255, 0, 0, 255], [0, 0, 255, 255]], 100);
    assert!(needs_compression(&anim), "multi-frame needs flattening");
    let still = animated_gif(&[[255, 0, 0, 255]], 100);
    assert!(
        !needs_compression(&still),
        "single-frame gif passes through"
    );
}

#[test]
fn probe_gif_info_handles_extensions_and_local_color_tables() {
    // Hand-built block layout — the walker never decodes LZW, so image
    // data bytes are placeholders. Covers: NETSCAPE app ext, comment ext,
    // GCE delays, LCT skip, trailer.
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&[2, 0, 2, 0, 0, 0, 0]); // LSD 2x2, no GCT
                                                   // NETSCAPE application extension (looping)
    gif.extend_from_slice(&[0x21, 0xFF, 0x0B]);
    gif.extend_from_slice(b"NETSCAPE2.0");
    gif.extend_from_slice(&[0x03, 0x01, 0x00, 0x00, 0x00]);
    // Comment extension
    gif.extend_from_slice(&[0x21, 0xFE, 0x05]);
    gif.extend_from_slice(b"hello");
    gif.push(0x00);
    // GCE (delay 100cs) + image descriptor with a 16-entry LCT
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x64, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 2, 0, 0x83]);
    gif.extend_from_slice(&[0u8; 48]); // local color table
    gif.extend_from_slice(&[0x02, 0x01, 0xAA, 0x00]); // LZW min + data + terminator
                                                      // GCE (delay 50cs) + plain image descriptor
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x32, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 2, 0, 0x00]);
    gif.extend_from_slice(&[0x02, 0x01, 0xBB, 0x00]);
    gif.push(0x3B); // trailer

    let info = probe_gif_info(&gif).unwrap();

    assert_eq!((info.width, info.height), (2, 2));
    assert_eq!(info.frames, 2);
    assert_eq!(info.duration_ms, 1500);
}

#[test]
fn gif_first_frame_rejects_decode_bomb_canvas() {
    // 60000x60000 canvas would need ~14GB RGBA — refused from the header.
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&[0x60, 0xEA, 0x60, 0xEA, 0, 0, 0]); // 60000x60000
    gif.extend_from_slice(&[0u8; 16]);

    let err = gif_first_frame_to_data_url(&gif).unwrap_err();

    assert!(err.to_string().contains("canvas too large"), "{err}");
}

#[test]
fn structure_corrupt_gif_flattens_instead_of_passing_through() {
    // Valid header + LSD, then garbage: the walk finds no frames.
    let mut gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00".to_vec();
    gif.extend_from_slice(b"garbage-not-blocks");
    assert!(probe_gif_info(&gif).is_none());

    // Not trusted to be small and static — flattening is attempted…
    assert!(needs_compression(&gif), "corrupt gif is not trusted");
    // …decode fails here, so normalize gives up and the caller keeps the
    // original (documented escape hatch, same as recompress exhaustion).
    let url = format!("data:image/gif;base64,{}", encode_base64(&gif));
    assert_eq!(normalize_data_url(&url), None);
}

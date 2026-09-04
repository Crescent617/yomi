//! Image utilities for reading and converting images to data URLs

use std::path::Path;

/// Maximum image file size (10MB)
pub const MAX_IMAGE_SIZE: u64 = 10 * 1024 * 1024;

/// Max image bytes embedded in a data URL for model input: Anthropic's
/// 5MB per-image cap is the tightest among providers (and base64 adds
/// ~33% on top). Larger images are recompressed by [`bytes_to_data_url`].
pub const MAX_EMBED_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Long-edge cap applied when recompression kicks in: Anthropic's
/// documented threshold — the API downsamples anything larger anyway, so
/// excess pixels only cost tokens, never fidelity.
const EMBED_MAX_DIMENSION: u32 = 1568;

/// Megapixel cap mirroring Anthropic's other downsample trigger
/// (~1.15MP). Together with the long-edge cap, API-side resampling never
/// happens: what we send is exactly what the model sees.
const EMBED_MAX_PIXELS: u32 = 1_150_000;

/// Supported image MIME types
pub const SUPPORTED_IMAGE_TYPES: &[(&str, &[u8])] = &[
    ("image/png", b"\x89PNG\r\n\x1a\n"),
    ("image/jpeg", b"\xff\xd8\xff"),
    ("image/gif", b"GIF87a"),
    ("image/gif", b"GIF89a"),
    ("image/webp", b"RIFF"), // WebP starts with RIFF, has WEBP at offset 8
];

/// Check if a file extension indicates an image file
pub fn is_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif"
            )
        })
}

/// Detect MIME type from file magic bytes
pub fn detect_mime_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

/// Check if a file is likely an image by reading its magic bytes
pub async fn is_image_file(path: &Path) -> bool {
    // First check extension as a quick filter
    if !is_image_extension(path) {
        return false;
    }

    // Read first 12 bytes to check magic bytes
    match tokio::fs::read(path).await {
        Ok(data) if data.len() >= 12 => detect_mime_type(&data).is_some(),
        _ => false,
    }
}

/// Read an image file and convert it to a base64 data URL
/// Returns `Ok(Some(data_url))` for valid images, `Ok(None)` for non-images
pub async fn image_to_data_url(path: &Path) -> crate::types::Result<Option<String>> {
    use crate::types::KernelError;
    // Check file size
    let metadata = tokio::fs::metadata(path).await.map_err(KernelError::from)?;
    if metadata.len() > MAX_IMAGE_SIZE {
        return Err(KernelError::io(format!(
            "Image file too large: {} bytes (max: {})",
            metadata.len(),
            MAX_IMAGE_SIZE
        )));
    }

    // Read file
    let data = tokio::fs::read(path).await?;

    // Not a recognized image
    if detect_mime_type(&data).is_none() {
        return Ok(None);
    }

    let data_url = bytes_to_data_url(&data)?;

    tracing::debug!(
        "Converted image {:?} to data URL ({} bytes -> {} chars)",
        path,
        data.len(),
        data_url.len()
    );

    Ok(Some(data_url))
}

/// Normalize raw image bytes for model input, mirroring Claude's own
/// normalization. Three triggers, all from Anthropic's documented
/// thresholds:
///
/// - long edge > [`EMBED_MAX_DIMENSION`] or pixels > [`EMBED_MAX_PIXELS`]
///   → downscale (regardless of byte size — the API downsamples those
///   anyway, so oversized pixels only inflate token count);
/// - bytes > [`MAX_EMBED_IMAGE_BYTES`] → additionally re-encode as JPEG
///   with a quality ladder (the API would reject these outright).
///
/// Returns `Ok(None)` when the image already fits every threshold, so
/// callers can pass the original through without any re-encode.
fn normalize_bytes(data: &[u8]) -> crate::types::Result<Option<String>> {
    use crate::types::KernelError;
    let Some(mime) = detect_mime_type(data) else {
        return Err(KernelError::io("unrecognized image data"));
    };
    // Animated or structure-unreadable GIFs flatten to frame 1 — later
    // frames never reach the model anyway (see
    // [`gif_first_frame_to_data_url`]), so inlining the whole animation
    // only inflates the request body.
    if gif_needs_flattening(data) {
        return gif_first_frame_to_data_url(data).map(Some);
    }
    let within_bytes = data.len() <= MAX_EMBED_IMAGE_BYTES;
    // Header-only dimension read — no pixel decode for compliant images.
    let within_res = read_dimensions(data).is_some_and(|(w, h)| within_model_resolution(w, h));
    if within_bytes && within_res {
        return Ok(None);
    }

    let img = image::load_from_memory(data)
        .map_err(|e| KernelError::io(format!("image decode failed: {e}")))?;
    let mut img = shrink_to_model_resolution(img);
    tracing::info!(
        bytes = data.len(),
        resolution_only = within_bytes,
        "recompressing image for model input"
    );
    if within_bytes {
        // Resolution-only overage: re-encode in kind — lossless PNG for
        // screenshots, JPEG q85 for photos.
        return Ok(Some(if mime == "image/png" {
            data_url("image/png", &encode_png(&img)?)
        } else {
            data_url("image/jpeg", &encode_jpeg(&img, 85)?)
        }));
    }

    for quality in [85u8, 70, 50] {
        let buf = encode_jpeg(&img, quality)?;
        if buf.len() <= MAX_EMBED_IMAGE_BYTES {
            return Ok(Some(data_url("image/jpeg", &buf)));
        }
        // Still too big — shrink 25% and retry at the next quality.
        img = img.resize(
            (img.width() * 3 / 4).max(1),
            (img.height() * 3 / 4).max(1),
            image::imageops::FilterType::Triangle,
        );
    }
    Err(KernelError::io(format!(
        "image too large even after recompression ({} bytes)",
        data.len()
    )))
}

/// Encode raw image bytes as a base64 data URL for model input,
/// normalized like Claude does (see [`normalize_bytes`]); images already
/// within every threshold pass through byte-identical.
pub fn bytes_to_data_url(data: &[u8]) -> crate::types::Result<String> {
    use crate::types::KernelError;
    let Some(mime) = detect_mime_type(data) else {
        return Err(KernelError::io("unrecognized image data"));
    };
    match normalize_bytes(data)? {
        Some(url) => Ok(url),
        None => Ok(data_url(mime, data)),
    }
}

/// Async sibling of [`bytes_to_data_url`] for async callers: the
/// recompression it may trigger (decode/resize/encode, potentially
/// hundreds of ms) runs on the blocking pool instead of stalling the
/// async worker.
pub async fn bytes_to_data_url_async(
    data: impl AsRef<[u8]> + Send + 'static,
) -> crate::types::Result<String> {
    match tokio::task::spawn_blocking(move || bytes_to_data_url(data.as_ref())).await {
        Ok(res) => res,
        Err(e) => Err(crate::types::KernelError::io(format!(
            "image processing failed: {e}"
        ))),
    }
}

/// Whether the image would be recompressed (bytes or resolution beyond
/// the model thresholds).
pub(crate) fn needs_compression(data: &[u8]) -> bool {
    gif_needs_flattening(data)
        || data.len() > MAX_EMBED_IMAGE_BYTES
        || read_dimensions(data).is_none_or(|(w, h)| !within_model_resolution(w, h))
}

/// GIF that must be flattened rather than passed through: provably
/// multi-frame, or structure unreadable (a corrupt walk means its true
/// size/shape is unknown — don't trust it). Single-frame GIFs still
/// pass through byte-identical.
fn gif_needs_flattening(data: &[u8]) -> bool {
    detect_mime_type(data) == Some("image/gif")
        && probe_gif_info(data).is_none_or(|info| info.frames > 1)
}

/// Read image dimensions from the container header — no pixel decode.
fn read_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Whether the dimensions fit Claude's resolution thresholds — beyond
/// them the API downsamples server-side (extra tokens, no extra detail).
fn within_model_resolution(w: u32, h: u32) -> bool {
    w.max(h) <= EMBED_MAX_DIMENSION && u64::from(w) * u64::from(h) <= u64::from(EMBED_MAX_PIXELS)
}

/// Animation info probed from GIF container bytes. Walking the block
/// structure needs no pixel decode, so probing even a long animation is
/// cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GifInfo {
    pub width: u32,
    pub height: u32,
    /// Image descriptors found (≈ frames).
    pub frames: u32,
    /// Sum of Graphic Control Extension delays.
    pub duration_ms: u64,
}

/// Probe GIF structure (dimensions, frame count, total delay) without
/// decoding pixels. Returns `None` for non-GIF data or too-short headers;
/// truncated block data just stops the count early.
pub fn probe_gif_info(data: &[u8]) -> Option<GifInfo> {
    if detect_mime_type(data) != Some("image/gif") || data.len() < 13 {
        return None;
    }
    // Logical Screen Descriptor.
    let width = u32::from(u16::from_le_bytes([data[6], data[7]]));
    let height = u32::from(u16::from_le_bytes([data[8], data[9]]));
    let mut pos = 13 + color_table_len(data[10]);
    let mut frames = 0u32;
    let mut duration_ms = 0u64;
    while let Some(&marker) = data.get(pos) {
        match marker {
            0x21 => {
                // Extension: label byte, then sub-blocks. The Graphic
                // Control Extension (0xF9) carries the frame delay in a
                // fixed 4-byte payload: size, packed, delay (LE), index.
                if data.get(pos + 1) == Some(&0xF9) && data.get(pos + 2) == Some(&4) {
                    if let (Some(&lo), Some(&hi)) = (data.get(pos + 4), data.get(pos + 5)) {
                        duration_ms += u64::from(u16::from_le_bytes([lo, hi])) * 10;
                    }
                }
                pos = skip_sub_blocks(data, pos + 2);
            }
            0x2C => {
                // Image descriptor: 9 bytes, optional local color table,
                // LZW minimum code size byte, then data sub-blocks.
                let Some(&packed) = data.get(pos + 9) else {
                    break;
                };
                pos = skip_sub_blocks(data, pos + 10 + color_table_len(packed) + 1);
                frames = frames.saturating_add(1);
            }
            // 0x3B trailer or a corrupt marker: stop, keep the count so far.
            _ => break,
        }
    }
    (frames > 0).then_some(GifInfo {
        width,
        height,
        frames,
        duration_ms,
    })
}

/// Color table length from a packed byte (bit 7 = present, bits 0-2 =
/// log2(entry count) − 1, 3 bytes per entry).
const fn color_table_len(packed: u8) -> usize {
    if packed & 0x80 == 0 {
        0
    } else {
        3 * (1usize << ((packed & 0x07) + 1))
    }
}

/// Skip a run of data sub-blocks; returns the position after the 0-length
/// terminator (or past the end for truncated data).
fn skip_sub_blocks(data: &[u8], mut pos: usize) -> usize {
    while let Some(&len) = data.get(pos) {
        pos += 1 + usize::from(len);
        if len == 0 {
            break;
        }
    }
    pos
}

/// Canvas pixel cap for first-frame decode. The logical screen size is
/// read from the GIF header without decoding, so decode-bomb canvases
/// (u16 dims — up to 65535², ~17GB RGBA) are refused before allocating;
/// the image crate's own 512MB alloc limit stays as backstop.
const GIF_MAX_CANVAS_PIXELS: u64 = 32_000_000;

/// Logical screen size from the GIF header — no block walk, no decode.
fn gif_canvas_size(data: &[u8]) -> Option<(u32, u32)> {
    if detect_mime_type(data) != Some("image/gif") || data.len() < 10 {
        return None;
    }
    Some((
        u32::from(u16::from_le_bytes([data[6], data[7]])),
        u32::from(u16::from_le_bytes([data[8], data[9]])),
    ))
}

/// Flatten GIF bytes to the first frame, normalized for model input.
/// Vision APIs only use the first frame anyway (Anthropic: "Animations
/// are unsupported, and only the first frame is used"), so this loses
/// nothing model-side and avoids inlining multi-MB animations.
pub fn gif_first_frame_to_data_url(data: &[u8]) -> crate::types::Result<String> {
    use crate::types::KernelError;
    if let Some((w, h)) = gif_canvas_size(data) {
        if u64::from(w) * u64::from(h) > GIF_MAX_CANVAS_PIXELS {
            return Err(KernelError::io(format!(
                "gif canvas too large to decode: {w}x{h}"
            )));
        }
    }
    let img = image::load_from_memory(data)
        .map_err(|e| KernelError::io(format!("gif first-frame decode failed: {e}")))?;
    // Transparency check comes before shrinking — interpolation blends alpha.
    let has_alpha = img.to_rgba8().pixels().any(|p| p.0[3] < 255);
    let img = shrink_to_model_resolution(img);
    Ok(if has_alpha {
        data_url("image/png", &encode_png(&img)?)
    } else {
        data_url("image/jpeg", &encode_jpeg(&img, 85)?)
    })
}

fn encode_png(img: &image::DynamicImage) -> crate::types::Result<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| crate::types::KernelError::io(format!("png encode failed: {e}")))?;
    Ok(buf)
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", encode_base64(bytes))
}

/// Scale `img` down to model resolution (aspect preserved): long edge ≤
/// [`EMBED_MAX_DIMENSION`] and total pixels ≤ [`EMBED_MAX_PIXELS`].
/// Anthropic downsamples beyond both thresholds, so anything larger
/// inflates token count without adding model-visible detail.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
// scale ≤ 1 and dims ≤ EMBED_MAX_DIMENSION, so the f64→u32 casts can't
// truncate anything real.
fn shrink_to_model_resolution(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    let mut scale = (f64::from(EMBED_MAX_DIMENSION) / w.max(h)).min(1.0);
    if w * h > f64::from(EMBED_MAX_PIXELS) {
        scale = scale.min((f64::from(EMBED_MAX_PIXELS) / (w * h)).sqrt());
    }
    if scale >= 1.0 {
        return img;
    }
    img.resize(
        ((w * scale) as u32).max(1),
        ((h * scale) as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

fn encode_jpeg(img: &image::DynamicImage, quality: u8) -> crate::types::Result<Vec<u8>> {
    use image::ImageEncoder as _;
    let rgb = img.to_rgb8();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .write_image(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| crate::types::KernelError::io(format!("jpeg encode failed: {e}")))?;
    Ok(buf)
}

fn encode_base64(data: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data)
}

/// Decode a `data:<mime>;base64,<payload>` URL into raw bytes. Returns
/// `None` for other URL kinds and unparseable payloads.
fn decode_data_url_bytes(url: &str) -> Option<Vec<u8>> {
    let b64 = url.strip_prefix("data:")?.split_once(";base64,")?.1;
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()
}

/// Normalize an inline `data:` image URL for model input (same rules as
/// [`bytes_to_data_url`]). Returns `None` for other URL kinds
/// (`http(s)://`, `asset://`), already-compliant images, and unparseable
/// payloads — callers keep the original in all those cases.
pub fn normalize_data_url(url: &str) -> Option<String> {
    normalize_bytes(&decode_data_url_bytes(url)?).ok()?
}

/// Async sibling of [`normalize_data_url`]: the common case (already
/// compliant) costs only a base64 decode + header parse; actual
/// recompression runs on the blocking pool (see [`bytes_to_data_url_async`]).
pub async fn normalize_data_url_async(url: &str) -> Option<String> {
    let bytes = decode_data_url_bytes(url)?;
    if !needs_compression(&bytes) {
        return None;
    }
    tokio::task::spawn_blocking(move || normalize_bytes(&bytes).ok().flatten())
        .await
        .ok()
        .flatten()
}

/// Normalize every inline `data:` image URL in a content block list, in
/// place. Single choke point for user-provided images (TUI paste, GUI
/// attachments, channels); already-compliant images cost only a base64
/// decode and a header parse, recompression runs on the blocking pool.
pub async fn normalize_image_blocks(blocks: &mut [crate::types::ContentBlock]) {
    for block in blocks {
        let crate::types::ContentBlock::ImageUrl { image_url } = block else {
            continue;
        };
        if let Some(normalized) = normalize_data_url_async(&image_url.url).await {
            image_url.url = normalized;
        }
    }
}

#[cfg(test)]
#[path = "image_test.rs"]
mod tests;

/// Shared fixtures for image-pipeline tests (image/asset/executor).
#[cfg(test)]
pub(crate) mod test_utils {

    /// Build an incompressible (pseudo-random) PNG of the given size.
    pub fn noisy_png(width: u32, height: u32) -> Vec<u8> {
        let mut state = 0x243F_6A88_85A3_08D3u64;
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..width * height * 3 {
            // SplitMix64 — deterministic noise that PNG filters can't compress.
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            rgb.push((state >> 32) as u8);
        }
        let mut out = Vec::new();
        image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(width, height, rgb)
            .unwrap()
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// Build a solid-color image — tiny in bytes but scalable past the
    /// resolution thresholds (unlike noise, which exceeds the byte cap).
    pub fn solid_image(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            width,
            height,
            image::Rgb([10u8, 20, 30]),
        ))
        .write_to(&mut std::io::Cursor::new(&mut out), format)
        .unwrap();
        out
    }

    pub fn to_data_url(mime: &str, bytes: &[u8]) -> String {
        format!(
            "data:{mime};base64,{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
        )
    }

    /// Split a data URL into (mime, decoded bytes).
    pub fn decode_data_url(url: &str) -> (String, Vec<u8>) {
        let (meta, payload) = url.split_once(',').expect("data url has comma");
        let mime = meta
            .strip_prefix("data:")
            .and_then(|m| m.strip_suffix(";base64"))
            .expect("data url mime");
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload)
            .expect("base64 decode");
        (mime.to_string(), bytes)
    }
}

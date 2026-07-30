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
    data.len() > MAX_EMBED_IMAGE_BYTES
        || read_dimensions(data).is_none_or(|(w, h)| !within_model_resolution(w, h))
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

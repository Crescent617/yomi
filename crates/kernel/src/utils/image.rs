//! Image utilities for reading and converting images to data URLs

use std::path::Path;

/// Maximum image file size (10MB)
pub const MAX_IMAGE_SIZE: u64 = 10 * 1024 * 1024;

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
pub async fn image_to_data_url(path: &Path) -> anyhow::Result<Option<String>> {
    // Check file size
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.len() > MAX_IMAGE_SIZE {
        anyhow::bail!(
            "Image file too large: {} bytes (max: {})",
            metadata.len(),
            MAX_IMAGE_SIZE
        );
    }

    // Read file
    let data = tokio::fs::read(path).await?;

    // Detect MIME type
    let mime_type = match detect_mime_type(&data) {
        Some(mime) => mime,
        None => return Ok(None), // Not a recognized image
    };

    // Encode to base64
    let base64_data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);

    // Remove any newlines
    let base64_clean: String = base64_data
        .chars()
        .filter(|&c| !c.is_whitespace())
        .collect();

    // Create data URL
    let data_url = format!("data:{mime_type};base64,{base64_clean}");

    tracing::debug!(
        "Converted image {:?} to {} data URL ({} bytes -> {} chars)",
        path,
        mime_type,
        data.len(),
        base64_clean.len()
    );

    Ok(Some(data_url))
}

#[cfg(test)]
mod tests {
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
}

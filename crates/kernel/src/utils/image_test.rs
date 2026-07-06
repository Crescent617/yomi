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

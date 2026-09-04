use super::*;

use crate::tools::DEFAULT_MAX_TOOL_OUTPUT_LENGTH;
use tempfile::TempDir;

#[tokio::test]
async fn test_read_basic() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create test file
    tokio::fs::write(base_path.join("test.txt"), "Hello, World!")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    assert!(result.text_content().contains("Hello, World!"));
}

#[tokio::test]
async fn test_read_with_offset() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "line1\nline2\nline3")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "offset": 2});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    assert!(!content.contains("line1"));
    assert!(content.contains("line2"));
    assert!(content.contains("line3"));
}

#[tokio::test]
async fn test_read_with_limit() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "line1\nline2\nline3")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "limit": 2});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    assert!(content.contains("line1"));
    assert!(content.contains("line2"));
    assert!(!content.contains("line3"));
}

#[tokio::test]
async fn test_read_with_offset_and_limit() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "a\nb\nc\nd\ne")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args =
        serde_json::json!({"path": "test.txt", "offset": 2, "limit": 2, "line_numbers": true});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    let lines: Vec<&str> = content.lines().collect();
    assert!(lines.iter().any(|l| l.contains('b')));
    assert!(lines.iter().any(|l| l.contains('c')));
    assert!(!lines
        .iter()
        .any(|l| l.trim() == "a" || l.trim().ends_with(" a")));
    assert!(!lines
        .iter()
        .any(|l| l.trim() == "d" || l.trim().ends_with(" d")));
}

#[tokio::test]
async fn test_read_with_line_numbers() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "line1\nline2")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "line_numbers": true});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    assert!(content.contains("1\tline1"));
    assert!(content.contains("2\tline2"));
}

#[tokio::test]
async fn test_read_offset_with_line_numbers() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "a\nb\nc")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "offset": 2, "line_numbers": true});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    // Line numbers should start from offset
    assert!(content.contains("2\tb"));
    assert!(content.contains("3\tc"));
    assert!(!content.contains("1\ta"));
}

#[tokio::test]
async fn test_read_file_not_found() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "nonexistent.txt"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.error_text().contains("does not exist"));
}

#[tokio::test]
async fn test_read_offset_out_of_range() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "line1\nline2")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "offset": 10});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.error_text().contains("out of range"));
}

#[tokio::test]
async fn test_read_without_line_numbers_stopped_hint() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "a\nb\nc\nd\ne")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "limit": 3, "line_numbers": false});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    // File content is a\nb\nc — check actual lines, not single chars
    // (prompt text contains letters like 'd' in "read")
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines[0], "a");
    assert_eq!(lines[1], "b");
    assert_eq!(lines[2], "c");
    // Should tell the model where it stopped when line_numbers is false
    assert!(content.contains("Stopped at line 3 of 5"));
    assert!(content.contains("Use offset/limit to read more"));
}

#[tokio::test]
async fn test_read_truncation() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create a large file that will trigger truncation
    // Each line is about 100 chars, create enough lines to exceed limit
    let line = "x".repeat(100);
    let lines_needed = DEFAULT_MAX_TOOL_OUTPUT_LENGTH / 100 + 10;
    let mut content = String::with_capacity(line.len() * lines_needed + lines_needed);
    for _ in 0..lines_needed {
        content.push_str(&line);
        content.push('\n');
    }
    tokio::fs::write(base_path.join("large.txt"), content)
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "large.txt"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let text = result.text_content();
    // Should contain truncation notice
    assert!(text.contains("Content truncated"));
    // Should indicate line number where truncated
    assert!(text.contains("at line"));
    // Length should be close to limit (allowing for truncation notice overhead)
    assert!(text.len() <= DEFAULT_MAX_TOOL_OUTPUT_LENGTH + 100);
}

/// Build an animated GIF from solid-color 64x48 frames (100ms each).
fn animated_gif_bytes(colors: &[[u8; 4]]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = image::codecs::gif::GifEncoder::new(&mut buf);
    let frames: Vec<image::Frame> = colors
        .iter()
        .map(|&rgba| {
            image::Frame::from_parts(
                image::ImageBuffer::from_pixel(64, 48, image::Rgba(rgba)),
                0,
                0,
                image::Delay::from_numer_denom_ms(100, 1),
            )
        })
        .collect();
    enc.encode_frames(frames).unwrap();
    drop(enc); // flush the trailer before reading buf
    buf
}

fn solid_image_bytes(format: image::ImageFormat) -> Vec<u8> {
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(
        16,
        16,
        image::Rgba([255, 0, 0, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut buf), format)
    .unwrap();
    buf
}

fn has_image_block(result: &ToolOutput) -> bool {
    result
        .contents
        .iter()
        .any(|b| matches!(b, crate::types::ToolOutputBlock::Image { .. }))
}

#[tokio::test]
async fn test_read_animated_gif_flattens_to_first_frame() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    let gif = animated_gif_bytes(&[[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]);
    tokio::fs::write(base_path.join("anim.gif"), &gif)
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "anim.gif"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    assert!(has_image_block(&result));
    let text = result.text_content();
    assert!(text.contains("Animated GIF"), "{text}");
    assert!(text.contains("64x48"), "{text}");
    assert!(text.contains("3 frames"), "{text}");
    assert!(text.contains("frame 1 shown"), "{text}");
    assert!(text.contains("magick"), "{text}");
    // First frame is inlined as a re-encoded still, never the raw animation.
    let url = result
        .contents
        .iter()
        .find_map(|b| match b {
            crate::types::ToolOutputBlock::Image { url, .. } => Some(url.as_str()),
            crate::types::ToolOutputBlock::Text { .. } => None,
        })
        .unwrap();
    assert!(
        url.starts_with("data:image/jpeg;base64,") || url.starts_with("data:image/png;base64,"),
        "{url}"
    );
    assert!(
        url.len() < 100_000,
        "flattened frame stays small: {} chars",
        url.len()
    );
}

#[tokio::test]
async fn test_read_static_gif_passes_through() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    tokio::fs::write(
        base_path.join("still.gif"),
        solid_image_bytes(image::ImageFormat::Gif),
    )
    .await
    .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "still.gif"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let text = result.text_content();
    assert!(text.contains("[Image:"), "{text}");
    assert!(!text.contains("Animated GIF"), "{text}");
    let url = result
        .contents
        .iter()
        .find_map(|b| match b {
            crate::types::ToolOutputBlock::Image { url, .. } => Some(url.as_str()),
            crate::types::ToolOutputBlock::Text { .. } => None,
        })
        .unwrap();
    assert!(url.starts_with("data:image/gif;base64,"), "{url}");
}

#[tokio::test]
async fn test_read_image_detected_by_magic_bytes() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    tokio::fs::write(
        base_path.join("mismatch.dat"),
        solid_image_bytes(image::ImageFormat::Png),
    )
    .await
    .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "mismatch.dat"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    assert!(has_image_block(&result));
    assert!(result.text_content().contains("[Image:"));
}

#[tokio::test]
async fn test_read_unsupported_image_data() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    tokio::fs::write(base_path.join("fake.png"), "definitely not an image")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "fake.png"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    let text = result.error_text();
    assert!(text.contains("Unsupported image"), "{text}");
    assert!(text.contains("fake.png"), "{text}");
}

#[tokio::test]
async fn test_read_binary_file_unsupported() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    tokio::fs::write(base_path.join("data.bin"), b"\x00\x01\x02\x03binary")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "data.bin"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    let text = result.error_text();
    assert!(text.contains("Unsupported binary file"), "{text}");
    assert!(text.contains("data.bin"), "{text}");
}

#[tokio::test]
async fn test_read_text_starting_with_gif_magic() {
    // Magic-byte sniffing is a heuristic: a text file that happens to
    // start with "GIF89a" must still read as text.
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    tokio::fs::write(
        base_path.join("notes.txt"),
        "GIF89a is the 1989 revision of the format.\nSecond line stays readable.",
    )
    .await
    .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "notes.txt"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    assert!(result.text_content().contains("Second line stays readable"));
}

#[tokio::test]
async fn test_read_corrupt_gif_strict_unsupported() {
    // Image extension = strict routing: no text fallback, explicit error.
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    let mut gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00".to_vec();
    gif.extend_from_slice(b"garbage-not-blocks");
    tokio::fs::write(base_path.join("broken.gif"), &gif)
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "broken.gif"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.error_text().contains("Unsupported image"));
}

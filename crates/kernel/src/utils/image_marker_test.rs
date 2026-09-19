//! `image_marker` 协议测试：行识别、路径解析、加载失败降级、上限与顺序。

use super::{collect_image_markers, IMAGE_MARKER_PREFIX, MAX_IMAGES_PER_RESULT};
use crate::types::ToolOutputBlock;
use crate::utils::image::test_utils::noisy_png;
use std::fmt::Write as _;

fn image_urls(out: &super::CollectedOutput) -> Vec<&str> {
    out.images
        .iter()
        .map(|b| match b {
            ToolOutputBlock::Image { url, .. } => url.as_str(),
            ToolOutputBlock::Text { .. } => unreachable!("images vec 只装图片 block"),
        })
        .collect()
}

/// Build an animated GIF from solid-color 64x48 frames (100ms each).
/// （`read_test.rs` 同名夹具的副本——测试文件各自独立，不跨模块共享。）
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
    drop(enc);
    buf
}

#[tokio::test]
async fn no_marker_passes_through() {
    let dir = tempfile::TempDir::new().unwrap();
    let out = collect_image_markers("line one\nline two\n", dir.path()).await;
    assert_eq!(out.text, "line one\nline two\n");
    assert!(out.images.is_empty());
}

#[tokio::test]
async fn absolute_path_marker_loads_image() {
    let dir = tempfile::TempDir::new().unwrap();
    let png = noisy_png(32, 24);
    let path = dir.path().join("shot.png");
    tokio::fs::write(&path, &png).await.unwrap();

    let input = format!("before\n{IMAGE_MARKER_PREFIX}{}\nafter\n", path.display());
    let out = collect_image_markers(&input, dir.path()).await;

    assert_eq!(image_urls(&out).len(), 1);
    assert!(
        image_urls(&out)[0].starts_with("data:image/png;base64,"),
        "{}",
        image_urls(&out)[0]
    );
    let note = format!("[Image: {} | Size: {} bytes]", path.display(), png.len());
    assert_eq!(out.text, format!("before\n{note}\nafter\n"));
}

#[tokio::test]
async fn relative_path_resolves_against_base_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    tokio::fs::create_dir(dir.path().join("out")).await.unwrap();
    tokio::fs::write(dir.path().join("out/chart.png"), noisy_png(16, 16))
        .await
        .unwrap();

    let out =
        collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}out/chart.png"), dir.path()).await;

    assert_eq!(image_urls(&out).len(), 1);
    assert!(
        out.text.contains(&format!(
            "[Image: {}",
            dir.path().join("out/chart.png").display()
        )),
        "{}",
        out.text
    );
}

#[tokio::test]
async fn path_with_spaces_and_unicode_is_verbatim() {
    let dir = tempfile::TempDir::new().unwrap();
    let name = "9 月 数据 对比.png";
    tokio::fs::write(dir.path().join(name), noisy_png(8, 8))
        .await
        .unwrap();

    let out = collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}{name}"), dir.path()).await;
    assert_eq!(image_urls(&out).len(), 1, "{}", out.text);
}

#[tokio::test]
async fn leading_whitespace_is_not_a_marker() {
    let dir = tempfile::TempDir::new().unwrap();
    let input = format!("  {IMAGE_MARKER_PREFIX}x.png");
    let out = collect_image_markers(&input, dir.path()).await;
    assert_eq!(out.text, input);
    assert!(out.images.is_empty());
}

#[tokio::test]
async fn crlf_line_ending_is_stripped() {
    let dir = tempfile::TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("a.png"), noisy_png(8, 8))
        .await
        .unwrap();

    let input = format!("{IMAGE_MARKER_PREFIX}a.png\r\nnext");
    let out = collect_image_markers(&input, dir.path()).await;
    assert_eq!(image_urls(&out).len(), 1, "{}", out.text);
    assert!(out.text.ends_with("\nnext"), "{}", out.text);
}

#[tokio::test]
async fn missing_file_degrades_to_unavailable_note() {
    let dir = tempfile::TempDir::new().unwrap();
    let out = collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}nope.png"), dir.path()).await;
    assert!(out.images.is_empty());
    assert_eq!(out.text, "[Image unavailable: nope.png | not found]");
}

#[tokio::test]
async fn non_image_file_degrades_to_unavailable_note() {
    let dir = tempfile::TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("data.txt"), "hello")
        .await
        .unwrap();
    let out = collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}data.txt"), dir.path()).await;
    assert!(out.images.is_empty());
    assert!(out.text.contains("not a supported image"), "{}", out.text);
}

#[tokio::test]
async fn empty_path_is_unavailable() {
    let dir = tempfile::TempDir::new().unwrap();
    let out = collect_image_markers(IMAGE_MARKER_PREFIX, dir.path()).await;
    assert!(out.images.is_empty());
    assert!(out.text.contains("empty path"), "{}", out.text);
}

#[tokio::test]
async fn excess_markers_are_omitted_with_note() {
    let dir = tempfile::TempDir::new().unwrap();
    for i in 0..=MAX_IMAGES_PER_RESULT {
        tokio::fs::write(dir.path().join(format!("{i}.png")), noisy_png(8, 8))
            .await
            .unwrap();
    }
    let mut input = String::new();
    for i in 0..=MAX_IMAGES_PER_RESULT {
        let _ = writeln!(input, "{IMAGE_MARKER_PREFIX}{i}.png");
    }
    let out = collect_image_markers(&input, dir.path()).await;

    assert_eq!(image_urls(&out).len(), MAX_IMAGES_PER_RESULT);
    assert!(
        out.text.contains(&format!(
            "[Image omitted: {MAX_IMAGES_PER_RESULT}.png | image limit {MAX_IMAGES_PER_RESULT} per tool result]"
        )),
        "{}",
        out.text
    );
}

#[tokio::test]
async fn images_keep_marker_order_and_notes_stay_in_place() {
    let dir = tempfile::TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("one.png"), noisy_png(8, 8))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("two.png"), noisy_png(8, 8))
        .await
        .unwrap();

    let input = format!(
        "log line\n{IMAGE_MARKER_PREFIX}one.png\nmiddle\n{IMAGE_MARKER_PREFIX}two.png\ndone"
    );
    let out = collect_image_markers(&input, dir.path()).await;

    assert_eq!(image_urls(&out).len(), 2);
    let one = out.text.find("one.png").unwrap();
    let two = out.text.find("two.png").unwrap();
    assert!(one < two);
    assert!(out.text.starts_with("log line\n[Image: "));
    assert!(out.text.contains("\nmiddle\n[Image: "));
    assert!(out.text.ends_with("\ndone"));
}

#[tokio::test]
async fn animated_gif_is_flattened_to_first_frame() {
    let dir = tempfile::TempDir::new().unwrap();
    let gif = animated_gif_bytes(&[[255, 0, 0, 255], [0, 255, 0, 255]]);
    tokio::fs::write(dir.path().join("anim.gif"), &gif)
        .await
        .unwrap();

    let out = collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}anim.gif"), dir.path()).await;

    let urls = image_urls(&out);
    assert_eq!(urls.len(), 1, "{}", out.text);
    assert!(
        urls[0].starts_with("data:image/jpeg;base64,")
            || urls[0].starts_with("data:image/png;base64,"),
        "{}",
        urls[0]
    );
    assert!(
        out.text.contains("animated GIF: frame 1 shown"),
        "{}",
        out.text
    );
}

#[tokio::test]
async fn static_gif_passes_through() {
    let dir = tempfile::TempDir::new().unwrap();
    let gif = animated_gif_bytes(&[[255, 0, 0, 255]]);
    tokio::fs::write(dir.path().join("still.gif"), &gif)
        .await
        .unwrap();

    let out = collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}still.gif"), dir.path()).await;

    let urls = image_urls(&out);
    assert_eq!(urls.len(), 1, "{}", out.text);
    assert!(urls[0].starts_with("data:image/gif;base64,"), "{}", urls[0]);
    assert!(!out.text.contains("frame 1"), "{}", out.text);
}

#[tokio::test]
async fn truncated_png_degrades_to_unavailable() {
    // 头合法（IHDR 完好）但像素缺失：不得透传进模型请求（provider 会
    // 400 整轮），必须降级为 unavailable 说明。
    let dir = tempfile::TempDir::new().unwrap();
    let png = noisy_png(64, 64);
    // 保留 40 字节：PNG 签名 8 + 完整 IHDR 25，IDAT 未写出。
    tokio::fs::write(dir.path().join("half.png"), &png[..40])
        .await
        .unwrap();

    let out = collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}half.png"), dir.path()).await;
    assert!(out.images.is_empty(), "{}", out.text);
    assert!(
        out.text.contains("[Image unavailable: half.png"),
        "{}",
        out.text
    );
}

#[tokio::test]
async fn oversized_file_is_rejected_before_read() {
    let dir = tempfile::TempDir::new().unwrap();
    let f = tokio::fs::File::create(dir.path().join("big.png"))
        .await
        .unwrap();
    f.set_len(crate::utils::image::MAX_IMAGE_SIZE + 1)
        .await
        .unwrap();
    drop(f);

    let out = collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}big.png"), dir.path()).await;
    assert!(out.images.is_empty());
    assert!(out.text.contains("too large"), "{}", out.text);
}

#[tokio::test]
async fn corrupt_gif_degrades_with_decode_reason() {
    let dir = tempfile::TempDir::new().unwrap();
    // 头完好 + 垃圾块：结构走查找不到帧 → 拍平 → 解码失败。
    let mut gif = b"GIF89a\x40\x00\x30\x00\x00\x00\x00".to_vec();
    gif.extend_from_slice(b"garbage-not-blocks");
    tokio::fs::write(dir.path().join("broken.gif"), &gif)
        .await
        .unwrap();

    let out = collect_image_markers(&format!("{IMAGE_MARKER_PREFIX}broken.gif"), dir.path()).await;
    assert!(out.images.is_empty(), "{}", out.text);
    assert!(
        out.text.contains("[Image unavailable: broken.gif"),
        "{}",
        out.text
    );
}

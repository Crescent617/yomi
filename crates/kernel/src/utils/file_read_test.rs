use super::*;
use base64::Engine as _;

fn decode(chunk: &FileBytes) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(&chunk.data_base64)
        .unwrap()
}

fn attachment_source(dir: &Path, path: &str) -> FileSource {
    FileSource::Attachment {
        base_dir: Some(dir.to_string_lossy().to_string()),
        path: path.to_string(),
    }
}

// ── read_file: attachments ───────────────────────────────────────────

#[tokio::test]
async fn reads_whole_file_in_one_chunk() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), b"\x89PNGdata").unwrap();

    let chunk = read_file(
        &attachment_source(dir.path(), "a.png"),
        Path::new("/unused"),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(decode(&chunk), b"\x89PNGdata");
    assert_eq!(chunk.mime, "image/png");
    assert_eq!(chunk.file_size, 8);
    assert_eq!(chunk.start_offset, 0);
    assert_eq!(chunk.end_offset, 8);
    assert!(chunk.mtime_ms > 0);
}

#[tokio::test]
async fn chunks_follow_offset_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..100u8).collect();
    std::fs::write(dir.path().join("data.bin"), &data).unwrap();
    let source = attachment_source(dir.path(), "data.bin");

    let first = read_file(&source, Path::new("/u"), None, Some(40))
        .await
        .unwrap();
    assert_eq!(decode(&first), &data[..40]);
    assert_eq!(first.start_offset, 0);
    assert_eq!(first.end_offset, 40);
    assert_eq!(first.file_size, 100);

    let rest = read_file(&source, Path::new("/u"), Some(first.end_offset), None)
        .await
        .unwrap();
    assert_eq!(decode(&rest), &data[40..]);
    assert_eq!(rest.end_offset, 100);
}

#[tokio::test]
async fn zero_limit_is_a_stat_call() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("report.pdf"), b"%PDF-1.7").unwrap();

    let chunk = read_file(
        &attachment_source(dir.path(), "report.pdf"),
        Path::new("/u"),
        None,
        Some(0),
    )
    .await
    .unwrap();

    assert!(chunk.data_base64.is_empty());
    assert_eq!(chunk.mime, "application/pdf");
    assert_eq!(chunk.file_size, 8);
    assert_eq!(chunk.start_offset, 0);
    assert_eq!(chunk.end_offset, 0);
}

#[tokio::test]
async fn offset_past_end_yields_empty_chunk() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hi").unwrap();

    let chunk = read_file(
        &attachment_source(dir.path(), "a.txt"),
        Path::new("/u"),
        Some(999),
        None,
    )
    .await
    .unwrap();

    assert!(chunk.data_base64.is_empty());
    assert_eq!(chunk.start_offset, 2);
    assert_eq!(chunk.end_offset, 2);
}

#[tokio::test]
async fn limit_is_capped_at_max_chunk_bytes() {
    // Just verify the cap math, not a multi-MiB transfer: a file smaller
    // than the cap returns at most file_size bytes either way.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hi").unwrap();

    let chunk = read_file(
        &attachment_source(dir.path(), "a.txt"),
        Path::new("/u"),
        None,
        Some(u64::MAX),
    )
    .await
    .unwrap();
    assert_eq!(chunk.end_offset, 2);
}

#[tokio::test]
async fn rejects_traversal_missing_and_directory() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = Path::new("/u");

    assert!(
        read_file(&attachment_source(dir.path(), "../x"), data_dir, None, None)
            .await
            .is_err()
    );
    assert!(read_file(
        &attachment_source(dir.path(), "missing.txt"),
        data_dir,
        None,
        None
    )
    .await
    .is_err());
    assert!(
        read_file(&attachment_source(dir.path(), ""), data_dir, None, None)
            .await
            .is_err()
    );
    // Relative path without a base dir cannot resolve.
    assert!(read_file(
        &FileSource::Attachment {
            base_dir: None,
            path: "a.txt".to_string(),
        },
        data_dir,
        None,
        None,
    )
    .await
    .is_err());
}

// ── read_file: assets ────────────────────────────────────────────────

#[tokio::test]
async fn reads_asset_from_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("deadbeef.png"), b"\x89PNG").unwrap();

    let chunk = read_file(
        &FileSource::Asset {
            url: "asset://deadbeef.png".to_string(),
        },
        dir.path(),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(decode(&chunk), b"\x89PNG");
    assert_eq!(chunk.mime, "image/png");
}

#[tokio::test]
async fn rejects_asset_traversal_and_unknown_scheme() {
    let dir = tempfile::tempdir().unwrap();
    // A decoy outside the assets dir that traversal would otherwise reach.
    std::fs::write(dir.path().join("secret.txt"), b"nope").unwrap();

    for url in [
        "asset://../secret.txt",
        "asset://a/b.png",
        "asset://..",
        "file:///etc/passwd",
        "asset://missing.png",
    ] {
        let result = read_file(
            &FileSource::Asset {
                url: url.to_string(),
            },
            dir.path(),
            None,
            None,
        )
        .await;
        assert!(result.is_err(), "{url} must not resolve");
    }
}

// ── wire encoding ────────────────────────────────────────────────────

#[test]
fn file_source_serializes_snake_case() {
    let value = serde_json::to_value(FileSource::Attachment {
        base_dir: Some("/w".to_string()),
        path: "a.png".to_string(),
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({"attachment": {"base_dir": "/w", "path": "a.png"}})
    );

    let value = serde_json::to_value(FileSource::Asset {
        url: "asset://x.png".to_string(),
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({"asset": {"url": "asset://x.png"}})
    );
}

#[tokio::test]
async fn read_file_attachment_falls_back_to_default_workspace_when_base_dir_missing() {
    // DB 无 working_dir 的会话（未绑项目）：base_dir 缺省按 <data_dir>/workspace 解析
    let data = tempfile::tempdir().unwrap();
    let ws = data.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("note.txt"), b"hello").unwrap();

    let source = FileSource::Attachment {
        base_dir: None,
        path: "note.txt".to_string(),
    };
    let bytes = read_file(&source, data.path(), None, Some(0))
        .await
        .unwrap();

    assert_eq!(bytes.file_size, 5);
}

#[tokio::test]
async fn read_file_attachment_absolute_path_is_taken_as_is() {
    // 绝对路径不受 base_dir/回落影响（防"越界守卫误杀绝对路径"回归——data_dir 故意给不存在）
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"abs").unwrap();

    let source = FileSource::Attachment {
        base_dir: None,
        path: file.path().to_string_lossy().to_string(),
    };
    let bytes = read_file(
        &source,
        std::path::Path::new("/nonexistent-data-dir"),
        None,
        Some(0),
    )
    .await
    .unwrap();

    assert_eq!(bytes.file_size, 3);
}

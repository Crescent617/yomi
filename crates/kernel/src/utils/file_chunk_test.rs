use super::*;
use std::io::Write;

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("create temp dir")
}

#[test]
fn reads_tail_pages_back_and_reads_delta() {
    let dir = temp_dir();
    let path = dir.path().join("debug.log");
    let max_bytes = 16;
    std::fs::write(&path, "0123456789abcdef-tail").unwrap();

    let tail = read_utf8_file_chunk(&path, None, None, max_bytes, false).unwrap();
    assert!(tail.content.ends_with("-tail"));
    assert!(tail.has_earlier);

    let earlier =
        read_utf8_file_chunk(&path, Some(tail.start_offset), None, max_bytes, false).unwrap();
    assert_eq!(earlier.start_offset, 0);

    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"more")
        .unwrap();
    let delta = read_utf8_file_chunk(&path, None, Some(tail.end_offset), max_bytes, false).unwrap();
    assert_eq!(delta.content, "more");

    std::fs::write(&path, "rotated").unwrap();
    let rotated =
        read_utf8_file_chunk(&path, None, Some(delta.end_offset), max_bytes, false).unwrap();
    assert_eq!(rotated.content, "rotated");
}

#[test]
fn trims_partial_utf8_boundaries() {
    let dir = temp_dir();
    let path = dir.path().join("debug.log");
    std::fs::write(&path, [b"ok".as_slice(), &[0xE4, 0xBD]].concat()).unwrap();
    let partial = read_utf8_file_chunk(&path, None, None, 16, false).unwrap();
    assert_eq!(partial.content, "ok");
    assert_eq!(partial.end_offset, 2);

    std::fs::write(&path, "你tail").unwrap();
    let boundary = read_utf8_file_chunk(&path, None, None, 5, false).unwrap();
    assert_eq!(boundary.content, "tail");
}

#[test]
fn missing_file_can_be_empty() {
    let dir = temp_dir();
    let path = dir.path().join("missing.log");
    assert_eq!(
        read_utf8_file_chunk(&path, None, None, 16, true)
            .unwrap()
            .content,
        ""
    );
    assert!(read_utf8_file_chunk(&path, None, None, 16, false).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinks() {
    let dir = temp_dir();
    let target = dir.path().join("target.log");
    let link = dir.path().join("debug.log");
    std::fs::write(&target, "secret").unwrap();
    std::os::unix::fs::symlink(target, &link).unwrap();
    assert!(read_utf8_file_chunk(&link, None, None, 16, false).is_err());
}

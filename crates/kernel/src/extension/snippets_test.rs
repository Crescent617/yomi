//! snippet 扫描测试：排序、破损源跳过、截断。

use super::load_snippets;

fn ext_with_snippets(data: &tempfile::TempDir, ext: &str, snippets: &[(&str, &str)]) {
    let dir = data.path().join("extensions").join(ext).join("snippets");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, content) in snippets {
        std::fs::write(dir.join(name), content).unwrap();
    }
}

#[tokio::test]
async fn empty_dir_is_zero_cost() {
    let data = tempfile::tempdir().unwrap();
    assert!(load_snippets(data.path()).await.is_empty());
}

#[tokio::test]
async fn sorted_by_ext_then_file() {
    let data = tempfile::tempdir().unwrap();
    ext_with_snippets(&data, "zeta", &[("a.md", "zeta-a")]);
    ext_with_snippets(&data, "alpha", &[("b.md", "alpha-b"), ("a.md", "alpha-a")]);

    let got = load_snippets(data.path()).await;
    let keys: Vec<(String, String)> = got
        .iter()
        .map(|s| (s.ext.clone(), s.file.clone()))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("alpha".to_string(), "a.md".to_string()),
            ("alpha".to_string(), "b.md".to_string()),
            ("zeta".to_string(), "a.md".to_string()),
        ]
    );
    assert_eq!(got[0].content, "alpha-a");
}

#[tokio::test]
async fn broken_source_ext_skipped() {
    let data = tempfile::tempdir().unwrap();
    ext_with_snippets(&data, "good", &[("a.md", "ok")]);
    // 破损 symlink：extensions/bad 指向不存在的源。
    std::fs::create_dir_all(data.path().join("extensions")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/nonexistent/bad", data.path().join("extensions/bad")).unwrap();

    let got = load_snippets(data.path()).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].ext, "good");
}

#[tokio::test]
async fn oversize_snippet_truncated() {
    let data = tempfile::tempdir().unwrap();
    let big = "x".repeat(20 * 1024);
    ext_with_snippets(&data, "big", &[("a.md", &big)]);

    let got = load_snippets(data.path()).await;
    assert_eq!(got.len(), 1);
    assert!(got[0].content.ends_with("(truncated)"));
    assert!(got[0].content.len() < 20 * 1024);
}

#[test]
fn truncate_utf8_splits_at_char_boundary() {
    // 多字节字符恰跨截断点：回退到 char 边界，不产出非法 UTF-8。
    // 每个「界」3 字节：max=4 → 截在第 2 个字（界=字节 3..6）内部，
    // 应回退到第 1 个字之后。
    let s = "界界界界";
    let out = super::truncate_utf8(s.as_bytes(), 4);
    assert!(out.starts_with('界'), "{out}");
    assert!(out.contains("(truncated)"), "{out}");
    assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    // max 恰好落在 lead byte 上：整个字被排除，不悬空。
    let out2 = super::truncate_utf8(s.as_bytes(), 3);
    assert_eq!(out2.chars().next().unwrap(), '界');
    assert_eq!(out2.chars().count(), 2 + 13); // 2 字 + "\n\n(truncated)"（13 字符）
}

#[test]
fn truncate_utf8_ascii_fast_path() {
    assert_eq!(super::truncate_utf8(b"short", 16), "short");
}

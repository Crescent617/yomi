//! source 解析测试（取货走真网，单测只覆盖解析）。

use super::{fetch_source, parse_source, PkgSource};

#[test]
fn local_dir_wins() {
    let dir = tempfile::tempdir().unwrap();
    let src = parse_source(dir.path().to_str().unwrap()).unwrap();
    match src {
        PkgSource::Local(p) => assert_eq!(p, dir.path().canonicalize().unwrap()),
        other @ PkgSource::Git { .. } => panic!("expected Local, got {other:?}"),
    }
    // 相对路径存在的目录也算 Local（测试 cwd 是 crate 根，src/ 必在）。
    let src = parse_source("src").unwrap();
    assert!(matches!(src, PkgSource::Local(_)));
}

#[test]
fn github_shorthand() {
    let cases = [
        (
            "Crescent617/yomi-extensions",
            "https://github.com/Crescent617/yomi-extensions.git",
            None,
            None,
        ),
        (
            "Crescent617/yomi-extensions/ext/memory-system",
            "https://github.com/Crescent617/yomi-extensions.git",
            Some("ext/memory-system"),
            None,
        ),
        (
            "Crescent617/yomi-extensions@v1.2.3",
            "https://github.com/Crescent617/yomi-extensions.git",
            None,
            Some("v1.2.3"),
        ),
        (
            "Crescent617/yomi-extensions/ext/demo@main",
            "https://github.com/Crescent617/yomi-extensions.git",
            Some("ext/demo"),
            Some("main"),
        ),
        (
            "https://github.com/Crescent617/yomi-extensions",
            "https://github.com/Crescent617/yomi-extensions.git",
            None,
            None,
        ),
        (
            "https://github.com/Crescent617/yomi-extensions.git",
            "https://github.com/Crescent617/yomi-extensions.git",
            None,
            None,
        ),
    ];
    for (input, url, subdir, ref_) in cases {
        let src = parse_source(input).unwrap();
        match src {
            PkgSource::Git {
                url: u,
                subdir: d,
                ref_: r,
            } => {
                assert_eq!(u, url, "{input}");
                assert_eq!(d.as_deref(), subdir, "{input}");
                assert_eq!(r.as_deref(), ref_, "{input}");
            }
            other @ PkgSource::Local(_) => panic!("{input}: expected Git, got {other:?}"),
        }
    }
}

#[test]
fn bad_sources_rejected() {
    for bad in [
        "",
        "owneronly",
        "a/b/../c",
        "a/b/c d",
        "../x",
        "@main",
        "a@",
    ] {
        assert!(parse_source(bad).is_err(), "{bad} should be rejected");
    }
}

#[tokio::test]
async fn local_source_fetches_without_temp() {
    let dir = tempfile::tempdir().unwrap();
    let src = parse_source(dir.path().to_str().unwrap()).unwrap();
    let (tmp, root, rev) = fetch_source(&src).await.unwrap();
    assert!(tmp.is_none());
    assert!(rev.is_none());
    assert_eq!(root, dir.path().canonicalize().unwrap());
}

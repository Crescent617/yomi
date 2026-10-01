//! installed（ext.lock 读写与扫描）测试。

use super::{list_installed, read_installed, write_install_meta, Provenance, Resources};

fn pkg_with_meta() -> (tempfile::TempDir, std::path::PathBuf) {
    let data = tempfile::tempdir().unwrap();
    let ext = data.path().join("extensions/demo");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(
        ext.join("ext.toml"),
        "[ext]\nname = \"demo\"\nversion = \"0.1.0\"\ndescription = \"t\"\n",
    )
    .unwrap();
    (data, ext)
}

#[test]
fn write_then_read_roundtrip() {
    let (_data, ext) = pkg_with_meta();
    write_install_meta(
        &ext,
        "abc123",
        &Provenance {
            source: "owner/repo@main".to_string(),
            rev: Some("deadbeef".to_string()),
        },
        &Resources {
            cron: vec!["ext:demo:tick".to_string()],
            hooks: vec!["pre_tool_use/50-guard".to_string()],
            bins: vec!["recall".to_string()],
            snippets: vec!["memory.md".to_string()],
        },
    )
    .unwrap();

    let installed = read_installed(&ext).unwrap();
    assert_eq!(installed.name(), "demo");
    assert_eq!(
        installed.mount_paths(),
        vec!["hooks/pre_tool_use/50-guard", "bin/recall"]
    );
    let meta = installed.meta.unwrap();
    assert_eq!(meta.source, "owner/repo@main");
    assert_eq!(meta.rev.as_deref(), Some("deadbeef"));
    assert_eq!(meta.content_hash, "abc123");
    assert_eq!(meta.resources.cron, vec!["ext:demo:tick"]);
    assert_eq!(meta.resources.bins, vec!["recall"]);
}

#[test]
fn foreign_dir_has_no_meta() {
    let (_data, ext) = pkg_with_meta();
    let installed = read_installed(&ext).unwrap();
    assert!(installed.meta.is_none());
    assert!(installed.mount_paths().is_empty());
}

#[tokio::test]
async fn list_installed_sorts_and_marks_foreign() {
    let data = tempfile::tempdir().unwrap();
    for (name, with_meta) in [("zeta", true), ("alpha", false)] {
        let ext = data.path().join("extensions").join(name);
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(
            ext.join("ext.toml"),
            format!("[ext]\nname = \"{name}\"\nversion = \"0\"\ndescription = \"d\"\n"),
        )
        .unwrap();
        if with_meta {
            write_install_meta(
                &ext,
                "h",
                &Provenance {
                    source: "s".to_string(),
                    rev: None,
                },
                &Resources::default(),
            )
            .unwrap();
        }
    }
    let all = list_installed(data.path()).await;
    let names: Vec<&str> = all.iter().map(|i| i.name()).collect();
    assert_eq!(names, vec!["alpha", "zeta"]);
    assert!(all[0].meta.is_none(), "alpha is foreign");
    assert!(all[1].meta.is_some(), "zeta installed");
}

#[test]
fn broken_dir_placeholder() {
    // ext.toml 损坏：list 仍看见它（健康检查要看见），meta 为空。
    let data = tempfile::tempdir().unwrap();
    let ext = data.path().join("extensions/broken");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(ext.join("ext.toml"), "not [valid toml").unwrap();
    // read_installed 本身报错，但 list 的占位兜底不 panic。
    let all = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(list_installed(data.path()));
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name(), "broken");
    assert!(all[0].meta.is_none());
}

#[test]
fn install_meta_written_to_lock_and_manifest_untouched() {
    let (_data, ext) = pkg_with_meta();
    let manifest_before = std::fs::read_to_string(ext.join("ext.toml")).unwrap();
    write_install_meta(
        &ext,
        "h",
        &Provenance {
            source: "s".to_string(),
            rev: None,
        },
        &Resources::default(),
    )
    .unwrap();
    // ext.toml 原封不动（作者的 manifest 归作者）。
    assert_eq!(
        std::fs::read_to_string(ext.join("ext.toml")).unwrap(),
        manifest_before
    );
    // ext.lock 独立成文件，合法 TOML，字段齐全。
    let raw = std::fs::read_to_string(ext.join("ext.lock")).unwrap();
    let v: toml::Table = raw.parse().unwrap();
    assert_eq!(v["source"].as_str(), Some("s"));
    assert_eq!(v["content_hash"].as_str(), Some("h"));
    assert!(v["installed_at"].as_str().is_some());
    assert!(v["resources"].is_table());
}

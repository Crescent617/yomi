//! installed（单文件注册表 ext.lock 的读写与扫描）测试。

use super::{
    list_installed, lockfile_path, read_installed, read_lockfile, write_lockfile, ExtLockfile,
    ExtensionHealth, ExtensionInfo, LockEntry, Resources,
};

fn pkg() -> (tempfile::TempDir, std::path::PathBuf) {
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

fn entry(name: &str) -> LockEntry {
    LockEntry {
        name: name.to_string(),
        source: "owner/repo@main".to_string(),
        rev: Some("deadbeef".to_string()),
        content_hash: "abc123".to_string(),
        resources: Resources {
            cron: vec![format!("ext:{name}:tick")],
            hooks: vec!["pre_tool_use/50-guard".to_string()],
            bins: vec!["recall".to_string()],
            snippets: vec!["memory.md".to_string()],
        },
        installed_at: chrono::Utc::now(),
    }
}

#[test]
fn write_then_read_roundtrip() {
    let (data, ext) = pkg();
    let mut lf = ExtLockfile::default();
    lf.upsert(entry("demo"));
    write_lockfile(data.path(), &lf).unwrap();

    let installed = read_installed(data.path(), &ext).unwrap();
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
    let (data, ext) = pkg();
    let installed = read_installed(data.path(), &ext).unwrap();
    assert!(installed.meta.is_none());
    assert!(installed.mount_paths().is_empty());
}

#[test]
fn lockfile_upsert_sorts_and_replaces() {
    let (data, _ext) = pkg();
    let mut lf = ExtLockfile::default();
    lf.upsert(entry("zeta"));
    lf.upsert(entry("alpha"));
    lf.upsert(entry("mike"));
    // 同名替换：内容更新、数量不增。
    let mut updated = entry("mike");
    updated.content_hash = "new".to_string();
    lf.upsert(updated);
    assert_eq!(lf.extensions.len(), 3);
    let names: Vec<&str> = lf.extensions.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "mike", "zeta"]);
    assert_eq!(lf.get("mike").unwrap().content_hash, "new");
    assert!(lf.remove("alpha"));
    assert_eq!(lf.extensions.len(), 2);
    // 落盘再读仍有序。
    write_lockfile(data.path(), &lf).unwrap();
    let back = read_lockfile(data.path());
    let names: Vec<&str> = back.extensions.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["mike", "zeta"]);
}

#[test]
fn in_dir_lock_is_inert_author_file() {
    // 包目录里作者自带的 ext.lock 不构成所有权证明（注册表才是），
    // 也不被解析为 meta——它只是普通包文件。
    let (data, ext) = pkg();
    std::fs::write(
        ext.join("ext.lock"),
        "source = \"fake\"\ncontent_hash = \"x\"\n",
    )
    .unwrap();
    let installed = read_installed(data.path(), &ext).unwrap();
    assert!(
        installed.meta.is_none(),
        "in-dir ext.lock must not be read as install meta"
    );
}

#[tokio::test]
async fn list_installed_sorts_and_marks_foreign() {
    let data = tempfile::tempdir().unwrap();
    let mut lf = ExtLockfile::default();
    let zeta = data.path().join("extensions/zeta");
    std::fs::create_dir_all(&zeta).unwrap();
    std::fs::write(
        zeta.join("ext.toml"),
        "[ext]\nname = \"zeta\"\nversion = \"0\"\ndescription = \"d\"\n",
    )
    .unwrap();
    lf.upsert(entry("zeta"));
    // alpha：foreign（无条目）。
    let alpha = data.path().join("extensions/alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::write(
        alpha.join("ext.toml"),
        "[ext]\nname = \"alpha\"\nversion = \"0\"\ndescription = \"d\"\n",
    )
    .unwrap();
    write_lockfile(data.path(), &lf).unwrap();

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
fn corrupt_lockfile_treated_as_empty() {
    let (data, ext) = pkg();
    std::fs::write(lockfile_path(data.path()), "not [valid toml").unwrap();
    // 不 panic、无 meta（全部 foreign），目录仍可见。
    let installed = read_installed(data.path(), &ext).unwrap();
    assert!(installed.meta.is_none());
}

#[test]
fn lockfile_is_single_toml_with_all_fields() {
    let (data, ext) = pkg();
    let manifest_before = std::fs::read_to_string(ext.join("ext.toml")).unwrap();
    let mut lf = ExtLockfile::default();
    lf.upsert(entry("demo"));
    write_lockfile(data.path(), &lf).unwrap();
    // ext.toml 原封不动（作者的 manifest 归作者）。
    assert_eq!(
        std::fs::read_to_string(ext.join("ext.toml")).unwrap(),
        manifest_before
    );
    // 包目录里没有 lock；注册表是合法 TOML、字段齐全。
    assert!(!ext.join("ext.lock").exists());
    let raw = std::fs::read_to_string(lockfile_path(data.path())).unwrap();
    let v: toml::Table = raw.parse().unwrap();
    assert_eq!(v["version"].as_integer(), Some(1));
    let exts = v["extensions"].as_array().unwrap();
    assert_eq!(exts.len(), 1);
    assert_eq!(exts[0]["name"].as_str(), Some("demo"));
    assert_eq!(exts[0]["source"].as_str(), Some("owner/repo@main"));
    assert_eq!(exts[0]["content_hash"].as_str(), Some("abc123"));
    assert!(exts[0]["resources"].is_table());
    assert!(exts[0]["installed_at"].is_str());
}

#[test]
fn extension_info_serializes_snake_case_and_omits_absent_meta() {
    let full = ExtensionInfo {
        name: "demo".to_string(),
        version: "0.1.0".to_string(),
        description: "t".to_string(),
        dir: "/data/extensions/demo".to_string(),
        health: ExtensionHealth::Ok,
        source: Some("owner/repo@main".to_string()),
        rev: Some("1a2b3c4d".to_string()),
        content_hash: Some("hash".to_string()),
        installed_at: Some("2026-11-01T00:00:00Z".parse().unwrap()),
        resources: Some(Resources::default()),
    };
    let v = serde_json::to_value(&full).unwrap();
    assert_eq!(v["health"], "ok");
    assert_eq!(v["name"], "demo");
    assert!(v["source"].is_string());
    assert!(v["rev"].is_string());
    // Wire delta vs the old `Value` shape: timestamps render with a `Z` suffix.
    assert!(v["installed_at"].as_str().unwrap().ends_with('Z'));
    assert!(v["resources"].is_object());
    // Round-trips through the remote client path (`call_json` deserializes).
    let back: ExtensionInfo = serde_json::from_value(v).unwrap();
    assert_eq!(back.health, ExtensionHealth::Ok);
    assert_eq!(back.source.as_deref(), Some("owner/repo@main"));
    assert!(back.resources.is_some());

    // foreign row: registry entry absent → optional fields omitted,
    // health remains a snake_case string, and the omitted fields
    // deserialize back to None.
    let foreign = ExtensionInfo {
        health: ExtensionHealth::Unreadable,
        source: None,
        rev: None,
        content_hash: None,
        installed_at: None,
        resources: None,
        ..full
    };
    let v = serde_json::to_value(&foreign).unwrap();
    assert_eq!(v["health"], "unreadable");
    assert!(v.get("source").is_none());
    assert!(v.get("rev").is_none());
    assert!(v.get("installed_at").is_none());
    assert!(v.get("resources").is_none());
    let back: ExtensionInfo = serde_json::from_value(v).unwrap();
    assert_eq!(back.health, ExtensionHealth::Unreadable);
    assert!(back.source.is_none());
    assert!(back.resources.is_none());
}

#[test]
fn extension_health_as_str_matches_serde() {
    for h in [
        ExtensionHealth::Ok,
        ExtensionHealth::Modified,
        ExtensionHealth::Foreign,
        ExtensionHealth::Unreadable,
        ExtensionHealth::Oversized,
    ] {
        // CLI column and `doctor` print `as_str()`; the wire uses serde.
        // They must never diverge.
        assert_eq!(
            serde_json::to_value(h).unwrap(),
            serde_json::Value::String(h.as_str().to_string()),
            "as_str/serde mismatch for {h:?}"
        );
    }
}

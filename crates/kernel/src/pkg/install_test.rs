//! install/remove 生命周期测试：tempdir 建包、内存 cron store。
//! symlink 语义 unix-only（windows CI 无开发者模式时 symlink 需特权）。

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use super::{install, remove, MountStatus};

const MANIFEST: &str = r#"
[ext]
name = "demo"
version = "0.1.0"
description = "test package"

[[cron]]
name = "dream"
schedule = "0 4 * * *"
message_file = "prompts/dream.txt"
"#;

fn write_pkg() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ext.toml"), MANIFEST).unwrap();
    std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
    std::fs::write(dir.path().join("prompts/dream.txt"), "dream the dream").unwrap();
    std::fs::create_dir_all(dir.path().join("hooks/pre_tool_use")).unwrap();
    let guard = dir.path().join("hooks/pre_tool_use/50-guard");
    std::fs::write(&guard, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::metadata(&guard)
        .unwrap()
        .permissions()
        .set_mode(0o755);
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    let recall = dir.path().join("bin/recall");
    std::fs::write(&recall, "#!/bin/sh\necho recall\n").unwrap();
    std::fs::metadata(&recall)
        .unwrap()
        .permissions()
        .set_mode(0o755);
    std::fs::create_dir_all(dir.path().join("snippets")).unwrap();
    std::fs::write(dir.path().join("snippets/memory.md"), "# Rules\ndo x").unwrap();
    dir
}

fn manifest_of(pkg: &tempfile::TempDir) -> super::super::ExtManifest {
    super::super::parse_manifest(pkg.path()).unwrap()
}

async fn test_cron_store() -> Arc<dyn crate::cron::CronStore> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::storage::migrations::run_migrations(&pool)
        .await
        .unwrap();
    Arc::new(crate::cron::SqliteCronStore::new(pool))
}

async fn cron_job_names(store: &Arc<dyn crate::cron::CronStore>) -> Vec<String> {
    store
        .list(None, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.name)
        .collect()
}

#[tokio::test]
async fn install_creates_everything() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    let report = install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();

    assert_eq!(report.name, "demo");
    assert_eq!(report.snippets, vec!["memory.md"]);
    assert!(report.cron[0].created);
    assert_eq!(report.hooks[0].status, MountStatus::Linked);
    assert_eq!(report.bins[0].status, MountStatus::Linked);

    // extensions/<名> symlink → 源。
    let ext_link = data.path().join("extensions/demo");
    assert_eq!(
        std::fs::read_link(&ext_link).unwrap(),
        pkg.path().canonicalize().unwrap()
    );
    // hook 与 bin 挂载：文本目标指向 extensions/<名>/...（两级链）。
    let hook_link = data.path().join("hooks/pre_tool_use/50-guard");
    assert_eq!(
        std::fs::read_link(&hook_link).unwrap(),
        ext_link.join("hooks/pre_tool_use/50-guard")
    );
    assert!(hook_link.is_file()); // 跟随两级 symlink 解析到源文件
    let bin_link = data.path().join("bin/recall");
    assert!(bin_link.is_file());

    // cron 收养：全名 + 内容来自 message_file。
    let jobs = crate::cron::CronStore::list(&*store, None, 10)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "ext:demo:dream");
    let crate::cron::CronAction::SendMessage { content, .. } = &jobs[0].action else {
        panic!("expected SendMessage");
    };
    assert_eq!(content, "dream the dream");
}

#[tokio::test]
async fn install_is_idempotent() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();
    let second = install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();

    assert_eq!(second.hooks[0].status, MountStatus::Already);
    assert_eq!(second.bins[0].status, MountStatus::Already);
    assert!(!second.cron[0].created, "ensure: existing job untouched");
    assert_eq!(cron_job_names(&store).await.len(), 1, "no duplicate cron");
}

#[tokio::test]
async fn install_refuses_occupied_slot() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    // 用户自己的 recall 先占了 bin 槽位。
    std::fs::create_dir_all(data.path().join("bin")).unwrap();
    std::fs::write(data.path().join("bin/recall"), "user's own").unwrap();

    let err = install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
    // 拒绝覆盖：用户的文件还在，cron 没收养。
    // （extensions/<名> 收编与先扫描到的 hook 挂载已发生是设计内行为
    // ——不做跨 fs/sqlite 事务，重跑 install 收敛、remove 可清。）
    assert_eq!(
        std::fs::read_to_string(data.path().join("bin/recall")).unwrap(),
        "user's own"
    );
    assert!(data
        .path()
        .join("hooks/pre_tool_use/50-guard")
        .symlink_metadata()
        .is_ok());
    assert!(cron_job_names(&store).await.is_empty());
}

#[tokio::test]
async fn install_recovers_after_partial_mounts() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    // 模拟上一次安装挂在挂载阶段：hook 已建，bin/cron 还没做。
    std::fs::create_dir_all(data.path().join("extensions")).unwrap();
    std::os::unix::fs::symlink(
        pkg.path().canonicalize().unwrap(),
        data.path().join("extensions/demo"),
    )
    .unwrap();
    std::fs::create_dir_all(data.path().join("hooks/pre_tool_use")).unwrap();
    std::os::unix::fs::symlink(
        data.path()
            .join("extensions/demo/hooks/pre_tool_use/50-guard"),
        data.path().join("hooks/pre_tool_use/50-guard"),
    )
    .unwrap();

    // 重跑收敛：已建的部分 Already，缺的补齐。
    let report = install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();
    assert_eq!(report.hooks[0].status, MountStatus::Already);
    assert_eq!(report.bins[0].status, MountStatus::Linked);
    assert!(report.cron[0].created);
}

#[tokio::test]
async fn remove_rolls_back_everything() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();

    // remove 的正确性不依赖记录——传 None 走退化路径（扫包目录 + cron
    // 前缀），正是设计要验证的。
    let report = remove(data.path(), "demo", &store, None).await.unwrap();

    assert_eq!(report.cron_removed, vec!["ext:demo:dream"]);
    assert!(report.ext_dir_removed);
    assert!(!data.path().join("hooks/pre_tool_use/50-guard").exists());
    assert!(!data.path().join("bin/recall").exists());
    assert!(!data.path().join("extensions/demo").exists());
    assert!(cron_job_names(&store).await.is_empty());
}

#[tokio::test]
async fn remove_leaves_repointed_slot() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();

    // 用户把 bin 槽位换成自己的文件。
    std::fs::remove_file(data.path().join("bin/recall")).unwrap();
    std::fs::write(data.path().join("bin/recall"), "user's own").unwrap();

    let report = remove(data.path(), "demo", &store, None).await.unwrap();
    assert_eq!(report.mounts_left, vec!["bin/recall"]);
    assert_eq!(
        std::fs::read_to_string(data.path().join("bin/recall")).unwrap(),
        "user's own"
    );
}

#[tokio::test]
async fn copy_mode_installs_real_dir() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    let report = install(
        data.path(),
        pkg.path(),
        true,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();
    assert_eq!(report.name, "demo");

    let ext_dir = data.path().join("extensions/demo");
    assert!(!ext_dir.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(ext_dir.join("bin/recall").is_file());
    // 源删掉后 copy 模式仍然完整（与 symlink 模式的对照语义）。
    let recall_target = std::fs::read_link(data.path().join("bin/recall")).unwrap();
    assert!(recall_target.starts_with(&ext_dir));
}

#[tokio::test]
async fn remove_rejects_traversal_names() {
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    for bad in ["..", "../extensions", "a/b", ".", "Demo"] {
        let err = remove(data.path(), bad, &store, None).await.unwrap_err();
        assert!(err.to_string().contains("invalid"), "{bad}: {err}");
    }
    // 数据目录安然无恙。
    assert!(data.path().exists());
}

#[tokio::test]
async fn remove_leaves_user_placed_real_dir() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();

    // 用户把 extensions/demo 换成自己的实体目录。
    std::fs::remove_file(data.path().join("extensions/demo")).unwrap();
    std::fs::create_dir_all(data.path().join("extensions/demo/my-notes")).unwrap();
    std::fs::write(
        data.path().join("extensions/demo/my-notes/keep.txt"),
        "precious",
    )
    .unwrap();

    // 无记录（symlink 模式）：不删实体目录，用户数据保留。
    let report = remove(data.path(), "demo", &store, None).await.unwrap();
    assert!(!report.ext_dir_removed);
    assert!(data
        .path()
        .join("extensions/demo/my-notes/keep.txt")
        .exists());

    // 有记录但 mode=symlink：同样不删。
    let record = super::super::ExtInstall {
        name: "demo".to_string(),
        source: pkg.path().to_string_lossy().into_owned(),
        mode: "symlink".to_string(),
        version: "0.1.0".to_string(),
        resources: Default::default(),
        installed_at: chrono::Utc::now(),
    };
    let report = remove(data.path(), "demo", &store, Some(&record))
        .await
        .unwrap();
    assert!(!report.ext_dir_removed);
    assert!(data
        .path()
        .join("extensions/demo/my-notes/keep.txt")
        .exists());
}

#[tokio::test]
async fn remove_deletes_copy_mode_dir_with_record() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        true,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();

    let record = super::super::ExtInstall {
        name: "demo".to_string(),
        source: pkg.path().to_string_lossy().into_owned(),
        mode: "copy".to_string(),
        version: "0.1.0".to_string(),
        resources: Default::default(),
        installed_at: chrono::Utc::now(),
    };
    let report = remove(data.path(), "demo", &store, Some(&record))
        .await
        .unwrap();
    assert!(report.ext_dir_removed);
    assert!(!data.path().join("extensions/demo").exists());
}

#[tokio::test]
async fn copy_reinstall_refreshes_with_record() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    install(
        data.path(),
        pkg.path(),
        true,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();
    // 无记录时 copy 重装拒绝（保守）。
    let err = install(
        data.path(),
        pkg.path(),
        true,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
    // 有记录（mode=copy）时原位刷新。
    let report = install(
        data.path(),
        pkg.path(),
        true,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        Some("copy"),
    )
    .await
    .unwrap();
    assert_eq!(report.name, "demo");
    assert!(data.path().join("extensions/demo/bin/recall").is_file());
}

#[tokio::test]
async fn copy_install_converts_symlink_slot() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    // 先 symlink 装。
    install(
        data.path(),
        pkg.path(),
        false,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
    )
    .await
    .unwrap();
    assert!(data
        .path()
        .join("extensions/demo")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());

    // 再 --copy 装：槽位换成实体目录，bin 链路指向 copy。
    let report = install(
        data.path(),
        pkg.path(),
        true,
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        Some("symlink"),
    )
    .await
    .unwrap();
    assert_eq!(report.name, "demo");
    let ext_dir = data.path().join("extensions/demo");
    assert!(!ext_dir.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(ext_dir.join("bin/recall").is_file());
    let target = std::fs::read_link(data.path().join("bin/recall")).unwrap();
    assert!(
        target.starts_with(&ext_dir),
        "bin links into the copy: {target:?}"
    );
}

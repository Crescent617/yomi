//! install/remove 生命周期测试：tempdir 建包、内存 cron store。
//! symlink 挂载语义 unix-only（windows CI 无开发者模式时 symlink 需特权）。

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

fn provenance() -> super::super::Provenance {
    super::super::Provenance {
        source: "test/pkg".to_string(),
        rev: None,
    }
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
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();

    assert_eq!(report.name, "demo");
    assert_eq!(report.snippets, vec!["memory.md"]);
    assert!(!report.content_hash.is_empty());
    assert!(report.cron[0].created);
    assert_eq!(report.hooks[0].status, MountStatus::Linked);
    assert_eq!(report.bins[0].status, MountStatus::Linked);

    // extensions/<名> 实体复制（统一 copy，不再是 symlink 槽位）。
    let ext_dir = data.path().join("extensions/demo");
    assert!(ext_dir.is_dir());
    assert!(!ext_dir.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(ext_dir.join("bin/recall").is_file());
    // hook 与 bin 挂载：symlink 文本目标指向 extensions/<名>/...。
    let hook_link = data.path().join("hooks/pre_tool_use/50-guard");
    assert_eq!(
        std::fs::read_link(&hook_link).unwrap(),
        ext_dir.join("hooks/pre_tool_use/50-guard")
    );
    assert!(hook_link.is_file());
    assert!(data.path().join("bin/recall").is_file());

    // [install] 段落盘：source 记录 + hash 与包内容一致。
    let installed = super::super::read_installed(&ext_dir).unwrap();
    let meta = installed.meta.expect("[install] written");
    assert_eq!(meta.source, "test/pkg");
    assert_eq!(meta.content_hash, report.content_hash);
    // list 侧健康判定的不变量：装完立刻重算必须仍是同一 hash（段已写入
    // 也能比对——hash 算法剔除了 [install] 表）。
    let now = super::super::package_hash(&ext_dir).unwrap();
    assert_eq!(
        now, meta.content_hash,
        "health must be ok right after install"
    );

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
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();
    // 第二次：槽位已有实体目录 + 安装记录 → allow_replace 原位刷新。
    let second = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        true,
        &provenance(),
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
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
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
    // 部分安装落了 [install] 段（资源=已建挂载）：挪走冲突项后重跑
    // 拿 allow_replace 原位刷新收敛，remove 也能精确回滚已建部分。
    let ext_dir = data.path().join("extensions/demo");
    let installed = super::super::read_installed(&ext_dir).unwrap();
    let meta = installed.meta.expect("partial install recorded");
    assert_eq!(meta.resources.hooks, vec!["pre_tool_use/50-guard"]);
    assert_eq!(meta.resources.bins, vec!["recall"]);
    std::fs::remove_file(data.path().join("bin/recall")).unwrap();
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        true,
        &provenance(),
    )
    .await
    .unwrap();
    assert!(report.cron[0].created, "re-run converges: cron adopted");
    assert!(data.path().join("bin/recall").is_file());
}

#[tokio::test]
async fn install_recovers_after_partial_mounts() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    // 完整装一次，然后模拟"挂载阶段被中断"的现场：hook/bin symlink
    // 与 cron job 都消失，只剩实体目录 + [install] 段。
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();
    std::fs::remove_file(data.path().join("hooks/pre_tool_use/50-guard")).unwrap();
    std::fs::remove_file(data.path().join("bin/recall")).unwrap();
    let jobs = crate::cron::CronStore::list(&*store, None, 10)
        .await
        .unwrap();
    for job in jobs {
        store.delete(&job.id).await.unwrap();
    }

    // 重跑收敛：已建的部分 Already，缺的补齐。
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        true,
        &provenance(),
    )
    .await
    .unwrap();
    assert_eq!(report.hooks[0].status, MountStatus::Linked);
    assert_eq!(report.bins[0].status, MountStatus::Linked);
    assert!(report.cron[0].created);
}

#[tokio::test]
async fn reinstall_refreshes_content() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();

    // 源内容更新后重装（有记录 → allow_replace）：原位替换，内容跟新。
    std::fs::write(pkg.path().join("snippets/memory.md"), "updated rules").unwrap();
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        true,
        &provenance(),
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(data.path().join("extensions/demo/snippets/memory.md")).unwrap();
    assert_eq!(content, "updated rules");
}

#[tokio::test]
async fn reinstall_refuses_without_ownership() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();

    // 无 allow_replace（调用方未确认是本包装的记录）：拒绝刷新。
    let err = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
}

#[tokio::test]
async fn remove_rolls_back_everything() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();

    // 退化路径（installed=None）：正确性不依赖记录——cron 前缀清扫 +
    // 挂载指向判定照样精确回滚；实体目录没有归属证明则留下不删。
    let report = remove(data.path(), "demo", &store, None).await.unwrap();
    assert_eq!(report.cron_removed, vec!["ext:demo:dream"]);
    assert!(!report.ext_dir_removed);
    assert!(!data.path().join("hooks/pre_tool_use/50-guard").exists());
    assert!(!data.path().join("bin/recall").exists());
    assert!(cron_job_names(&store).await.is_empty());

    // 带上 read_installed 的结果（含 [install] 段）：目录也删掉。
    let ext_dir = data.path().join("extensions/demo");
    let installed = super::super::read_installed(&ext_dir).ok();
    let report = remove(data.path(), "demo", &store, installed.as_ref())
        .await
        .unwrap();
    assert!(report.ext_dir_removed);
    assert!(!ext_dir.exists());
}

#[tokio::test]
async fn remove_leaves_repointed_slot() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
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
async fn remove_leaves_foreign_dir() {
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    // 用户把自己的目录放进槽位（无 [install] 段 = foreign）：不删。
    let ext_dir = data.path().join("extensions/demo");
    std::fs::create_dir_all(ext_dir.join("my-notes")).unwrap();
    std::fs::write(ext_dir.join("my-notes/keep.txt"), "precious").unwrap();
    std::fs::write(
        ext_dir.join("ext.toml"),
        "[ext]\nname = \"demo\"\nversion = \"0\"\ndescription = \"mine\"\n",
    )
    .unwrap();

    let installed = super::super::read_installed(&ext_dir).ok();
    let report = remove(data.path(), "demo", &store, installed.as_ref())
        .await
        .unwrap();
    assert!(!report.ext_dir_removed);
    assert!(ext_dir.join("my-notes/keep.txt").exists());
}

#[tokio::test]
async fn remove_deletes_installed_dir_with_meta() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();

    let ext_dir = data.path().join("extensions/demo");
    let installed = super::super::read_installed(&ext_dir).ok();
    assert!(installed.as_ref().is_some_and(|i| i.meta.is_some()));
    let report = remove(data.path(), "demo", &store, installed.as_ref())
        .await
        .unwrap();
    assert!(report.ext_dir_removed);
    assert!(!ext_dir.exists());
}

#[tokio::test]
async fn package_supplied_install_section_is_stripped() {
    // 包内自带 [install] 表 = 伪造归属证明：装时被剥掉，hash 覆盖剥后
    // 内容；之后 remove 走正常归属判定，用户数据不误删。
    let pkg = write_pkg();
    std::fs::write(
        pkg.path().join("ext.toml"),
        format!("{MANIFEST}\n[install]\nsource = \"fake\"\ncontent_hash = \"x\"\n"),
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();

    let ext_dir = data.path().join("extensions/demo");
    let raw = std::fs::read_to_string(ext_dir.join("ext.toml")).unwrap();
    let table: toml::Table = raw.parse().unwrap();
    let install_section = table.get("install").expect("our [install] written");
    // 只剩我们写的段（source 是 provenance 的，不是包内伪造的）。
    assert_eq!(
        install_section.get("source").and_then(|v| v.as_str()),
        Some("test/pkg")
    );
    // hash 与剥后内容一致：list 侧重算必须是 ok。
    let installed = super::super::read_installed(&ext_dir).unwrap();
    let meta = installed.meta.unwrap();
    assert_eq!(
        super::super::package_hash(&ext_dir).unwrap(),
        meta.content_hash
    );
}

#[tokio::test]
async fn oversized_file_rejected_before_touching_slots() {
    let pkg = write_pkg();
    // 2MB > 1MB 上限。
    std::fs::write(
        pkg.path().join("snippets/blob.md"),
        vec![b'x'; 2 * 1024 * 1024],
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    let err = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("exceeds"), "{err}");
    // 拒绝发生在复制/挂载/cron 之前：任何槽位都没被碰，可以直接修好
    // 重跑（不会撞 occupied Conflict）。
    assert!(!data.path().join("extensions/demo").exists());
    assert!(!data.path().join("hooks/pre_tool_use/50-guard").exists());
    assert!(cron_job_names(&store).await.is_empty());
    std::fs::write(pkg.path().join("snippets/blob.md"), "small now").unwrap();
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        false,
        &provenance(),
    )
    .await
    .unwrap();
}

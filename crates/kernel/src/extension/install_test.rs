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

/// 注册表条目（新签名 install 的 `previous` 参数来源）。
async fn installed_meta(data: &tempfile::TempDir, name: &str) -> Option<super::super::LockEntry> {
    super::super::read_lockfile(data.path()).get(name).cloned()
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

/// create 必败的 store 包装：cron 收养失败路径的验证道具。
struct FailCreateStore {
    inner: Arc<dyn crate::cron::CronStore>,
}

#[async_trait::async_trait]
impl crate::cron::CronStore for FailCreateStore {
    async fn create(&self, _job: &crate::cron::CronJob) -> Result<(), crate::cron::CronError> {
        Err(crate::cron::CronError::Storage(
            "injected failure".to_string(),
        ))
    }
    async fn get(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>, crate::cron::CronError> {
        self.inner.get(id).await
    }
    async fn get_by_name(
        &self,
        name: &str,
    ) -> Result<Option<crate::cron::CronJob>, crate::cron::CronError> {
        self.inner.get_by_name(name).await
    }
    async fn list(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>, crate::cron::CronError> {
        self.inner.list(status, limit).await
    }
    async fn update(
        &self,
        id: &crate::cron::CronJobId,
        input: &crate::cron::UpdateCronJobInput,
    ) -> Result<bool, crate::cron::CronError> {
        self.inner.update(id, input).await
    }
    async fn delete(&self, id: &crate::cron::CronJobId) -> Result<bool, crate::cron::CronError> {
        self.inner.delete(id).await
    }
    async fn list_by_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>, crate::cron::CronError> {
        self.inner.list_by_prefix(prefix, limit).await
    }
    async fn list_active(&self) -> Result<Vec<crate::cron::CronJob>, crate::cron::CronError> {
        self.inner.list_active().await
    }
    async fn record_execution(
        &self,
        id: &crate::cron::CronJobId,
        error: Option<String>,
    ) -> Result<(), crate::cron::CronError> {
        self.inner.record_execution(id, error).await
    }
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
        None,
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

    // 单文件注册表 ext.lock 落盘：source 记录 + hash 与包内容一致；
    // 包目录里没有 lock 文件。
    let lockfile = super::super::lockfile_path(data.path());
    assert!(lockfile.is_file());
    assert!(!ext_dir.join("ext.lock").exists());
    let meta = installed_meta(&data, "demo").await.expect("lock written");
    assert_eq!(meta.source, "test/pkg");
    assert_eq!(meta.content_hash, report.content_hash);
    // list 侧健康判定的不变量：装完立刻重算必须仍是同一 hash。
    let now = super::super::package_hash(&ext_dir).unwrap();
    assert_eq!(
        now, meta.content_hash,
        "health must be ok right after install"
    );

    // cron 收养：全名 + 内容来自已装目录的 message_file（与 hash/copy
    // 同一事实源，不读可能已清理的取货临时目录）。
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
        None,
        &provenance(),
    )
    .await
    .unwrap();
    // 第二次：槽位已有实体目录 + 安装记录 → 原位刷新。
    let prev = installed_meta(&data, "demo").await;
    let second = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        prev.as_ref(),
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
        None,
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
    // 部分安装落了注册表条目（资源=已建挂载）：挪走冲突项后重跑
    // 原位刷新收敛，remove 也能精确回滚已建部分。
    let meta = installed_meta(&data, "demo")
        .await
        .expect("partial install recorded");
    assert_eq!(meta.resources.hooks, vec!["pre_tool_use/50-guard"]);
    assert_eq!(meta.resources.bins, vec!["recall"]);
    std::fs::remove_file(data.path().join("bin/recall")).unwrap();
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        Some(&meta),
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
    // 与 cron job 都消失，只剩实体目录 + 注册表条目。
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
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
    let prev = installed_meta(&data, "demo").await;
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        prev.as_ref(),
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
        None,
        &provenance(),
    )
    .await
    .unwrap();

    // 源内容更新后重装（有记录 → 原位刷新），内容跟新。
    std::fs::write(pkg.path().join("snippets/memory.md"), "updated rules").unwrap();
    let prev = installed_meta(&data, "demo").await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        prev.as_ref(),
        &provenance(),
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(data.path().join("extensions/demo/snippets/memory.md")).unwrap();
    assert_eq!(content, "updated rules");
}

#[tokio::test]
async fn reinstall_sweeps_stale_mounts() {
    // v2 删掉了 v1 的 hook/bin：目录内容已原位替换，旧挂载必然悬空。
    // 不扫掉会 phantom-block 其他扩展装同名槽位，health 还看不见。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap();

    // v2：hook 与 bin 都没了。
    std::fs::remove_dir_all(pkg.path().join("hooks")).unwrap();
    std::fs::remove_dir_all(pkg.path().join("bin")).unwrap();
    let prev = installed_meta(&data, "demo").await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        prev.as_ref(),
        &provenance(),
    )
    .await
    .unwrap();

    assert!(!data
        .path()
        .join("hooks/pre_tool_use/50-guard")
        .symlink_metadata()
        .is_ok());
    assert!(!data.path().join("bin/recall").symlink_metadata().is_ok());
    // 保留下来的资源不动：cron 照常。
    assert_eq!(cron_job_names(&store).await, vec!["ext:demo:dream"]);
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
        None,
        &provenance(),
    )
    .await
    .unwrap();

    // 无 previous（调用方未查到本包的 sidecar 记录）：拒绝刷新。
    let err = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
}

#[tokio::test]
async fn install_refuses_foreign_dir_even_with_authored_lock() {
    // 作者随包自带 ext.lock 不能伪造所有权：用户手放的同名目录（内含
    // 作者式 ext.lock）必须被拒为 occupied，绝不能原位刷新删掉。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    let ext_dir = data.path().join("extensions/demo");
    std::fs::create_dir_all(ext_dir.join("precious")).unwrap();
    std::fs::write(ext_dir.join("precious/keep.txt"), "user data").unwrap();
    std::fs::write(
        ext_dir.join("ext.toml"),
        "[ext]\nname = \"demo\"\nversion = \"0\"\ndescription = \"mine\"\n",
    )
    .unwrap();
    // 注册表没有这个扩展——目录里的 ext.lock 只是用户文件。
    std::fs::write(
        ext_dir.join("ext.lock"),
        "source = \"fake\"\ncontent_hash = \"x\"\n",
    )
    .unwrap();

    let err = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
    // 用户目录原封不动。
    assert!(ext_dir.join("precious/keep.txt").exists());
    assert_eq!(
        std::fs::read_to_string(ext_dir.join("ext.lock")).unwrap(),
        "source = \"fake\"\ncontent_hash = \"x\"\n"
    );
}

#[tokio::test]
async fn install_records_partial_lock_on_cron_failure() {
    // cron 收养失败：目录/挂载已是事实，sidecar lock 必须照样落盘
    // （含已建 cron 名单为空），否则重跑被 occupied 挡住、"re-run
    // to converge" 成空话。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let inner = test_cron_store().await;
    let store: Arc<dyn crate::cron::CronStore> = Arc::new(FailCreateStore {
        inner: Arc::clone(&inner),
    });

    let err = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("injected failure"), "{err}");

    // 部分 lock 在：重跑（此时 create 已修好）能原位刷新收敛。
    let meta = installed_meta(&data, "demo")
        .await
        .expect("partial install recorded on cron failure");
    assert!(meta
        .resources
        .hooks
        .contains(&"pre_tool_use/50-guard".to_string()));
    let report = install(
        data.path(),
        pkg.path(),
        &inner,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        Some(&meta),
        &provenance(),
    )
    .await
    .unwrap();
    assert!(report.cron[0].created, "re-run converges after cron fix");
}

#[tokio::test]
async fn install_adopts_legacy_in_dir_lock_under_registry_lock() {
    // 0.10.55/56 存量现场：包目录里是旧版 in-dir lock、注册表无条目。
    // kernel 预读的 previous=None，install 必须持锁收养 legacy 条目并
    // 原位刷新（而非 occupied 拒绝），收养持久化进注册表。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap();

    // 模拟老版本：注册表清空，包目录里放旧格式 in-dir lock。
    std::fs::remove_file(super::super::lockfile_path(data.path())).unwrap();
    let ext_dir = data.path().join("extensions/demo");
    std::fs::write(
        ext_dir.join("ext.lock"),
        r#"source = "test/pkg"
content_hash = "legacyhash"
installed_at = "2026-10-01T22:00:00Z"

[resources]
cron = ["ext:demo:dream"]
hooks = ["pre_tool_use/50-guard"]
bins = ["recall"]
snippets = ["memory.md"]
"#,
    )
    .unwrap();

    // 源更新 + 重装（previous=None → install 内收养）。
    std::fs::write(pkg.path().join("snippets/memory.md"), "v2 rules").unwrap();
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap();

    // 原位刷新（内容跟新）+ 收养持久化。
    assert_eq!(
        std::fs::read_to_string(ext_dir.join("snippets/memory.md")).unwrap(),
        "v2 rules"
    );
    let meta = installed_meta(&data, "demo")
        .await
        .expect("legacy entry adopted into registry");
    assert_eq!(meta.resources.hooks, vec!["pre_tool_use/50-guard"]);
    assert_eq!(cron_job_names(&store).await, vec!["ext:demo:dream"]);
    let _ = report;
}

#[tokio::test]
async fn partial_failure_keeps_previous_resources_for_remove() {
    // 半途失败（挂载冲突）时部分条目必须 = 旧记录 ∪ 本次已建——只增
    // 不减：否则 remove 按记录回滚会漏摘旧挂载，悬空 symlink
    // phantom-block 后续安装。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap();

    // v2 加第二个 bin，但让用户文件占住它的槽位 → 挂载冲突、半途失败。
    std::fs::write(pkg.path().join("bin/extra"), "#!/bin/sh\n").unwrap();
    std::fs::create_dir_all(data.path().join("bin")).unwrap();
    std::fs::write(data.path().join("bin/extra"), "user's own").unwrap();

    let prev = installed_meta(&data, "demo").await;
    let err = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        prev.as_ref(),
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");

    // 部分条目保留了旧记录的全部资源（recall + 50-guard + cron），
    // remove 能精确回滚，不留悬空挂载。
    let meta = installed_meta(&data, "demo")
        .await
        .expect("partial entry present");
    assert!(meta.resources.bins.contains(&"recall".to_string()));
    assert!(meta
        .resources
        .hooks
        .contains(&"pre_tool_use/50-guard".to_string()));
    assert_eq!(meta.resources.cron, vec!["ext:demo:dream"]);

    std::fs::remove_file(data.path().join("bin/extra")).unwrap();
    let ext_dir = data.path().join("extensions/demo");
    let installed = super::super::read_installed(data.path(), &ext_dir).ok();
    let report = remove(data.path(), "demo", &store, installed.as_ref())
        .await
        .unwrap();
    assert!(report.ext_dir_removed);
    assert!(!data.path().join("bin/recall").symlink_metadata().is_ok());
    assert!(!data
        .path()
        .join("hooks/pre_tool_use/50-guard")
        .symlink_metadata()
        .is_ok());
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
        None,
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

    // 带上 read_installed 的结果（含注册表条目）：目录删除、注册表
    // 条目同步移除。
    let ext_dir = data.path().join("extensions/demo");
    let installed = super::super::read_installed(data.path(), &ext_dir).ok();
    let report = remove(data.path(), "demo", &store, installed.as_ref())
        .await
        .unwrap();
    assert!(report.ext_dir_removed);
    assert!(!ext_dir.exists());
    assert!(
        installed_meta(&data, "demo").await.is_none(),
        "registry entry removed with the extension"
    );
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
        None,
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

    // 用户把自己的目录放进槽位（无注册表条目 = foreign）：不删。
    let ext_dir = data.path().join("extensions/demo");
    std::fs::create_dir_all(ext_dir.join("my-notes")).unwrap();
    std::fs::write(ext_dir.join("my-notes/keep.txt"), "precious").unwrap();
    std::fs::write(
        ext_dir.join("ext.toml"),
        "[ext]\nname = \"demo\"\nversion = \"0\"\ndescription = \"mine\"\n",
    )
    .unwrap();

    let installed = super::super::read_installed(data.path(), &ext_dir).ok();
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
        None,
        &provenance(),
    )
    .await
    .unwrap();

    assert!(installed_meta(&data, "demo").await.is_some());
    let ext_dir = data.path().join("extensions/demo");
    let installed = super::super::read_installed(data.path(), &ext_dir).ok();
    let report = remove(data.path(), "demo", &store, installed.as_ref())
        .await
        .unwrap();
    assert!(report.ext_dir_removed);
    assert!(!ext_dir.exists());
}

#[tokio::test]
async fn manifest_copied_verbatim() {
    // ext.toml 原封不动：包内带 [install] 表也随它进副本——无害（归属
    // 只看包外 sidecar lock），author 的文件 byte 级保持原样。hash 与
    // 副本一致：list 侧重算必须 ok。
    let pkg = write_pkg();
    let authored = format!("{MANIFEST}\n[install]\nsource = \"fake\"\n");
    std::fs::write(pkg.path().join("ext.toml"), &authored).unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap();

    let ext_dir = data.path().join("extensions/demo");
    assert_eq!(
        std::fs::read_to_string(ext_dir.join("ext.toml")).unwrap(),
        authored,
        "manifest byte-identical"
    );
    let meta = installed_meta(&data, "demo").await.expect("lock written");
    assert_eq!(meta.source, "test/pkg");
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
        None,
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
        None,
        &provenance(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn git_metadata_dir_is_not_packaged() {
    // git 源 = clone 到临时目录直接喂 install：.git 是 VCS 元数据不是
    // 包内容，里面的 pack 文件动辄超 1MB——不排除会在默认玩法上把真实
    // 仓误判成"包文件超限"拒装。hash 与复制同口径跳过。
    let pkg = write_pkg();
    std::fs::create_dir_all(pkg.path().join(".git/objects/pack")).unwrap();
    std::fs::write(
        pkg.path().join(".git/objects/pack/x.pack"),
        vec![b'x'; 2 * 1024 * 1024],
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    // 超大 pack 不触发 1MB 拒绝。
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap();

    let ext_dir = data.path().join("extensions/demo");
    assert!(!ext_dir.join(".git").exists(), ".git not copied");
    assert!(ext_dir.join("bin/recall").is_file());
    // hash 一致性：list 侧重算仍须 ok。
    let meta = installed_meta(&data, "demo").await.unwrap();
    assert_eq!(
        super::super::package_hash(&ext_dir).unwrap(),
        meta.content_hash
    );
}

#[tokio::test]
async fn remove_leaves_repointed_symlink_slot() {
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        None,
        &provenance(),
    )
    .await
    .unwrap();

    // 用户把 bin 槽位换成指向别处的 symlink（不是实体文件）。
    std::fs::remove_file(data.path().join("bin/recall")).unwrap();
    std::os::unix::fs::symlink("/tmp/somewhere-else", data.path().join("bin/recall")).unwrap();

    let report = remove(data.path(), "demo", &store, None).await.unwrap();
    assert_eq!(report.mounts_left, vec!["bin/recall"]);
    // 指向判定保护：symlink 仍指用户目标。
    assert_eq!(
        std::fs::read_link(data.path().join("bin/recall")).unwrap(),
        std::path::Path::new("/tmp/somewhere-else")
    );
}

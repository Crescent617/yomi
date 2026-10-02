//! install/remove 生命周期测试：tempdir 建包、内存 cron store。
//! symlink 挂载语义 unix-only（windows CI 无开发者模式时 symlink 需特权）。

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use super::{install, remove, CronAdoptStatus, MountStatus};

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

/// 注册表里某扩展的当前条目（断言 provisional/终值语义用）。
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
        &provenance(),
    )
    .await
    .unwrap();

    assert_eq!(report.name, "demo");
    assert_eq!(report.snippets, vec!["memory.md"]);
    assert!(!report.content_hash.is_empty());
    assert_eq!(report.cron[0].status, CronAdoptStatus::Created);
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
        &provenance(),
    )
    .await
    .unwrap();
    // 第二次：槽位已有实体目录 + 安装记录 → 原位刷新。
    let second = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();

    assert_eq!(second.hooks[0].status, MountStatus::Already);
    assert_eq!(second.bins[0].status, MountStatus::Already);
    assert_eq!(
        second.cron[0].status,
        CronAdoptStatus::Untouched,
        "existing job untouched"
    );
    assert_eq!(cron_job_names(&store).await.len(), 1, "no duplicate cron");
}

#[tokio::test]
async fn refresh_sweeps_mounts_from_interrupted_previous_run() {
    // 双中断刷新：run1（v1→v2）在挂载阶段后被拦下（bin 冲突），现场
    // = 目录已是 v2、注册表是 provisional（资源沿用 v1 名单）、v2 的
    // 新挂载（hook B）已建。run2（v2→v3）若按旧记录名单清扫，B 既不在
    // v1 名单也不在 v3 目录里，会漏摘成永久悬空 symlink（phantom
    // block + health 看不见）。清扫必须扫挂载树本身。
    let hook_pkg = |hooks: &[&str], bins: &[&str]| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ext.toml"),
            "[ext]\nname = \"demo\"\nversion = \"0.1.0\"\ndescription = \"t\"\n",
        )
        .unwrap();
        for h in hooks {
            std::fs::create_dir_all(dir.path().join("hooks/pre_tool_use")).unwrap();
            std::fs::write(
                dir.path().join(format!("hooks/pre_tool_use/{h}")),
                "#!/bin/sh\n",
            )
            .unwrap();
        }
        for b in bins {
            std::fs::create_dir_all(dir.path().join("bin")).unwrap();
            let f = dir.path().join(format!("bin/{b}"));
            std::fs::write(&f, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    };
    let v1 = hook_pkg(&["50-guard"], &[]);
    let v2 = hook_pkg(&["60-other"], &["recall"]);
    let v3 = hook_pkg(&["70-third"], &[]);
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    // v1 正常装：hook A 挂载。
    install(
        data.path(),
        v1.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&v1),
        &provenance(),
    )
    .await
    .unwrap();

    // run1（v1→v2）：用户文件占 bin 槽位 → 挂载 hook B 后整体拒绝，
    // 注册表留下 provisional（资源 = v1 名单）。
    std::fs::create_dir_all(data.path().join("bin")).unwrap();
    std::fs::write(data.path().join("bin/recall"), "user's own").unwrap();
    let err = install(
        data.path(),
        v2.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&v2),
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
    let meta = installed_meta(&data, "demo").await.unwrap();
    assert_eq!(meta.resources.hooks, vec!["pre_tool_use/50-guard"]);
    // 崩溃现场：v2 的 hook B 已挂载，目录已是 v2。
    assert!(data
        .path()
        .join("hooks/pre_tool_use/60-other")
        .symlink_metadata()
        .is_ok());

    // run2（v2→v3）：v3 不含 B。扫树清扫必须把 B 摘掉，只留 C。
    let report = install(
        data.path(),
        v3.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&v3),
        &provenance(),
    )
    .await
    .unwrap();
    assert!(!data
        .path()
        .join("hooks/pre_tool_use/60-other")
        .symlink_metadata()
        .is_ok());
    assert!(data
        .path()
        .join("hooks/pre_tool_use/70-third")
        .symlink_metadata()
        .is_ok());
    assert_eq!(report.hooks.len(), 1);
    let meta = installed_meta(&data, "demo").await.unwrap();
    assert_eq!(meta.resources.hooks, vec!["pre_tool_use/70-third"]);
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
    // provisional 条目已落注册表（fresh 安装资源为空表，remove 靠
    // "旧资源 ∪ 包目录扫描 ∪ cron 前缀清扫"精确回滚）：挪走冲突项后
    // 重跑原位刷新收敛。
    let meta = installed_meta(&data, "demo")
        .await
        .expect("provisional entry recorded");
    assert!(meta.resources.hooks.is_empty());
    std::fs::remove_file(data.path().join("bin/recall")).unwrap();
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();
    assert_eq!(
        report.cron[0].status,
        CronAdoptStatus::Created,
        "re-run converges: cron adopted"
    );
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
        &provenance(),
    )
    .await
    .unwrap();
    assert_eq!(report.hooks[0].status, MountStatus::Linked);
    assert_eq!(report.bins[0].status, MountStatus::Linked);
    assert_eq!(report.cron[0].status, CronAdoptStatus::Created);
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
        &provenance(),
    )
    .await
    .unwrap();

    // 源内容更新后重装（有记录 → 原位刷新），内容跟新。
    std::fs::write(pkg.path().join("snippets/memory.md"), "updated rules").unwrap();
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
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
        &provenance(),
    )
    .await
    .unwrap();

    // v2：hook 与 bin 都没了。
    std::fs::remove_dir_all(pkg.path().join("hooks")).unwrap();
    std::fs::remove_dir_all(pkg.path().join("bin")).unwrap();
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
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
async fn reinstall_converges_from_registry_entry() {
    // 注册表是唯一事实源：装过之后注册表必有条目，重装一律是
    // 更新语义的原位刷新（幂等），不会因为调用方没预读记录而误拒。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();

    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();
    assert_eq!(report.hooks[0].status, MountStatus::Already);
    assert_eq!(report.bins[0].status, MountStatus::Already);
    assert_eq!(
        report.cron[0].status,
        CronAdoptStatus::Untouched,
        "已收养且一致不动"
    );
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
    // cron 收养失败：目录/挂载已是事实，provisional 条目必须在
    // 注册表里（fresh 安装资源为空表——remove 靠目录扫描与 cron
    // 前缀清扫兜底），否则重跑被 occupied 挡住、"re-run to
    // converge" 成空话。
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
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("injected failure"), "{err}");

    // provisional 条目在：重跑（此时 create 已修好）能原位刷新收敛。
    let meta = installed_meta(&data, "demo")
        .await
        .expect("provisional entry recorded on cron failure");
    assert!(meta.resources.hooks.is_empty());
    let report = install(
        data.path(),
        pkg.path(),
        &inner,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();
    assert_eq!(
        report.cron[0].status,
        CronAdoptStatus::Created,
        "re-run converges after cron fix"
    );
}

#[tokio::test]
async fn partial_failure_keeps_previous_resources_for_remove() {
    // 半途失败（挂载冲突）时注册表留下的是 provisional 条目：资源
    // 沿用旧记录。remove 的回滚 = 旧资源 ∪ 包目录扫描 ∪ cron 前缀
    // 清扫，所以照样精确、不留悬空挂载。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();

    // v2 加第二个 bin，但让用户文件占住它的槽位 → 挂载冲突、半途失败。
    std::fs::write(pkg.path().join("bin/extra"), "#!/bin/sh\n").unwrap();
    std::fs::create_dir_all(data.path().join("bin")).unwrap();
    std::fs::write(data.path().join("bin/extra"), "user's own").unwrap();

    let err = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
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

/// 带 init 钩子的最小包：脚本把 $YOMI_DATA_DIR 写进 marker（验证
/// 环境注入），并可按 mode 控制行为（ok / fail）。
fn write_init_pkg(mode: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ext.toml"),
        "[ext]\nname = \"demo\"\nversion = \"0.1.0\"\ndescription = \"t\"\ninit = \"scripts/init.sh\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    let script = dir.path().join("scripts/init.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s' \"$YOMI_DATA_DIR\" > \"${{YOMI_DATA_DIR}}/init-marker-{mode}\"\ncase {mode} in fail) echo boom >&2; exit 1;; esac\necho init-done\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

#[tokio::test]
async fn install_runs_init_with_data_dir_env() {
    let pkg = write_init_pkg("ok");
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();

    let init = report.init.expect("init report present");
    assert_eq!(init.path, "scripts/init.sh");
    assert!(init.output.contains("init-done"), "output: {}", init.output);
    // 环境注入：脚本看到的 YOMI_DATA_DIR = install 的 data_dir。
    assert_eq!(
        std::fs::read_to_string(data.path().join("init-marker-ok")).unwrap(),
        data.path().to_string_lossy()
    );
}

#[tokio::test]
async fn install_init_failure_aborts_and_recovers() {
    let pkg = write_init_pkg("fail");
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    let err = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().starts_with("init:"), "{err}");
    assert!(err.to_string().contains("boom"), "stderr tail: {err}");
    // provisional 条目在（重跑据此原位收敛而非 occupied）：
    assert!(installed_meta(&data, "demo").await.is_some());
    // 原地改脚本（模拟修好后重跑同一来源）。
    let script = pkg.path().join("scripts/init.sh");
    std::fs::write(&script, "#!/bin/sh\necho fixed\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();
    assert!(report.init.unwrap().output.contains("fixed"));
    assert!(!report.content_hash.is_empty());
}

#[tokio::test]
async fn refresh_reruns_init() {
    // ensure 哲学：refresh 同样跑 init（幂等是作者约定），每次安装
    // 都重新 ensure 环境就绪。
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ext.toml"),
        "[ext]\nname = \"demo\"\nversion = \"0.1.0\"\ndescription = \"t\"\ninit = \"scripts/count.sh\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    let script = dir.path().join("scripts/count.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nn=0\n[ -f \"$YOMI_DATA_DIR/init-count\" ] && n=$(cat \"$YOMI_DATA_DIR/init-count\")\necho $((n + 1)) > \"$YOMI_DATA_DIR/init-count\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;

    for _ in 0..2 {
        install(
            data.path(),
            dir.path(),
            &store,
            crate::permission::Level::Caution,
            &manifest_of(&dir),
            &provenance(),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(data.path().join("init-count"))
            .unwrap()
            .trim(),
        "2"
    );
}

#[tokio::test]
async fn refresh_updates_cron_content_but_keeps_schedule() {
    // 内容随包、时机随你：刷新把消息文本更新到包内值，用户改过的
    // schedule 不被冲掉。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();

    // 用户用 cron update 口径改时刻。
    let job = store.get_by_name("ext:demo:dream").await.unwrap().unwrap();
    store
        .update(
            &job.id,
            &crate::cron::UpdateCronJobInput {
                schedule: Some("23 23 * * *".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // 作者改消息文本后重装。
    std::fs::write(pkg.path().join("prompts/dream.txt"), "new dream text").unwrap();
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();
    assert_eq!(report.cron[0].status, CronAdoptStatus::Updated);

    let job = store.get_by_name("ext:demo:dream").await.unwrap().unwrap();
    assert_eq!(job.schedule, "23 23 * * *", "用户时刻不被冲掉");
    match &job.action {
        crate::cron::CronAction::SendMessage { content, .. } => {
            assert!(content.contains("new dream text"), "{content}");
        }
        other => panic!("unexpected action: {other:?}"),
    }
}

#[tokio::test]
async fn refresh_untouched_when_cron_identical() {
    // 内容一致就不碰 store：重装报告 Untouched，updated_at 不动。
    let pkg = write_pkg();
    let data = tempfile::tempdir().unwrap();
    let store = test_cron_store().await;
    for _ in 0..2 {
        install(
            data.path(),
            pkg.path(),
            &store,
            crate::permission::Level::Caution,
            &manifest_of(&pkg),
            &provenance(),
        )
        .await
        .unwrap();
    }
    let report = install(
        data.path(),
        pkg.path(),
        &store,
        crate::permission::Level::Caution,
        &manifest_of(&pkg),
        &provenance(),
    )
    .await
    .unwrap();
    assert_eq!(report.cron[0].status, CronAdoptStatus::Untouched);
}

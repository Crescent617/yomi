//! Tests for [`super`] (daemon singleton lock).

use super::*;
use tempfile::TempDir;

#[test]
fn same_dir_second_acquire_contends() {
    let tmp = TempDir::new().unwrap();
    let first = acquire(tmp.path()).expect("first acquire");
    let err = acquire(tmp.path()).expect_err("second acquire must contend");
    match err {
        AcquireError::Contended { owner } => {
            let owner = owner.expect("meta written by first acquire");
            assert_eq!(owner.pid, std::process::id());
        }
        AcquireError::Io(e) => panic!("expected contention, got I/O error: {e}"),
    }
    drop(first);
}

#[test]
fn drop_releases_lock() {
    let tmp = TempDir::new().unwrap();
    let first = acquire(tmp.path()).unwrap();
    drop(first);
    acquire(tmp.path()).expect("reacquire after drop");
}

#[test]
fn different_dirs_lock_independently() {
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    let _ga = acquire(a.path()).unwrap();
    acquire(b.path()).expect("different data dirs must not contend");
}

#[test]
fn stale_meta_does_not_block_after_release() {
    let tmp = TempDir::new().unwrap();
    let first = acquire(tmp.path()).unwrap();
    let lock_path = first.lock_path().to_path_buf();
    drop(first);
    // 锁文件与 meta 都还在（我们不删），但锁已释放：必须能重新获取。
    assert!(lock_path.exists());
    acquire(tmp.path()).expect("stale files must not block reacquire");
}

#[test]
fn reacquire_overwrites_meta() {
    let tmp = TempDir::new().unwrap();
    let first = acquire(tmp.path()).unwrap();
    drop(first);
    let _second = acquire(tmp.path()).expect("reacquire");
    // meta 必须反映当前持有者（pid 在同进程内相同，至少存在且可读）。
    let owner = read_owner(tmp.path()).expect("meta rewritten by second acquire");
    assert_eq!(owner.pid, std::process::id());
}

#[test]
fn lock_lives_in_system_temp_not_data_dir() {
    let tmp = TempDir::new().unwrap();
    let guard = acquire(tmp.path()).unwrap();
    let lock_path = guard.lock_path();
    // unix 固定 /tmp（$TMPDIR 会把键劈裂，见模块文档）；非 unix 是 temp_dir()。
    #[cfg(unix)]
    let expected_dir = std::path::PathBuf::from("/tmp");
    #[cfg(not(unix))]
    let expected_dir = std::env::temp_dir();
    assert_eq!(
        lock_path.parent(),
        Some(expected_dir.as_path()),
        "lock file must live in the pinned temp dir, got {lock_path:?}"
    );
    assert!(
        !lock_path.starts_with(tmp.path()),
        "lock file must not pollute the data dir: {lock_path:?}"
    );
    assert!(guard.lock_path().exists());
}

#[test]
fn symlinked_data_dir_shares_lock_key() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    // 两个路径推导出的锁文件必须是同一个（symlink 指向同一物理目录）。
    assert_eq!(lock_file_path(&real), lock_file_path(&link));

    let _guard = acquire(&real).unwrap();
    // 竞争方 / GUI 回退用未规范化的路径读 meta 也必须读得到。
    let err = acquire(&link).expect_err("symlink alias must contend");
    match err {
        AcquireError::Contended { owner } => {
            assert!(owner.is_some(), "meta readable via symlinked path");
        }
        AcquireError::Io(e) => panic!("expected contention, got I/O error: {e}"),
    }
}

#[test]
fn fnv1a_64_known_vectors() {
    // FNV-1a 64 公开测试向量（http://www.isthe.com/chongo/src/fnv/hash_64a.c），
    // 锁键的持久性依赖这个哈希不漂移，静默改动必须被这里抓住。
    assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn lock_key_stable_before_and_after_dir_created() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("a").join("b");
    // 目录还不存在（`daemon lock-path` 对未初始化 data_dir 的场景）。
    let before = lock_file_path(&nested);
    std::fs::create_dir_all(&nested).unwrap();
    let after = lock_file_path(&nested);
    assert_eq!(before, after, "lock key must not depend on dir existence");
}

#[test]
fn legacy_lock_blocks_acquire_and_is_held() {
    // 升级窗口：legacy 锁文件被"旧版 daemon"（此处是测试进程里的另一
    // 把 flock）持有时，acquire 必须 Contended，且 tmp 锁不能泄漏。
    let tmp = TempDir::new().unwrap();
    let legacy = tmp.path().join("daemon.lock");
    let foreign = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&legacy)
        .unwrap();
    let foreign = nix::fcntl::Flock::lock(foreign, nix::fcntl::FlockArg::LockExclusive).unwrap();

    let err = acquire(tmp.path()).expect_err("locked legacy file must contend");
    match err {
        AcquireError::Contended { .. } => {}
        AcquireError::Io(e) => panic!("expected contention, got I/O error: {e}"),
    }
    drop(foreign);
    // tmp 锁没泄漏：旧锁释放后立刻能拿到。
    let _guard = acquire(tmp.path()).expect("acquire after legacy released");
    // guard 持有期间 legacy 文件必须被 flock——旧版竞争者只看这把锁。
    let f2 = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&legacy)
        .unwrap();
    let again = nix::fcntl::Flock::lock(f2, nix::fcntl::FlockArg::LockExclusiveNonblock);
    assert!(
        again.is_err(),
        "guard must hold the legacy flock for old-version contenders"
    );
}

#[test]
fn preplanted_symlink_at_lock_path_is_rejected() {
    // /tmp 全机可写且锁文件名可预测：攻击者预置 symlink 诱导 create()
    // 跟随打开。O_NOFOLLOW 必须让获取硬失败，且不碰链接目标。
    let tmp = TempDir::new().unwrap();
    let lock_path = lock_file_path(tmp.path());
    let victim = tmp.path().join("victim");
    std::fs::write(&victim, "precious").unwrap();
    std::os::unix::fs::symlink(&victim, &lock_path).unwrap();

    let err = acquire(tmp.path()).expect_err("symlinked lock path must not be followed");
    assert!(
        matches!(err, AcquireError::Io(_)),
        "expected I/O error (ELOOP), got {err:?}"
    );
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
    let _ = std::fs::remove_file(&lock_path);
}

#[test]
fn preplanted_symlink_at_meta_path_is_not_clobbered() {
    // meta 是截断写，被预置 symlink 命中会清掉目标文件。best-effort
    // 路径可以写不进 meta，但绝不能写穿链接。
    let tmp = TempDir::new().unwrap();
    let meta_path = meta_file_path(tmp.path());
    let victim = tmp.path().join("victim");
    std::fs::write(&victim, "precious").unwrap();
    std::os::unix::fs::symlink(&victim, &meta_path).unwrap();

    let _guard = acquire(tmp.path()).expect("lock acquire despite meta symlink");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
    let _ = std::fs::remove_file(&meta_path);
}

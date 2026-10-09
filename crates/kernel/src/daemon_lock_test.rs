//! Tests for [`super`] (daemon singleton lock).

use super::*;
use tempfile::TempDir;

#[test]
fn lock_lives_under_run_subdir() {
    let tmp = TempDir::new().unwrap();
    let guard = acquire(tmp.path()).unwrap();
    // 锁路径走 canonicalize（macOS 上 tmp 在 /var → /private/var），
    // 断言也走 canonicalize 对齐。
    let expect = std::fs::canonicalize(tmp.path())
        .unwrap()
        .join("run")
        .join("daemon.lock");
    assert_eq!(guard.lock_path(), expect.as_path());
    // run/ 由 acquire 自动创建。
    assert!(expect.parent().is_some_and(|p| p.is_dir()));
}

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
fn symlinked_data_dir_resolves_to_same_lock() {
    // 调用方有的传配置原文、有的传规范化路径（build_kernel）——两种
    // 拼法必须指向同一把锁，否则 GUI 回退读不到 meta、lock-path 打印
    // 的是 daemon 没在用的路径。
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    assert_eq!(lock_file_path(&real), lock_file_path(&link));
    assert_eq!(meta_file_path(&real), meta_file_path(&link));

    let _guard = acquire(&real).unwrap();
    let err = acquire(&link).expect_err("symlink alias must contend");
    match err {
        AcquireError::Contended { owner } => {
            assert!(owner.is_some(), "meta readable via symlinked path");
        }
        AcquireError::Io(e) => panic!("expected contention, got I/O error: {e}"),
    }
}

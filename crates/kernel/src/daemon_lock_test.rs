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

use super::*;

#[tokio::test]
async fn test_resolve_safe_path_normal() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    std::fs::write(base.join("hello.txt"), "hi").unwrap();
    let sub = base.join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("nested.txt"), "nested").unwrap();

    assert!(resolve_safe_path(base, "hello.txt").await.is_some());
    assert!(resolve_safe_path(base, "sub/nested.txt").await.is_some());
}

#[tokio::test]
async fn test_resolve_safe_path_rejects_absolute() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    assert!(resolve_safe_path(base, "/etc/passwd").await.is_none());
}

#[tokio::test]
async fn test_resolve_safe_path_rejects_dotdot() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    assert!(resolve_safe_path(base, "../secrets.txt").await.is_none());
    assert!(resolve_safe_path(base, "foo/../../secrets.txt")
        .await
        .is_none());
}

#[tokio::test]
async fn test_resolve_safe_path_nonexistent() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let resolved = resolve_safe_path(base, "does_not_exist.txt").await;
    assert!(resolved.is_some());
    assert_eq!(
        resolved.unwrap().file_name(),
        Some(std::ffi::OsStr::new("does_not_exist.txt"))
    );
}

#[tokio::test]
async fn test_resolve_safe_path_traversal_via_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.txt"), "secret").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let _ = symlink(outside.path().join("outside.txt"), base.join("link.txt"));
        let resolved = resolve_safe_path(base, "link.txt").await;
        // Symlink pointing outside base should be rejected by canonicalize check
        assert!(resolved.is_none());
    }
}

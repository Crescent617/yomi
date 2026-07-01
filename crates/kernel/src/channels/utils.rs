//! Shared utilities for platform adapters.

/// Maximum retry delay for platform connection failures (shared across adapters).
#[allow(clippy::duration_suboptimal_units)]
pub(crate) const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(300);

/// Resolve a relative path under `base`, rejecting path-traversal attempts.
///
/// Rejects absolute paths, `..` components, and paths that escape `base`.
/// Uses `tokio::fs::canonicalize` for async-safe symlink resolution.
pub async fn resolve_safe_path(base: &std::path::Path, path: &str) -> Option<std::path::PathBuf> {
    // Reject absolute paths and paths containing .. components.
    let path_obj = std::path::Path::new(path);
    if path_obj.is_absolute() {
        return None;
    }
    for comp in path_obj.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return None;
        }
    }
    let joined = base.join(path);
    match tokio::fs::canonicalize(&joined).await {
        Ok(canonical) => {
            let base_canonical = tokio::fs::canonicalize(base).await.ok()?;
            if canonical.starts_with(&base_canonical) {
                Some(canonical)
            } else {
                None
            }
        }
        Err(_) => {
            // File may not exist yet; verify logically within base.
            let base_canonical = tokio::fs::canonicalize(base).await.ok()?;
            let joined = base_canonical.join(path);
            if joined.starts_with(&base_canonical) {
                Some(joined)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}

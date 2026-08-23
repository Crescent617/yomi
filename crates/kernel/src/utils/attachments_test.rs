use super::*;

// ── parse_attachments ────────────────────────────────────────────────

#[test]
fn no_block_returns_text_unchanged() {
    let text = "  hello <yomi_attachments> world\n";
    let (cleaned, paths) = parse_attachments(text);
    assert_eq!(cleaned, text);
    assert!(paths.is_empty());
}

#[test]
fn trailing_block_is_stripped() {
    let (cleaned, paths) = parse_attachments(
        "report done\n\n<yomi_attachments>\nout.pdf\n data.csv \n</yomi_attachments>\n",
    );
    assert_eq!(cleaned, "report done");
    assert_eq!(paths, vec!["out.pdf", "data.csv"]);
}

#[test]
fn block_only_leaves_empty_text() {
    let (cleaned, paths) = parse_attachments("<yomi_attachments>\na.pdf\n</yomi_attachments>");
    assert_eq!(cleaned, "");
    assert_eq!(paths, vec!["a.pdf"]);
}

#[test]
fn mid_text_block_is_recognized() {
    // Position no longer matters — only fence parity does.
    let text = "before <yomi_attachments>a.pdf</yomi_attachments> after";
    let (cleaned, paths) = parse_attachments(text);
    assert_eq!(cleaned, "before  after");
    assert_eq!(paths, vec!["a.pdf"]);
}

#[test]
fn declaration_with_trailing_outro() {
    let (cleaned, paths) =
        parse_attachments("done\n<yomi_attachments>\nout.pdf\n</yomi_attachments>\n附件如上");
    assert_eq!(cleaned, "done\n\n附件如上");
    assert_eq!(paths, vec!["out.pdf"]);
}

#[test]
fn declaration_followed_by_fenced_code() {
    // Balanced fences after the block keep the parity even.
    let (cleaned, paths) =
        parse_attachments("<yomi_attachments>\nout.pdf\n</yomi_attachments>\n```\ncode\n```");
    assert_eq!(cleaned, "```\ncode\n```");
    assert_eq!(paths, vec!["out.pdf"]);
}

#[test]
fn multiple_blocks_merge_in_order() {
    let (cleaned, paths) = parse_attachments(
        "<yomi_attachments>a.pdf</yomi_attachments>\n<yomi_attachments>b.pdf</yomi_attachments>\n",
    );
    assert_eq!(cleaned, "");
    assert_eq!(paths, vec!["a.pdf", "b.pdf"]);
}

#[test]
fn fenced_example_surfaces_as_typed() {
    // The supported way to show the syntax: the enclosing fence closes
    // after the block, leaving an odd fence count behind it.
    let text = "use this syntax:\n```\n<yomi_attachments>\nout.pdf\n</yomi_attachments>\n```";
    let (cleaned, paths) = parse_attachments(text);
    assert_eq!(cleaned, text);
    assert!(paths.is_empty());
}

#[test]
fn fenced_example_then_real_declaration() {
    // The example stays verbatim; the real declaration is collected.
    let text = "```\n<yomi_attachments>\nexample.pdf\n</yomi_attachments>\n```\n<yomi_attachments>\nreal.pdf\n</yomi_attachments>";
    let (cleaned, paths) = parse_attachments(text);
    assert_eq!(
        cleaned,
        "```\n<yomi_attachments>\nexample.pdf\n</yomi_attachments>\n```"
    );
    assert_eq!(paths, vec!["real.pdf"]);
}

#[test]
fn unterminated_block_is_left_untouched() {
    let text = "done\n<yomi_attachments>\na.pdf";
    let (cleaned, paths) = parse_attachments(text);
    assert_eq!(cleaned, text);
    assert!(paths.is_empty());
}

#[test]
fn empty_block_is_stripped_without_paths() {
    let (cleaned, paths) = parse_attachments("done\n<yomi_attachments>\n</yomi_attachments>");
    assert_eq!(cleaned, "done");
    assert!(paths.is_empty());
}

// ── resolve_safe_path ────────────────────────────────────────────────

#[tokio::test]
async fn safe_path_normal() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap();
    std::fs::write(dir.path().join("hello.txt"), b"hi").unwrap();
    assert!(resolve_safe_path(base.as_ref(), "hello.txt")
        .await
        .is_some());
    assert!(resolve_safe_path(base.as_ref(), "sub/nested.txt")
        .await
        .is_some());
}

#[tokio::test]
async fn safe_path_rejects_absolute() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap();
    assert!(resolve_safe_path(base.as_ref(), "/etc/passwd")
        .await
        .is_none());
}

#[tokio::test]
async fn safe_path_rejects_dotdot() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap();
    assert!(resolve_safe_path(base.as_ref(), "../secrets.txt")
        .await
        .is_none());
    assert!(resolve_safe_path(base.as_ref(), "foo/../../secrets.txt")
        .await
        .is_none());
}

#[tokio::test]
async fn safe_path_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap();
    let resolved = resolve_safe_path(base.as_ref(), "does_not_exist.txt").await;
    // Non-existent but logically inside base → allowed.
    assert!(resolved.is_some());
}

#[tokio::test]
async fn safe_path_traversal_via_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, b"secret").unwrap();
    std::os::unix::fs::symlink(&secret, base.join("link.txt")).unwrap();
    let resolved = resolve_safe_path(base, "link.txt").await;
    // Symlink escapes base → rejected.
    assert!(resolved.is_none());
}

// ── resolve_attachment ───────────────────────────────────────────────

#[tokio::test]
async fn resolves_relative_path_under_base() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.pdf"), b"x").unwrap();
    let resolved = resolve_attachment(Some(dir.path()), "a.pdf").await;
    assert!(resolved.is_some());
}

#[tokio::test]
async fn resolves_absolute_path_without_base() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.pdf");
    std::fs::write(&file, b"x").unwrap();
    let resolved = resolve_attachment(None, file.to_str().unwrap()).await;
    assert_eq!(resolved, tokio::fs::canonicalize(&file).await.ok());
}

#[tokio::test]
async fn rejects_traversal_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(resolve_attachment(Some(dir.path()), "../outside.txt")
        .await
        .is_none());
    assert!(resolve_attachment(Some(dir.path()), "missing.txt")
        .await
        .is_none());
    assert!(resolve_attachment(None, "/definitely/missing/file.txt")
        .await
        .is_none());
}

#[tokio::test]
async fn rejects_relative_without_base_and_directories() {
    assert!(resolve_attachment(None, "a.pdf").await.is_none());
    let dir = tempfile::tempdir().unwrap();
    // A directory is not a deliverable file.
    assert!(resolve_attachment(None, dir.path().to_str().unwrap())
        .await
        .is_none());
}

#[test]
fn block_before_unterminated_fence_is_stripped() {
    // 截断回复（fence 未闭合）不改变块的位置事实：块在 fence 之外，正常交付。
    let text = "<yomi_attachments>\nout.pdf\n</yomi_attachments>\n```\n truncated";
    let (cleaned, paths) = parse_attachments(text);
    assert_eq!(cleaned, "```\n truncated");
    assert_eq!(paths, vec!["out.pdf"]);
}

#[test]
fn block_inside_unterminated_fence_is_kept() {
    // fence 开了没合，块一直在 fence 里 → 是示例不是声明。
    let text = "```\n<yomi_attachments>\nout.pdf\n</yomi_attachments>";
    let (cleaned, paths) = parse_attachments(text);
    assert_eq!(cleaned, text);
    assert!(paths.is_empty());
}

// ── resolve_attachment_with_default_workspace ──────────────────────────

#[tokio::test]
async fn default_workspace_fallback_resolves_relative_when_base_missing() {
    let data = tempfile::tempdir().unwrap();
    let ws = data.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("a.pdf"), b"x").unwrap();

    let resolved = resolve_attachment_with_default_workspace(data.path(), None, "a.pdf").await;

    assert_eq!(resolved, Some(ws.join("a.pdf").canonicalize().unwrap()));
}

#[tokio::test]
async fn default_workspace_fallback_explicit_base_wins() {
    let data = tempfile::tempdir().unwrap(); // 回落目标（不含文件）
    let base = tempfile::tempdir().unwrap(); // 显式 base（含文件）
    std::fs::write(base.path().join("a.pdf"), b"x").unwrap();

    let resolved =
        resolve_attachment_with_default_workspace(data.path(), Some(base.path()), "a.pdf").await;

    assert_eq!(
        resolved,
        Some(base.path().join("a.pdf").canonicalize().unwrap())
    );
}

#[tokio::test]
async fn default_workspace_fallback_missing_file_returns_none() {
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data.path().join("workspace")).unwrap();

    assert!(
        resolve_attachment_with_default_workspace(data.path(), None, "missing.pdf")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn default_workspace_fallback_empty_base_dir_also_falls_back() {
    let data = tempfile::tempdir().unwrap();
    let ws = data.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("a.pdf"), b"x").unwrap();

    let resolved =
        resolve_attachment_with_default_workspace(data.path(), Some(Path::new("")), "a.pdf").await;

    assert_eq!(resolved, Some(ws.join("a.pdf").canonicalize().unwrap()));
}

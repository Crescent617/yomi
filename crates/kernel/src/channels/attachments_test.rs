use super::*;
use crate::channels::{ChannelError, ChannelMessage};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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

// ── resolve_attachments / send_attachments ───────────────────────────

#[derive(Default)]
struct MockAdapter {
    sent_files: Mutex<Vec<Vec<PathBuf>>>,
    outgoing: Mutex<Vec<String>>,
    fail_send: bool,
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelMessage>,
        _cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        std::future::pending().await
    }

    async fn send_message(
        &self,
        _external_chat_id: &str,
        blocks: Vec<crate::types::ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        let text = blocks
            .iter()
            .map(|b| match b {
                crate::types::ContentBlock::Text { text } => text.clone(),
                _ => String::new(),
            })
            .collect::<String>();
        self.outgoing.lock().unwrap().push(text);
        Ok(None)
    }

    async fn send_files(
        &self,
        _external_chat_id: &str,
        files: &[(&Path, Option<&str>)],
        _reply_msg_id: Option<&str>,
    ) -> Result<(), ChannelError> {
        if self.fail_send {
            return Err(ChannelError::Platform("boom".into()));
        }
        self.sent_files
            .lock()
            .unwrap()
            .push(files.iter().map(|(p, _)| p.to_path_buf()).collect());
        Ok(())
    }
}

fn routing() -> SessionRouting {
    SessionRouting {
        channel_name: "test".to_string(),
        external_chat_id: "chat1".to_string(),
        reply_msg_id: None,
    }
}

/// Build a reply with a pre-set attachments list (bypassing the parser,
/// which is covered above).
fn reply_with_attachments(text: Option<&str>, attachments: &[&str]) -> FinalReply {
    let mut reply = crate::channels::reply::RunReplyBuffer::new().into_reply();
    if let Some(text) = text {
        reply.push_note(text);
    }
    reply.set_attachments(attachments.iter().map(|s| s.to_string()).collect());
    reply
}

#[tokio::test]
async fn resolve_dedupes_and_notes_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.pdf"), b"x").unwrap();
    let mut reply =
        reply_with_attachments(Some("see attached"), &["a.pdf", "a.pdf", "missing.pdf"]);

    let files = resolve_attachments(Some(dir.path()), &mut reply).await;

    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("a.pdf"));
    // Missing file surfaced as a note on the reply text; list consumed.
    let text = reply.text().unwrap();
    assert!(text.starts_with("see attached"));
    assert!(text.contains("missing.pdf"));
    assert!(reply.attachments().is_empty());
}

#[tokio::test]
async fn send_delivers_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.pdf");
    std::fs::write(&file, b"x").unwrap();
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    send_attachments(&adapter, &routing(), vec![file.clone()]).await;

    let sent = mock.sent_files.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0], vec![file]);
    assert!(mock.outgoing.lock().unwrap().is_empty());
}

#[tokio::test]
async fn send_failure_sends_follow_up_message() {
    let mock = Arc::new(MockAdapter {
        fail_send: true,
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    send_attachments(&adapter, &routing(), vec![PathBuf::from("/tmp/a.pdf")]).await;

    let outgoing = mock.outgoing.lock().unwrap();
    assert_eq!(outgoing.len(), 1);
    assert!(outgoing[0].contains("boom"));
}

#[tokio::test]
async fn empty_files_is_noop() {
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    send_attachments(&adapter, &routing(), Vec::new()).await;
    assert!(mock.sent_files.lock().unwrap().is_empty());
    assert!(mock.outgoing.lock().unwrap().is_empty());
}

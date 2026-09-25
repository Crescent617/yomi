use super::*;
use crate::channels::{ChannelError, ChannelEvent};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MockAdapter {
    sent_files: Mutex<Vec<Vec<PathBuf>>>,
    outgoing: Mutex<Vec<String>>,
    fail_send: bool,
    /// Mirrors the real adapters' `upload_image` contract: `Ok(None)`
    /// when the platform has no inline support (`inline_upload` off) or
    /// the file is not an image; `Err` on upload failure.
    inline_upload: bool,
    fail_upload: bool,
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
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
        files: &[(&std::path::Path, Option<&str>)],
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

    async fn upload_image(&self, path: &std::path::Path) -> Result<Option<String>, ChannelError> {
        if self.fail_upload {
            return Err(ChannelError::Platform("boom".into()));
        }
        if !self.inline_upload || path.extension().and_then(|e| e.to_str()) != Some("png") {
            return Ok(None);
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("img")
            .to_string();
        Ok(Some(format!("key-{name}")))
    }
}

fn routing() -> SessionRouting {
    SessionRouting {
        channel_name: "test".to_string(),
        external_chat_id: "chat1".to_string(),
        reply_msg_id: None,
        mapping_key: "chat1".to_string(),
        doc_comment: None,
        kind: crate::channels::MappingKind::Normal,
    }
}

/// Build a reply with a pre-set attachments list (the parser itself is
/// covered in `crate::utils::attachments::tests`).
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
    assert_eq!(files[0].0, "a.pdf");
    assert!(files[0].1.ends_with("a.pdf"));
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

#[tokio::test]
async fn inline_partition_for_reply_uploads_images_and_keeps_the_rest() {
    let mock = Arc::new(MockAdapter {
        inline_upload: true,
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut reply = reply_with_attachments(Some("see attached"), &[]);
    let files = vec![
        ("a.png".to_string(), PathBuf::from("/tmp/a.png")),
        ("b.pdf".to_string(), PathBuf::from("/tmp/b.pdf")),
        ("c.png".to_string(), PathBuf::from("/tmp/c.png")),
    ];

    let rest = inline_partition_for_reply(&adapter, &mut reply, files).await;

    // Non-images stay on the post-reply file path, order preserved.
    assert_eq!(rest, vec![PathBuf::from("/tmp/b.pdf")]);
    let inline = reply.inline_images();
    assert_eq!(inline.len(), 2);
    assert_eq!(inline[0].key, "key-a.png");
    assert_eq!(inline[0].alt, "a.png");
    assert_eq!(inline[0].path, PathBuf::from("/tmp/a.png"));
    assert_eq!(inline[0].declared, "a.png");
    assert_eq!(inline[1].key, "key-c.png");
    assert_eq!(inline[1].declared, "c.png");
}

#[tokio::test]
async fn inline_partition_for_reply_upload_failure_keeps_the_file() {
    let mock = Arc::new(MockAdapter {
        inline_upload: true,
        fail_upload: true,
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut reply = reply_with_attachments(Some("see attached"), &[]);

    let rest = inline_partition_for_reply(
        &adapter,
        &mut reply,
        vec![("a.png".to_string(), PathBuf::from("/tmp/a.png"))],
    )
    .await;

    assert_eq!(rest, vec![PathBuf::from("/tmp/a.png")]);
    assert!(reply.inline_images().is_empty());
}

#[tokio::test]
async fn inline_partition_for_reply_without_platform_support_is_a_noop() {
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut reply = reply_with_attachments(Some("see attached"), &[]);

    let rest = inline_partition_for_reply(
        &adapter,
        &mut reply,
        vec![("a.png".to_string(), PathBuf::from("/tmp/a.png"))],
    )
    .await;

    assert_eq!(rest, vec![PathBuf::from("/tmp/a.png")]);
    assert!(reply.inline_images().is_empty());
}

#[tokio::test]
async fn inline_partition_for_reply_textless_reply_keeps_everything() {
    let mock = Arc::new(MockAdapter {
        inline_upload: true,
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut reply = reply_with_attachments(None, &[]);

    let rest = inline_partition_for_reply(
        &adapter,
        &mut reply,
        vec![("a.png".to_string(), PathBuf::from("/tmp/a.png"))],
    )
    .await;

    // No body text — no card to ride: the file list passes through.
    assert_eq!(rest, vec![PathBuf::from("/tmp/a.png")]);
    assert!(reply.inline_images().is_empty());
}

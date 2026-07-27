use super::*;
use crate::channels::{ChannelError, ChannelMessage};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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
}

fn routing() -> SessionRouting {
    SessionRouting {
        channel_name: "test".to_string(),
        external_chat_id: "chat1".to_string(),
        reply_msg_id: None,
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

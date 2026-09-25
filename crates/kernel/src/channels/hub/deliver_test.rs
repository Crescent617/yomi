use super::*;
use crate::channels::reply::InlineImage;
use crate::channels::{ChannelError, ChannelEvent, PlatformAdapter, SessionRouting};
use std::path::PathBuf;
// deliver.rs pulls the kernel-wide `types::Result` alias into the glob —
// the trait signatures here need the std two-parameter form.
use std::result::Result;
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MockAdapter {
    cards: Mutex<Vec<String>>,
    texts: Mutex<Vec<String>>,
    files: Mutex<Vec<Vec<PathBuf>>>,
    card_support: bool,
    fail_files: bool,
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

    fn supports_status_card(&self) -> bool {
        self.card_support
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
        self.texts.lock().unwrap().push(text);
        Ok(Some("m1".to_string()))
    }

    async fn send_card(
        &self,
        _external_chat_id: &str,
        card_json: &str,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        self.cards.lock().unwrap().push(card_json.to_string());
        Ok(Some("c1".to_string()))
    }

    async fn send_files(
        &self,
        _external_chat_id: &str,
        files: &[(&std::path::Path, Option<&str>)],
        _reply_msg_id: Option<&str>,
    ) -> Result<(), ChannelError> {
        if self.fail_files {
            return Err(ChannelError::Platform("disk full".into()));
        }
        self.files
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
        mapping_key: "chat1".to_string(),
        doc_comment: None,
        kind: crate::channels::MappingKind::Normal,
    }
}

fn reply_with_inline_image() -> reply::FinalReply {
    let mut reply = reply::RunReplyBuffer::new().into_reply();
    reply.push_note("body");
    reply.set_inline_images(vec![InlineImage {
        key: "img_k1".to_string(),
        alt: "a.png".to_string(),
        path: PathBuf::from("/tmp/a.png"),
    }]);
    reply
}

#[tokio::test]
async fn flush_card_path_renders_inline_images_even_without_trace() {
    let mock = Arc::new(MockAdapter {
        card_support: true,
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    // tool_trace off: the inline image alone still drives the card path.
    let msg_id = flush_reply(&adapter, &routing(), reply_with_inline_image(), false).await;

    assert_eq!(msg_id.as_deref(), Some("c1"));
    let cards = mock.cards.lock().unwrap();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].contains("img_k1"));
    assert!(mock.files.lock().unwrap().is_empty());
}

#[tokio::test]
async fn flush_plain_path_delivers_inline_images_as_files() {
    let mock = Arc::new(MockAdapter::default()); // no card support
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    let msg_id = flush_reply(&adapter, &routing(), reply_with_inline_image(), false).await;

    assert_eq!(msg_id.as_deref(), Some("m1"));
    assert_eq!(mock.texts.lock().unwrap().as_slice(), ["body"]);
    let files = mock.files.lock().unwrap();
    assert_eq!(files.as_slice(), [vec![PathBuf::from("/tmp/a.png")]]);
    assert!(mock.cards.lock().unwrap().is_empty());
}

#[tokio::test]
async fn flush_plain_path_inline_image_file_failure_surfaces_a_note() {
    let mock = Arc::new(MockAdapter {
        fail_files: true,
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    let msg_id = flush_reply(&adapter, &routing(), reply_with_inline_image(), false).await;

    assert_eq!(msg_id.as_deref(), Some("m1"));
    // The delivery failure rides a follow-up message — never silent.
    let texts = mock.texts.lock().unwrap();
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0], "body");
    assert!(texts[1].contains("disk full"), "note: {}", texts[1]);
}

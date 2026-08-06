use super::*;

use crate::channels::{ChannelEvent, DocCommentReplyLite};
use tokio_util::sync::CancellationToken;

// ── Mock adapter ───────────────────────────────────────────────────

struct CommentMockAdapter {
    detail: tokio::sync::Mutex<Option<DocCommentDetail>>,
    fetch_error: tokio::sync::Mutex<bool>,
    /// `fetch_doc_comment` call count — the disabled-toggle test asserts
    /// the feature costs zero platform API calls when off.
    fetch_calls: tokio::sync::Mutex<usize>,
    title: Option<String>,
}

impl CommentMockAdapter {
    fn new(detail: Option<DocCommentDetail>) -> Self {
        Self {
            detail: tokio::sync::Mutex::new(detail),
            fetch_error: tokio::sync::Mutex::new(false),
            fetch_calls: tokio::sync::Mutex::new(0),
            title: Some("2026 产品方案".to_string()),
        }
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for CommentMockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        _external_chat_id: &str,
        _blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        Ok(None)
    }

    async fn fetch_doc_title(&self, _file_token: &str, _file_type: &str) -> Option<String> {
        self.title.clone()
    }

    async fn fetch_doc_comment(
        &self,
        _file_token: &str,
        _file_type: &str,
        _comment_id: &str,
    ) -> Result<Option<DocCommentDetail>, ChannelError> {
        if *self.fetch_error.lock().await {
            return Err(ChannelError::Platform("mock: comment fetch failed".into()));
        }
        *self.fetch_calls.lock().await += 1;
        Ok(self.detail.lock().await.clone())
    }
}

fn detail_with_replies(replies: Vec<(&str, &str)>) -> DocCommentDetail {
    DocCommentDetail {
        is_whole: false,
        quote: Some("被划词引用的原文段落".to_string()),
        replies: replies
            .into_iter()
            .enumerate()
            .map(|(i, (reply_id, text))| DocCommentReplyLite {
                reply_id: reply_id.to_string(),
                user_id: "ou_commenter".to_string(),
                create_time: 1_700_000_000 + i as i64,
                text: text.to_string(),
            })
            .collect(),
    }
}

fn notice() -> DocCommentNotice {
    DocCommentNotice {
        file_token: "doxcnABC123".to_string(),
        file_type: "docx".to_string(),
        comment_id: "7123456789".to_string(),
        reply_id: Some("r_2".to_string()),
        commenter_open_id: "ou_commenter".to_string(),
        is_mentioned: true,
        notice_type: "add_comment".to_string(),
        create_time: Some(1_700_000_000_000),
    }
}

fn feishu_config() -> ChannelConfig {
    ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        platform: super::super::PlatformConfig::Feishu {
            app_id: "app".to_string(),
            app_secret: "secret".to_string(),
        },
        ..Default::default()
    }
}

async fn handle(
    config: &ChannelConfig,
    adapter: &Arc<CommentMockAdapter>,
    notice: DocCommentNotice,
) -> mpsc::Receiver<(ChannelMessage, super::super::hub::Gate)> {
    let (tx, rx) = mpsc::channel(1);
    let adapter: Arc<dyn PlatformAdapter> = adapter.clone();
    handle_doc_comment_added("feishu", config, &adapter, &tx, notice).await;
    rx
}

// ── Policy ─────────────────────────────────────────────────────────

#[tokio::test]
async fn disabled_event_never_fetches_or_dispatches() {
    let mut config = feishu_config();
    config.disabled_events = vec!["doc_comment".to_string()];
    let adapter = Arc::new(CommentMockAdapter::new(Some(detail_with_replies(vec![(
        "r_1", "hi",
    )]))));
    let mut rx = handle(&config, &adapter, notice()).await;
    assert!(rx.try_recv().is_err(), "disabled feature must not dispatch");
    assert_eq!(
        *adapter.fetch_calls.lock().await,
        0,
        "disabled feature must cost zero platform API calls"
    );
}

#[tokio::test]
async fn non_add_notice_types_are_dropped() {
    let config = feishu_config();
    let adapter = Arc::new(CommentMockAdapter::new(Some(detail_with_replies(vec![(
        "r_1", "hi",
    )]))));
    for notice_type in ["resolve_comment", "delete_comment", ""] {
        let mut n = notice();
        n.notice_type = notice_type.to_string();
        let mut rx = handle(&config, &adapter, n).await;
        assert!(rx.try_recv().is_err(), "{notice_type} must be dropped");
    }
}

#[tokio::test]
async fn non_mentioned_comment_is_dropped() {
    let config = feishu_config();
    let adapter = Arc::new(CommentMockAdapter::new(Some(detail_with_replies(vec![(
        "r_1", "hi",
    )]))));
    let mut n = notice();
    n.is_mentioned = false;
    let mut rx = handle(&config, &adapter, n).await;
    assert!(rx.try_recv().is_err(), "non-mentioned must be dropped");
}

#[tokio::test]
async fn blocked_and_non_allowlisted_commenters_are_dropped() {
    let mut config = feishu_config();
    let adapter = Arc::new(CommentMockAdapter::new(Some(detail_with_replies(vec![(
        "r_1", "hi",
    )]))));

    config.blocked_users = vec!["ou_commenter".to_string()];
    let mut rx = handle(&config, &adapter, notice()).await;
    assert!(rx.try_recv().is_err(), "blocked user must be dropped");

    config.blocked_users.clear();
    config.allowed_users = vec!["ou_someone_else".to_string()];
    let mut rx = handle(&config, &adapter, notice()).await;
    assert!(
        rx.try_recv().is_err(),
        "non-allowlisted user must be dropped"
    );

    config.allowed_users = vec!["ou_commenter".to_string()];
    let mut rx = handle(&config, &adapter, notice()).await;
    assert!(rx.try_recv().is_ok(), "allowlisted user must pass");
}

#[tokio::test]
async fn deleted_comment_triggers_nothing() {
    let config = feishu_config();
    let adapter = Arc::new(CommentMockAdapter::new(None));
    let mut rx = handle(&config, &adapter, notice()).await;
    assert!(rx.try_recv().is_err(), "deleted comment must be dropped");
}

// ── Accepted trigger ───────────────────────────────────────────────

#[tokio::test]
async fn accepted_comment_builds_meta_message() {
    let config = feishu_config();
    let adapter = Arc::new(CommentMockAdapter::new(Some(detail_with_replies(vec![
        ("r_1", "第一条回复"),
        ("r_2", "@bot 这段改写一下"),
    ]))));
    let mut rx = handle(&config, &adapter, notice()).await;
    let (msg, _gate) = rx.try_recv().expect("trigger dispatched");

    assert!(msg.is_mention);
    assert_eq!(msg.external_user_id, "ou_commenter");
    assert!(msg.external_chat_id.is_empty());
    assert!(!msg.is_group);
    // raw_text feeds the session title: the bare comment text, @bot stripped.
    assert_eq!(msg.raw_text.as_deref(), Some("这段改写一下"));
    let dc = msg.doc_comment.as_ref().expect("doc_comment set");
    assert_eq!(dc.file_token, "doxcnABC123");
    assert_eq!(dc.comment_id, "7123456789");

    let ContentBlock::Text { text } = &msg.content[0] else {
        panic!("expected text block");
    };
    assert!(
        text.contains("[platform: feishu][source: doc_comment]"),
        "{text}"
    );
    assert!(text.contains("[doc: docx:doxcnABC123]"), "{text}");
    assert!(
        text.contains("[doc_url: https://feishu.cn/docx/doxcnABC123]"),
        "{text}"
    );
    assert!(text.contains("[comment_id: 7123456789]"), "{text}");
    assert!(text.contains("[reply_id: r_2]"), "{text}");
    assert!(text.contains("[doc_title: 2026 产品方案]"), "{text}");
    assert!(text.contains("[from_user_id: ou_commenter]"), "{text}");
    // Partial comment: the quote rides as a quote line.
    assert!(text.contains("> 被划词引用的原文段落"), "{text}");
    // The triggering reply is r_2's, not the thread's last by default.
    assert!(text.ends_with("@bot 这段改写一下"), "{text}");
}

#[tokio::test]
async fn fetch_failure_injects_bare_meta_with_note() {
    let config = feishu_config();
    let adapter = Arc::new(CommentMockAdapter::new(None));
    *adapter.fetch_error.lock().await = true;
    let mut rx = handle(&config, &adapter, notice()).await;
    let (msg, _gate) = rx.try_recv().expect("bare trigger dispatched");

    assert!(msg.raw_text.is_none(), "no text → no title input");
    let ContentBlock::Text { text } = &msg.content[0] else {
        panic!("expected text block");
    };
    assert!(text.contains("[source: doc_comment]"), "{text}");
    assert!(text.contains("[评论内容拉取失败:"), "{text}");
    assert!(!text.contains('>'), "{text}");
}

// ── Assembly parts ─────────────────────────────────────────────────

#[test]
fn pick_triggering_reply_prefers_reply_id_over_latest() {
    let detail = detail_with_replies(vec![("r_1", "first"), ("r_2", "second"), ("r_3", "third")]);
    assert_eq!(pick_triggering_reply(&detail, Some("r_2")), "second");
    assert_eq!(pick_triggering_reply(&detail, Some("r_missing")), "third");
    assert_eq!(pick_triggering_reply(&detail, None), "third");
}

#[test]
fn assemble_message_without_quote_or_title() {
    let mut n = notice();
    n.reply_id = None;
    let text = assemble_message(&n, None, "hello", None, None);
    assert!(!text.contains("[doc_title:"), "{text}");
    assert!(!text.contains("[reply_id:"), "{text}");
    assert!(!text.contains('>'), "{text}");
    assert!(text.ends_with("hello"), "{text}");
}

// ── Chunking ───────────────────────────────────────────────────────

#[test]
fn chunk_text_splits_on_char_boundaries() {
    let text = "字".repeat(4001);
    let chunks = chunk_text(&text, COMMENT_REPLY_CHUNK_CHARS);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].chars().count(), 4000);
    assert_eq!(chunks[1].chars().count(), 1);
}

#[test]
fn chunk_text_prefers_newline_breaks() {
    let line = "a".repeat(100);
    let text = format!("{line}\n{line}\n{line}");
    let chunks = chunk_text(&text, 205);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0], format!("{line}\n{line}\n"));
    assert_eq!(chunks[1], line);
}

#[test]
fn chunk_text_edge_cases() {
    assert!(chunk_text("", 10).is_empty());
    assert_eq!(chunk_text("short", 10), vec!["short".to_string()]);
    // Exactly at the cap: one chunk.
    let exact = "x".repeat(10);
    assert_eq!(chunk_text(&exact, 10), vec![exact]);
}

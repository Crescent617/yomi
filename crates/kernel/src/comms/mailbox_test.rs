use super::Mailbox;
use crate::types::ContentBlock;

fn text(value: &str) -> ContentBlock {
    ContentBlock::Text {
        text: value.to_string(),
    }
}

#[tokio::test]
async fn queued_steer_messages_are_joined_with_blank_lines() {
    let mailbox = Mailbox::new();
    mailbox.push_steer(vec![text("[From User] first")]).await;
    mailbox
        .push_steer(vec![text("[From Agent: sub_1] second")])
        .await;

    assert_eq!(
        mailbox.try_pull_steer(20).await,
        vec![
            text("[From User] first"),
            text("\n\n"),
            text("[From Agent: sub_1] second"),
        ]
    );
}

#[tokio::test]
async fn steer_limit_counts_messages_instead_of_content_blocks() {
    let mailbox = Mailbox::new();
    mailbox
        .push_steer(vec![text("first"), text(" continuation")])
        .await;
    mailbox.push_steer(vec![text("second")]).await;

    assert_eq!(
        mailbox.try_pull_steer(1).await,
        vec![text("first"), text(" continuation")]
    );
    assert_eq!(mailbox.try_pull_steer(1).await, vec![text("second")]);
}

// ── 管理面（snapshot / remove / clear_scope） ────────────────────────

use crate::agent::AgentInput;
use crate::comms::{MailboxItemKind, MailboxScope};

fn user_input(value: &str) -> AgentInput {
    AgentInput::User {
        content: vec![text(value)],
    }
}

#[tokio::test]
async fn snapshot_exposes_steer_and_user_in_fifo_with_ids() {
    let mailbox = Mailbox::new();
    mailbox.push(user_input("first task")).await;
    mailbox.push_steer(vec![text("mid-run note")]).await;
    mailbox.push(AgentInput::Compact).await; // 控制输入不暴露
    mailbox.push(user_input("second task")).await;

    let snap = mailbox.snapshot().await;
    assert_eq!(snap.steer.len(), 1);
    assert_eq!(snap.queue.len(), 2, "Compact must stay hidden");
    assert_eq!(snap.steer[0].kind, MailboxItemKind::Steer);
    assert_eq!(snap.steer[0].preview, "mid-run note");
    assert_eq!(snap.queue[0].preview, "first task");
    assert_eq!(snap.queue[1].preview, "second task");
    assert!(snap.steer[0].id.as_str().starts_with("mbx_"));
    assert_ne!(snap.queue[0].id, snap.queue[1].id);
    assert_eq!(
        mailbox.lens().await,
        (1, 3),
        "lens counts control inputs too"
    );
}

#[tokio::test]
async fn remove_retracts_pending_and_fails_safely() {
    let mailbox = Mailbox::new();
    mailbox.push_steer(vec![text("s")]).await;
    mailbox.push(user_input("q")).await;
    let snap = mailbox.snapshot().await;

    assert!(mailbox.remove(&snap.steer[0].id).await);
    assert!(mailbox.remove(&snap.queue[0].id).await);
    // 已移除/已消费的 id 安全失败；clear 后旧 id 也不会误中
    assert!(!mailbox.remove(&snap.steer[0].id).await);
    assert!(mailbox.is_empty());
}

#[tokio::test]
async fn clear_scope_drains_selected_queues() {
    let mailbox = Mailbox::new();
    mailbox.push_steer(vec![text("s1")]).await;
    mailbox.push_steer(vec![text("s2")]).await;
    mailbox.push(user_input("q1")).await;

    assert_eq!(mailbox.clear_scope(MailboxScope::Steer).await, 2);
    assert!(mailbox.is_steer_empty());
    assert!(!mailbox.is_empty());
    assert_eq!(mailbox.clear_scope(MailboxScope::All).await, 1);
    assert!(mailbox.is_empty());
}

#[tokio::test]
async fn preview_flattens_and_truncates() {
    let mailbox = Mailbox::new();
    mailbox
        .push(user_input(&format!("multi\n  line {}", "x".repeat(120))))
        .await;
    let snap = mailbox.snapshot().await;
    assert!(snap.queue[0].preview.starts_with("multi line"));
    assert!(snap.queue[0].preview.ends_with('…'));
    assert!(snap.queue[0].preview.chars().count() <= 80);
}

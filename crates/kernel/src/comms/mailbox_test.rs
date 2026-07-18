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

//! `yomi session mailbox` — pending mailbox management (steer + queued
//! user messages). Consumption events (`mailbox_changed`) keep frontends
//! fresh; this is the CLI/dogfood surface.

use crate::args::GlobalArgs;
use anyhow::Result;
use comfy_table::{ContentArrangement, Table};
use kernel::client::KernelApi;
use kernel::comms::{MailboxItemKind, MailboxScope};
use kernel::types::SessionId;

pub async fn list(global: &GlobalArgs, session: Option<String>) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;
    let kernel = crate::daemon::connect_strict().await?;
    let snap = kernel
        .mailbox_snapshot(&SessionId::from(session_id))
        .await?;

    if snap.steer.is_empty() && snap.queue.is_empty() {
        println!("Mailbox is empty.");
        return Ok(());
    }

    let mut table = Table::new();
    table
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["ID", "KIND", "AGE", "PREVIEW"]);
    table.load_preset(comfy_table::presets::NOTHING);
    if let Some(col) = table.column_mut(0) {
        col.set_padding((0, 1));
    }
    for item in snap.steer.iter().chain(snap.queue.iter()) {
        let kind = match item.kind {
            MailboxItemKind::Steer => "steer",
            MailboxItemKind::Queue => "queue",
        };
        table.add_row(vec![
            item.id.as_str(),
            kind,
            &kernel::storage::format_age(item.enqueued_at),
            &item.preview,
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn remove(global: &GlobalArgs, session: Option<String>, item_id: String) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;
    let kernel = crate::daemon::connect_strict().await?;
    let removed = kernel
        .remove_mailbox_item(&SessionId::from(session_id), &item_id)
        .await?;
    if removed {
        println!("Removed {item_id}.");
    } else {
        println!("Nothing removed — {item_id} is not pending (already consumed or unknown).");
    }
    Ok(())
}

pub async fn clear(
    global: &GlobalArgs,
    session: Option<String>,
    steer: bool,
    queue: bool,
) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;
    let scope = if steer {
        MailboxScope::Steer
    } else if queue {
        MailboxScope::Queue
    } else {
        MailboxScope::All
    };
    let kernel = crate::daemon::connect_strict().await?;
    let removed = kernel
        .clear_mailbox(&SessionId::from(session_id), scope)
        .await?;
    println!("Cleared {removed} pending item(s).");
    Ok(())
}

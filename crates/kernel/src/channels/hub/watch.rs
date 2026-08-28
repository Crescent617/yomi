//! Watch mode (`/watch`): the chat's own session as sole-listener
//! observer — one session per chat, the mapping's `kind` is the mode.
//!
//! `/watch on` flips the chat mapping's `kind` to `watch` (creating the
//! session if absent): from then on every plain message passing access
//! control — mention or not — is *mirrored*: steered verbatim into the
//! chat's session, while the gate suspends the conversation-trigger
//! path (see `gate.rs`). The session is the chat's only message
//! consumer and decides for itself when a reply is warranted. While
//! `kind='watch'` the channel delivers NOTHING for it: no status card,
//! no streaming consumption, no reply delivery, no reactions, no
//! subscriber notify (suppressed at the event-forwarder single point).
//! Its only voice is the platform skill from its own skill list (e.g.
//! `lark` for feishu) — with no matching skill it is a pure read-only
//! observer. `/watch off` flips the kind back to `normal`: the SAME
//! session answers mentions again, its watch-period memory intact. The
//! watch contract lives in the system prompt
//! ([`crate::prompt::watch_section`], appended by the conductor at
//! spawn while kind is `watch`), so it survives context compaction.

use std::sync::Arc;
use tracing::{info, warn};

use crate::kernel::Kernel;
use crate::types::ContentBlock;

use crate::channels::{ChannelMessage, ChannelStore, MappingKind};

use crate::channels::hub_routing::get_or_create_session;

/// Mirror one message into the watched chat's session: assemble the
/// content and steer it in. Fire-and-forget semantics for the caller —
/// a run starts when the session is idle, a mid-run mailbox post when
/// it is already thinking. Images are NOT downloaded (unlike post-gate
/// triggers): the session pulls them via skill only if it cares.
/// Failures are logged, never propagated — the tee must not break the
/// serial dispatch of the conversation path it shadows.
///
/// The tee fires on the gate-time snapshot, so it re-reads the live
/// watch state first (one indexed read): a `/watch off` or gc landing
/// in between must not steer into a session that is `normal` again —
/// it would answer publicly — nor resurrect a watch gc already ended.
///
/// Fast path: an existing, alive mapping steers without taking the
/// route lock (one sqlite read + one existence read per message). Only
/// a dangling mapping (row alive, session gone) goes through the locked
/// get-or-create below; a missing row means watch is off — drop.
pub(crate) async fn mirror_message(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    msg: &ChannelMessage,
) {
    let chat_id = &msg.external_chat_id;
    match store.is_chat_watched(channel_name, chat_id).await {
        Ok(true) => {}
        Ok(false) => {
            info!(channel = %channel_name, chat_id = %chat_id, "watch ended between gate and tee; mirror dropped");
            return;
        }
        Err(e) => {
            warn!(channel = %channel_name, chat_id = %chat_id, error = %e, "watch state re-read failed; mirror dropped");
            return;
        }
    }
    let sid = match store.find_mapping(channel_name, chat_id).await {
        Ok(Some(sid)) => match kernel.session_store().await.get(&sid).await {
            Ok(Some(_)) => sid,
            _ => {
                warn!(channel = %channel_name, chat_id = %chat_id, "watch mapping dangles to a deleted session; recreating");
                if let Err(e) = store.delete_mapping(channel_name, chat_id).await {
                    warn!(channel = %channel_name, error = %e, "stale watch mapping delete failed");
                }
                match get_or_create_session(
                    channel_name,
                    store,
                    kernel,
                    chat_id,
                    chat_id,
                    None,
                    MappingKind::Watch,
                )
                .await
                {
                    Ok((sid, _reused)) => sid,
                    Err(e) => {
                        warn!(channel = %channel_name, chat_id = %chat_id, error = %e, "watch session resolution failed");
                        return;
                    }
                }
            }
        },
        // The re-read above says watched ⇒ the row exists; a missing row
        // here means a concurrent delete — drop, don't resurrect.
        Ok(None) => return,
        Err(e) => {
            warn!(channel = %channel_name, chat_id = %chat_id, error = %e, "watch mapping lookup failed");
            return;
        }
    };
    kernel.send_steer(&sid, mirror_content(msg)).await;
    info!(channel = %channel_name, chat_id = %chat_id, session_id = %sid.0, "mirrored to watch session");
}

/// The mirrored content: the message's own blocks verbatim (the adapter
/// header already carries `[ts][from][chat][msg_id][thread][root]`), plus
/// image references as text — opaque platform keys the session can
/// resolve on demand via its skill.
fn mirror_content(msg: &ChannelMessage) -> Vec<ContentBlock> {
    let mut content = msg.content.clone();
    if !msg.image_keys.is_empty() {
        let refs = msg
            .image_keys
            .iter()
            .map(|key| format!("[image: {key}]"))
            .collect::<Vec<_>>()
            .join(" ");
        content.push(ContentBlock::Text { text: refs });
    }
    content
}

#[cfg(test)]
#[path = "watch_test.rs"]
mod tests;

// ── Query / switch (shared by the `/watch` command and the RPC) ─────────

/// Query a chat's watch mode by channel name.
pub(crate) async fn get_channel_watch_by_name(
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    chat_id: &str,
) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
    let on = store.is_chat_watched(channel_name, chat_id).await?;
    let session_id = store
        .find_mapping(channel_name, chat_id)
        .await?
        .map(|sid| sid.0.to_string());
    Ok(crate::channels::ChannelWatchStatus { on, session_id })
}

/// Switch a chat's watch mode by channel name. Same core as `/watch
/// on|off`: on ensures the chat session exists and flips its kind to
/// `Watch`; off flips back to `Normal`. A flip in either direction
/// cancels the in-flight run and drains the mailbox: pending I/O from
/// the previous mode must not leak into the new one (a queued
/// conversation request must not be answered invisibly while watched,
/// nor a mirrored message wake the session after off).
pub(crate) async fn set_channel_watch_by_name(
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    channel_name: &str,
    chat_id: &str,
    on: bool,
) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
    let sid = if on {
        // Ensure a live row (heals dangling ones), then flip explicitly —
        // get_or_create only writes `kind` on create, so the flip below
        // is needed exactly when an existing row was reused.
        let (sid, reused) = get_or_create_session(
            channel_name,
            store,
            kernel,
            chat_id,
            chat_id,
            None,
            MappingKind::Watch,
        )
        .await?;
        if reused {
            store
                .update_mapping(channel_name, chat_id, None, Some(MappingKind::Watch))
                .await?;
        }
        sid
    } else {
        // Off never creates a session: no row, nothing to flip.
        let Some(sid) = store.find_mapping(channel_name, chat_id).await? else {
            return Ok(crate::channels::ChannelWatchStatus {
                on: false,
                session_id: None,
            });
        };
        store
            .update_mapping(channel_name, chat_id, None, Some(MappingKind::Normal))
            .await?;
        sid
    };
    // A flip in either direction resets pending I/O: a queued
    // conversation request must not be answered invisibly while watched,
    // nor a mirrored message wake the session after off.
    kernel.cancel(&sid);
    kernel
        .clear_mailbox(&sid, crate::comms::MailboxScope::All)
        .await;
    Ok(crate::channels::ChannelWatchStatus {
        on,
        session_id: Some(sid.0.to_string()),
    })
}

impl crate::channels::hub::ChannelHub {
    /// Query a chat's watch mode (channel resolved by name or platform).
    pub async fn get_channel_watch(
        &self,
        channel: Option<&str>,
        platform: &str,
        chat_id: &str,
    ) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
        let (name, ..) = self.resolve_channel(channel, platform)?;
        get_channel_watch_by_name(&self.store(), &name, chat_id).await
    }

    /// Switch a chat's watch mode (channel resolved by name or platform).
    pub async fn set_channel_watch(
        &self,
        kernel: &Kernel,
        channel: Option<&str>,
        platform: &str,
        chat_id: &str,
        on: bool,
    ) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
        let (name, ..) = self.resolve_channel(channel, platform)?;
        set_channel_watch_by_name(&self.store(), kernel, &name, chat_id, on).await
    }

    /// The `set_channel_watch` RPC: `on` absent = query (Vim `:set` style).
    pub async fn rpc_set_channel_watch(
        &self,
        kernel: &Kernel,
        channel: Option<&str>,
        platform: &str,
        chat_id: &str,
        on: Option<bool>,
    ) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
        match on {
            Some(on) => {
                self.set_channel_watch(kernel, channel, platform, chat_id, on)
                    .await
            }
            None => self.get_channel_watch(channel, platform, chat_id).await,
        }
    }
}

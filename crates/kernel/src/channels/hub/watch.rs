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
/// The tee fires on the gate-time snapshot, then re-reads the live row
/// under the route lock — the same lock the kind flip holds across its
/// update+cancel+drain (see [`set_channel_watch_by_name`]), so an
/// off/gc can never interleave between the re-read and the steer (which
/// would make a back-to-`normal` session answer publicly). A missing
/// row means watch is off — drop, never resurrect. Only a dangling row
/// (alive, session gone) heals, outside the lock (get_or_create takes
/// it internally).
pub(crate) async fn mirror_message(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    msg: &ChannelMessage,
) {
    let chat_id = &msg.external_chat_id;
    let lock_key = format!("channel_route:{channel_name}:{chat_id}");
    let mut heal = false;
    {
        let _guard = crate::utils::g_lock::g_lock(lock_key.clone()).await;
        match store.find_mapping_kind(channel_name, chat_id).await {
            Ok(Some((sid, MappingKind::Watch))) => {
                match kernel.session_store().await.get(&sid).await {
                    Ok(Some(_)) => {
                        // Fast path: steer under the lock — one sqlite
                        // read + one existence read per message.
                        kernel.send_steer(&sid, mirror_content(msg)).await;
                        info!(channel = %channel_name, chat_id = %chat_id, session_id = %sid.0, "mirrored to watch session");
                    }
                    _ => {
                        warn!(channel = %channel_name, chat_id = %chat_id, "watch mapping dangles to a deleted session; recreating");
                        if let Err(e) = store.delete_mapping(channel_name, chat_id).await {
                            warn!(channel = %channel_name, error = %e, "stale watch mapping delete failed");
                        }
                        heal = true;
                    }
                }
            }
            Ok(Some((_, MappingKind::Normal))) => {
                info!(channel = %channel_name, chat_id = %chat_id, "watch ended between gate and tee; mirror dropped");
            }
            Ok(None) => {}
            Err(e) => {
                warn!(channel = %channel_name, chat_id = %chat_id, error = %e, "watch mapping lookup failed");
            }
        }
    }
    if !heal {
        return;
    }
    let sid = match get_or_create_session(
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
    };
    let _guard = crate::utils::g_lock::g_lock(lock_key).await;
    if matches!(
        store.find_mapping_kind(channel_name, chat_id).await,
        Ok(Some((_, MappingKind::Watch)))
    ) {
        kernel.send_steer(&sid, mirror_content(msg)).await;
        info!(channel = %channel_name, chat_id = %chat_id, session_id = %sid.0, "mirrored to watch session (healed)");
    }
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
    let row = store.find_mapping_kind(channel_name, chat_id).await?;
    Ok(crate::channels::ChannelWatchStatus {
        on: matches!(row, Some((_, MappingKind::Watch))),
        session_id: row.map(|(sid, _)| sid.0.to_string()),
    })
}

/// Switch a chat's watch mode by channel name. Same core as `/watch
/// on|off`: on ensures the chat session exists and flips its kind to
/// `Watch`; off flips back to `Normal`. Both directions hold the route
/// lock across read-flip-reset — mutually exclusive with the tee's
/// re-read+steer (see [`mirror_message`]).
///
/// A flip cancels the in-flight run and drains the mailbox: pending I/O
/// from the previous mode must not leak into the new one (a queued
/// conversation request must not be answered invisibly while watched,
/// nor a mirrored message wake the session after off). No state change
/// (idempotent on, or off while not watched) is a pure no-op — in
/// particular off must never kill an ordinary session's run.
pub(crate) async fn set_channel_watch_by_name(
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    channel_name: &str,
    chat_id: &str,
    on: bool,
) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
    let lock_key = format!("channel_route:{channel_name}:{chat_id}");
    if on {
        // Already watched (and alive): nothing to do.
        {
            let _guard = crate::utils::g_lock::g_lock(lock_key.clone()).await;
            if let Some((sid, MappingKind::Watch)) =
                store.find_mapping_kind(channel_name, chat_id).await?
            {
                if kernel.session_store().await.get(&sid).await?.is_some() {
                    return Ok(crate::channels::ChannelWatchStatus {
                        on: true,
                        session_id: Some(sid.0.to_string()),
                    });
                }
            }
        }
        // Ensure a live row (heals dangling ones) — outside the lock,
        // get_or_create takes it internally.
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
        let _guard = crate::utils::g_lock::g_lock(lock_key).await;
        if reused {
            store
                .update_mapping(channel_name, chat_id, None, Some(MappingKind::Watch))
                .await?;
        }
        kernel.cancel(&sid);
        kernel
            .clear_mailbox(&sid, crate::comms::MailboxScope::All)
            .await;
        Ok(crate::channels::ChannelWatchStatus {
            on: true,
            session_id: Some(sid.0.to_string()),
        })
    } else {
        let _guard = crate::utils::g_lock::g_lock(lock_key).await;
        let Some((sid, MappingKind::Watch)) =
            store.find_mapping_kind(channel_name, chat_id).await?
        else {
            return Ok(crate::channels::ChannelWatchStatus {
                on: false,
                session_id: None,
            });
        };
        store
            .update_mapping(channel_name, chat_id, None, Some(MappingKind::Normal))
            .await?;
        kernel.cancel(&sid);
        kernel
            .clear_mailbox(&sid, crate::comms::MailboxScope::All)
            .await;
        Ok(crate::channels::ChannelWatchStatus {
            on: false,
            session_id: Some(sid.0.to_string()),
        })
    }
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

//! Watch mode (`/watch`): a chat's sole-listener observer session.
//!
//! While a chat is watch-on (its `watch:{chat_id}` mapping carries
//! `kind='watch'`), every plain message passing access control — mention
//! or not — is *mirrored*: steered verbatim into the observer session,
//! while the gate suspends the conversation-trigger path wholesale (see
//! `gate.rs`): the observer is the group's only message consumer and
//! decides for itself when a reply is warranted. Its mapping kind
//! (`watch`/`watch_off`) suppresses everything the channel would
//! otherwise do for a session: no status card, no streaming consumption,
//! no reply delivery, no reactions, no subscriber notify. Its only
//! voice is the platform skill from its own skill list (e.g. `lark` for
//! feishu) — with no matching skill it is a pure read-only observer.
//! `/watch off` flips the kind to `watch_off`: the mirror tap closes but
//! row, session and context stay put, so `/watch on` resumes the same
//! observer. The watch contract lives in the system prompt
//! ([`crate::prompt::watch_section`], appended by the conductor at
//! spawn), so it survives context compaction.

use std::sync::Arc;
use tracing::{info, warn};

use crate::kernel::Kernel;
use crate::types::ContentBlock;

use crate::channels::{ChannelMessage, ChannelStore, MappingKind};

use crate::channels::hub_routing::get_or_create_session;

/// Mirror one message into the chat's watch-observer session: assemble
/// the content and steer it in. Fire-and-forget semantics for the
/// caller — a run starts when the observer is idle, a mid-run mailbox
/// post when it is already thinking. Images are NOT downloaded (unlike
/// post-gate triggers): the observer pulls them via skill only if it
/// cares. Failures are logged, never propagated — the tee must not
/// break the serial dispatch of the conversation path it mirrors.
///
/// Fast path: an existing, alive mapping steers without any store write
/// (one sqlite read + one existence read per message — the naive
/// get-or-create's ON-CONFLICT rewrite would churn a write per message
/// for zero state change). A dangling mapping (session gc'd/deleted)
/// is dropped and recreated by the locked get-or-create below.
pub(crate) async fn mirror_message(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    msg: &ChannelMessage,
) {
    let chat_id = &msg.external_chat_id;
    let mapping_key = crate::channels::watch_mapping_key(chat_id);
    let sid = match store.find_mapping(channel_name, &mapping_key).await {
        Ok(Some(sid)) => match kernel.session_store().await.get(&sid).await {
            Ok(Some(_)) => Some(sid),
            _ => {
                warn!(channel = %channel_name, chat_id = %chat_id, "watch mapping dangles to a deleted session; recreating");
                if let Err(e) = store.delete_mapping(channel_name, &mapping_key).await {
                    warn!(channel = %channel_name, error = %e, "stale watch mapping delete failed");
                }
                None
            }
        },
        Ok(None) => None,
        Err(e) => {
            warn!(channel = %channel_name, chat_id = %chat_id, error = %e, "watch mapping lookup failed");
            None
        }
    };
    let sid = match sid {
        Some(sid) => sid,
        None => {
            match get_or_create_session(
                channel_name,
                store,
                kernel,
                chat_id,
                &mapping_key,
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
    };
    kernel.send_steer(&sid, mirror_content(msg)).await;
    info!(channel = %channel_name, chat_id = %chat_id, session_id = %sid.0, "mirrored to watch session");
}

/// The mirrored content: the message's own blocks verbatim (the adapter
/// header already carries `[ts][from][chat][msg_id][thread][root]`), plus
/// image references as text — opaque platform keys the observer can
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
    let kind = store.get_watch_state(channel_name, chat_id).await?;
    let session_id = store
        .find_mapping(channel_name, &crate::channels::watch_mapping_key(chat_id))
        .await?
        .map(|sid| sid.0.to_string());
    Ok(crate::channels::ChannelWatchStatus {
        on: matches!(kind, Some(MappingKind::Watch)),
        session_id,
    })
}

/// Switch a chat's watch mode by channel name. Same core as `/watch
/// on|off`: on eagerly creates or resumes the observer session; off
/// flips the row to `watch_off` (kept), cancels the in-flight run, and
/// drains the mailbox.
pub(crate) async fn set_channel_watch_by_name(
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    channel_name: &str,
    chat_id: &str,
    on: bool,
) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
    let watch_key = crate::channels::watch_mapping_key(chat_id);
    if on {
        let (sid, _reused) = get_or_create_session(
            channel_name,
            store,
            kernel,
            chat_id,
            &watch_key,
            None,
            MappingKind::Watch,
        )
        .await?;
        Ok(crate::channels::ChannelWatchStatus {
            on: true,
            session_id: Some(sid.0.to_string()),
        })
    } else {
        let Some(sid) = store.find_mapping(channel_name, &watch_key).await? else {
            return Ok(crate::channels::ChannelWatchStatus {
                on: false,
                session_id: None,
            });
        };
        store
            .save_mapping(
                channel_name,
                &watch_key,
                &sid,
                chat_id,
                None,
                MappingKind::WatchPaused,
            )
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

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

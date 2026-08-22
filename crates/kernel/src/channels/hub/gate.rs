//! Incoming-message gating: access control, mention rules, ack reactions.

use std::sync::Arc;
use tracing::{info, warn};

use crate::channels::hub_command::{parse_channel_command, ChannelCommand};

use crate::channels::hub_routing::resolve_require_mention;

use crate::channels::{ChannelConfig, ChannelMessage, ChannelStore, PlatformAdapter};

/// Outcome of gating one incoming message (see `gate_message`).
/// `comment.rs` hands assembled doc-comment triggers to the dispatch
/// path as `Allow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Accepted — process normally.
    Allow,
    /// Access denied (disabled / allowlist miss / blocklist hit).
    Denied,
    /// Access is fine but the message doesn't address the bot
    /// (mention-missed group chatter): not processed, but it may still
    /// count as a mid-run post (see `record_passive_receipt`).
    NotAddressed,
}

/// Gate one incoming message: enforce access control and the mention
/// requirement, deciding the outcome and the reaction to fire.
///
/// Reaction policy: an accepted, addressed message gets the platform's ack
/// reaction; an allowlist miss gets the access-denied reaction — but only
/// when the message addresses the bot, so random group chatter stays
/// untouched. Blocklist hits, disabled channels, and non-addressed
/// messages stay silent. The reaction itself is fired by the caller, off
/// the serial dispatch path, so heavy per-message work can never delay it.
pub(crate) async fn gate_message(
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    msg: &ChannelMessage,
) -> (Gate, Option<&'static str>) {
    // Access control first: denied messages (blocked users, disabled
    // channels) never cost a store read. The mention requirement only
    // decides the denied reaction for an allowlist miss — resolved
    // lazily on that rare path.
    if let Err(e) = config.check_access(&msg.external_chat_id, &msg.external_user_id) {
        info!(channel = %config.name, error = %e, "access denied");
        if e.is_allowlist_miss() {
            let (require_mention, _) = resolve_require_mention(store, config, msg).await;
            if !require_mention || msg.is_mention {
                return (Gate::Denied, Some(config.platform.access_denied_reaction()));
            }
        }
        return (Gate::Denied, None);
    }
    let (require_mention, _) = resolve_require_mention(store, config, msg).await;
    let addressed = !require_mention || msg.is_mention;
    if !addressed {
        info!(channel = %config.name, chat_id = %msg.external_chat_id, "ignoring non-mention message");
        return (Gate::NotAddressed, None);
    }
    (Gate::Allow, Some(ack_reaction_for(config, &msg)))
}

/// Pick the gate ack reaction: `/queue` messages get their own ("noted,
/// queued") — the trigger ack would promise imminent processing.
pub(crate) fn ack_reaction_for(config: &ChannelConfig, msg: &ChannelMessage) -> &'static str {
    if matches!(
        parse_channel_command(msg.raw_text.as_deref()),
        ChannelCommand::Queue(_)
    ) {
        config.platform.queue_reaction()
    } else {
        config.platform.ack_reaction()
    }
}

/// Best-effort gate reaction; needs an emoji and a message to target and
/// only logs on failure.
pub(crate) async fn send_gate_reaction(
    adapter: &Arc<dyn PlatformAdapter>,
    config: &ChannelConfig,
    msg: &ChannelMessage,
    emoji: Option<&'static str>,
) {
    let (Some(emoji), Some(message_id)) = (emoji, msg.external_message_id.as_deref()) else {
        return;
    };
    if let Err(e) = adapter
        .send_reaction(&msg.external_chat_id, message_id, emoji)
        .await
    {
        warn!(channel = %config.name, error = %e, "gate reaction failed");
    }
}

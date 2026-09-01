//! Incoming-message gating: access control, mention rules, ack reactions.

use std::sync::Arc;
use tracing::{info, warn};

use crate::channels::hub_command::{parse_channel_command, ChannelCommand};

use crate::channels::hub_routing::resolve_require_mention;

use crate::channels::{
    AccessDeniedReason, ChannelConfig, ChannelError, ChannelMessage, ChannelStore, PlatformAdapter,
};

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
/// Returns the outcome, the reaction to fire (if any), and the **watch
/// snapshot**: whether the chat was watch-on at gate time. The snapshot
/// travels with the message through the dispatch queue so the tee and
/// the conversation path decide from ONE read — re-reading at dispatch
/// time would race `/watch on|off` toggles queued in between (a message
/// gated before `/watch on` must not be both mirrored and triggered).
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
) -> (Gate, Option<&'static str>, bool) {
    let cmd = parse_channel_command(msg.raw_text.as_deref());
    // Known commands vs. everything else: unknown slash-words may be
    // another bot's — they follow plain-message rules everywhere, never
    // command rules.
    let known_command = !matches!(cmd, ChannelCommand::None | ChannelCommand::Unknown(_));
    // Access control first: denied messages (blocked users, disabled
    // channels) never cost a store read — with one exception: a watched
    // chat's observer mirrors the WHOLE conversation, so a plain message
    // missing only the USER allowlist skips it and steers to the
    // observer like anyone else's (one store read on that path alone).
    // The bypass covers exactly what the dispatch tee mirrors
    // (`Command::None`) — unknown slash-words (another bot's?) and known
    // commands keep full access control. Explicit blocks, the chat
    // allowlist, and a disabled channel always deny.
    if let Err(e) = config.check_access(&msg.external_chat_id, &msg.external_user_id) {
        if matches!(cmd, ChannelCommand::None)
            && matches!(
                e,
                ChannelError::AccessDenied {
                    reason: AccessDeniedReason::UserNotAllowed,
                    ..
                }
            )
            && read_watch_on(store, config, &msg.external_chat_id).await
        {
            return (Gate::NotAddressed, None, true);
        }
        info!(channel = %config.name, error = %e, "access denied");
        if e.is_allowlist_miss() {
            let (require_mention, _) = resolve_require_mention(store, config, msg).await;
            if !require_mention || msg.is_mention {
                return (
                    Gate::Denied,
                    Some(config.platform.access_denied_reaction()),
                    false,
                );
            }
        }
        return (Gate::Denied, None, false);
    }
    // Watch-on chats (`/watch on`): the observer session is the ONLY
    // consumer of plain messages — the agent itself decides when a reply
    // is warranted (via its platform skill), so the mention requirement
    // and the conversation-trigger path are suspended wholesale: plain
    // chatter AND @-mentions alike are `NotAddressed` (silent — no ack
    // reaction may promise a reply the agent never promised), and the
    // dispatch loop's tee mirrors them. Known commands stay control-plane
    // and execute as usual; unknown slash-words stay silent (they may be
    // another bot's).
    //
    // Watch-on chats (DM included — a watched DM is a silent observer):
    // every plain message is mirrored, conversation triggers suspended.
    // A store read failure falls back to normal gating — a transient
    // sqlite error must not crash message intake, but the degradation
    // (watch-on chat answered like a normal one) must be visible.
    let watch_on = read_watch_on(store, config, &msg.external_chat_id).await;
    // A group command must @ the bot — in EVERY mode: watch-on, a
    // mention-off override, a mention-off channel config. An
    // un-addressed slash word in a group was not spoken to this bot and
    // stays silent (mirroring never applies to commands). DMs keep
    // @-free commands.
    if msg.is_group && known_command && !msg.is_mention {
        info!(channel = %config.name, chat_id = %msg.external_chat_id, "ignoring group command without mention");
        return (Gate::NotAddressed, None, watch_on);
    }
    if watch_on {
        return match cmd {
            ChannelCommand::None | ChannelCommand::Unknown(_) => (Gate::NotAddressed, None, true),
            _ => (Gate::Allow, Some(ack_reaction_for(config, msg)), true),
        };
    }
    let (require_mention, _) = resolve_require_mention(store, config, msg).await;
    let addressed = !require_mention || msg.is_mention;
    if !addressed {
        info!(channel = %config.name, chat_id = %msg.external_chat_id, "ignoring non-mention message");
        return (Gate::NotAddressed, None, false);
    }
    (Gate::Allow, Some(ack_reaction_for(config, msg)), false)
}

/// Whether the chat is currently watch-on (`/watch on`): its mapping row
/// carries kind `watch`. A store read failure falls back to `false`
/// (normal gating) — a transient sqlite error must not crash message
/// intake, but the degradation (a watch-on chat answered like a normal
/// one) must be visible.
async fn read_watch_on(
    store: &Arc<dyn ChannelStore>,
    config: &ChannelConfig,
    chat_id: &str,
) -> bool {
    match store.find_mapping_kind(&config.name, chat_id).await {
        Ok(row) => row.is_some_and(|(_, kind)| kind == crate::channels::MappingKind::Watch),
        Err(e) => {
            warn!(
                channel = %config.name,
                chat_id = %chat_id,
                error = %e,
                "watch state read failed; falling back to normal gating"
            );
            false
        }
    }
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

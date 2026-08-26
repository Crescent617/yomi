//! Channel slash-command definitions, parsing, and reply formatting.

use std::fmt::Write as _;

use crate::storage::format_age;

use crate::channels::obs::fmt_tokens;
use crate::channels::reply::ctx_footer;

pub(crate) const CMD_MODELS: &str = "/models";

pub(crate) const CMD_MODEL: &str = "/model";

pub(crate) const CMD_CLEAR: &str = "/clear";

pub(crate) const CMD_COMPACT: &str = "/compact";

pub(crate) const CMD_STOP: &str = "/stop";

pub(crate) const CMD_STEER: &str = "/steer";

pub(crate) const CMD_QUEUE: &str = "/queue";

pub(crate) const CMD_INFO: &str = "/info";

pub(crate) const CMD_HELP: &str = "/help";

pub(crate) const CMD_PERMITS: &str = "/permits";

pub(crate) const CMD_APPROVE: &str = "/approve";

pub(crate) const CMD_DENY: &str = "/deny";

pub(crate) const CMD_RESTART: &str = "/restart";

pub(crate) const CMD_THREAD: &str = "/thread";

pub(crate) const CMD_SUBSCRIBE: &str = "/subscribe";

pub(crate) const CMD_UNSUBSCRIBE: &str = "/unsubscribe";

pub(crate) const CMD_MENTION: &str = "/mention";

pub(crate) const CMD_THREADS: &str = "/threads";

pub(crate) const CMD_MAILBOX: &str = "/mailbox";

pub(crate) const CMD_SHELL: &str = "/bg";
pub(crate) const CMD_SETTINGS: &str = "/settings";
pub(crate) const CMD_CRON: &str = "/cron";

pub(crate) const CMD_BIND: &str = "/bind";

pub(crate) const CMD_SESSIONS: &str = "/sessions";

pub(crate) const CMD_STATUS: &str = "/status";

pub(crate) const CMD_USAGE: &str = "/usage";

pub(crate) const CMD_WORKFLOW: &str = "/workflow";

/// All channel commands: canonical name plus short aliases. Matching is
/// exact (after stripping an `@bot` suffix), so table order is irrelevant
/// and lookalike words (`/clearance`) never resolve.
pub(crate) const COMMANDS: &[(&str, &[&str])] = &[
    (CMD_HELP, &["/h"]),
    (CMD_INFO, &["/i"]),
    (CMD_MODELS, &[]),
    (CMD_MODEL, &["/m"]),
    (CMD_CLEAR, &["/c"]),
    (CMD_COMPACT, &[]),
    (CMD_STOP, &["/s"]),
    (CMD_STEER, &[]),
    (CMD_QUEUE, &["/q"]),
    (CMD_THREAD, &["/t"]),
    (CMD_SUBSCRIBE, &["/sub"]),
    (CMD_UNSUBSCRIBE, &["/unsub"]),
    (CMD_MENTION, &[]),
    (CMD_THREADS, &[]),
    (CMD_MAILBOX, &["/mb"]),
    (CMD_SHELL, &["/shell"]),
    (CMD_SETTINGS, &[]),
    (CMD_CRON, &[]),
    (CMD_BIND, &[]),
    (CMD_SESSIONS, &[]),
    (CMD_STATUS, &[]),
    (CMD_USAGE, &["/u"]),
    (CMD_WORKFLOW, &["/wkfl"]),
    (CMD_PERMITS, &[]),
    (CMD_APPROVE, &[]),
    (CMD_DENY, &[]),
    (CMD_RESTART, &[]),
];

/// `/help` response: the channel command list.
pub(crate) const HELP_TEXT: &str = "\
**Info**
`/help` (`/h`) — this help
`/info` (`/i`) — current session info
`/mailbox` (`/mb`) — pending steer/queued messages; `/mailbox retract <n>` · `/mailbox clear [steer|queue|all]` (admin)
`/bg` — background tasks (shells + running subagents) with stop buttons (admin)
`/models` — list configured models (current one marked)
`/model` (`/m`) — show current model; `/model <key>` to switch
`/sessions` — recent 10 sessions of this channel with jump links; `/sessions <offset>` for the next page (admin)
`/status` — daemon runtime: uptime, active runs, shells, subagents, cron jobs (admin)
`/usage` (`/u`) — token usage for the last N days; `/usage [days]` (default 7, max 90) (admin)
`/workflow` (`/wkfl`) — workflow scripts in `~/.yomi/workflows/`; `/workflow ls` · `/workflow run <name> [args]` · `/workflow rm <name>` (run/rm admin)

**Session control**
`/clear` (`/c`) — clear context and start fresh
`/compact` — summarize and compact the context
`/stop` (`/s`) — stop the current run
`/steer <text>` — inject a message into the current run
`/queue <text>` (`/q`) — queue a message for a later turn
`/thread <text>` (`/t`) — ask in a new thread opened off this message (Feishu)
`/subscribe [chat_id] [-r]` (`/sub`) — DM you when runs here complete; `-r` covers this chat's threads (Feishu)
`/unsubscribe` (`/unsub`) — cancel the subscription here

**Chat admin**
`/mention` — show the @-requirement here; `/mention on|off|reset` to override it
`/threads` — show reply-in-thread mode for this chat; `/threads on|off|reset` to override it
`/settings` — settings panel card: mention / reply-in-thread / model overrides as dropdowns
`/cron` — cron panel card: pause / resume / delete scheduled jobs (admin; **all** jobs, any chat)
`/bind` — show this conversation's session id; `/bind <session_id>` to retarget it
`/permits` — list pending doc-permission requests
`/approve <id> [perm]` — approve a doc-permission request
`/deny <id>` — deny a doc-permission request
`/restart` — restart the daemon

Anything else is sent to the agent as a message.";

/// Parsed channel command from an incoming message.
pub(crate) enum ChannelCommand {
    /// Clear context and start fresh.
    Clear,
    /// Summarize and compact the session context.
    Compact,
    /// Stop current streaming.
    Stop,
    /// Inject a steer message before the next turn.
    Steer(String),
    /// Queue a normal user message for a later turn.
    Queue(String),
    /// A `/steer` without text.
    InvalidSteerCommand,
    /// A `/queue` without text.
    InvalidQueueCommand,
    /// List configured models and mark the current one.
    ListModels,
    /// Show the current session model.
    CurrentModel,
    /// Switch this session to the model identified by its config key.
    SwitchModel(String),
    /// A model command with too many arguments.
    InvalidModelCommand,
    /// Show basic info about the current session.
    Info,
    /// Show the command list.
    Help,
    /// List pending doc-permission applications (admin only).
    Permits,
    /// Approve a doc-permission application, optionally overriding the level.
    Approve { id: i64, perm: Option<String> },
    /// Deny a doc-permission application.
    Deny { id: i64 },
    /// An approval command with missing or malformed arguments.
    InvalidApprovalCommand,
    /// Restart the daemon (admin only).
    Restart,
    /// One-shot: run this trigger with the reply anchored to the
    /// command message, opening a new thread off it.
    Thread(String),
    /// A `/thread` command without text.
    InvalidThreadCommand,
    /// Subscribe the user to run-completion notifications for this
    /// conversation scope (chat or thread), optionally redirecting the
    /// notification to another chat; `recursive` (chat level only) also
    /// covers runs in this chat's threads.
    Subscribe {
        recursive: bool,
        target_chat_id: Option<String>,
    },
    /// Cancel the user's subscription for this conversation scope.
    Unsubscribe,
    /// A malformed `/subscribe` or `/unsubscribe` command.
    InvalidSubscribeCommand,
    /// `/bind` with no target shows the scope's current session; with a
    /// session id, retargets the scope's mapping to it (admin).
    Bind(Option<String>),
    /// A `/bind` with too many arguments.
    InvalidBindCommand,
    /// Query (`None`) or mutate this conversation's require-mention
    /// override (admin only for mutations).
    Mention(Option<OverrideMode>),
    /// A malformed `/mention` command.
    InvalidMentionCommand,
    /// Query (`None`) or mutate this chat's reply-in-thread override
    /// (admin only for mutations).
    Threads(Option<OverrideMode>),
    /// A malformed `/threads` command.
    InvalidThreadsCommand,
    /// Show or manage the session's pending mailbox (admin).
    Mailbox(crate::channels::mailbox::MailboxSub),
    /// A malformed `/mailbox` command.
    InvalidMailboxCommand,
    /// Background tasks panel (`/bg [--all]`, admin): shells + running
    /// subagents with per-row stop buttons and a refresh. `--all` spans
    /// every session.
    BackgroundTasks {
        /// Show tasks across all sessions (`--all` / `-a`).
        all: bool,
    },
    /// Chat-scope settings panel card (`/settings`, admin): mention /
    /// reply-in-thread / model overrides as select dropdowns.
    Settings,
    /// Chat-scope cron panel card (`/cron`, admin): pause / resume /
    /// delete scheduled jobs — **global** list (any chat's jobs can be
    /// deleted from here).
    Cron,
    /// List this channel's recent sessions (admin), with the page offset.
    Sessions(usize),
    /// A malformed `/sessions` command.
    InvalidSessionsCommand,
    /// Daemon runtime snapshot (admin): uptime, active runs, shells,
    /// subagents, cron jobs, channels.
    Status,
    /// Token usage report (admin) for the last N days.
    Usage(usize),
    /// A malformed `/usage` command.
    InvalidUsageCommand,
    /// List workflow scripts in `<data_dir>/workflows/` (bare `/workflow`
    /// behaves the same).
    WorkflowList,
    /// Run a workflow script by name, passing args through (admin).
    /// Execution is direct (no agent); the result arrives as a follow-up
    /// reply.
    WorkflowRun {
        /// Script name (bare file name).
        name: String,
        /// Arguments passed verbatim to the script.
        args: Vec<String>,
    },
    /// Delete a workflow script by name (admin).
    WorkflowRemove(String),
    /// A malformed `/workflow` command.
    InvalidWorkflowCommand,
    /// Command-shaped (`/word`) but matches no known command or alias.
    Unknown(String),
    /// Not a command.
    None,
}

/// A runtime override mutation (`/mention`, `/threads`): set the
/// override, or clear it to fall back to the inherited value (thread →
/// chat → channel config for `/mention`; channel config for `/threads`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverrideMode {
    On,
    Off,
    Reset,
}

/// Whether a command settles everything before it — run triggers by
/// consuming context, `/clear` by discarding it.
pub(crate) fn consumes_history(cmd: &ChannelCommand) -> bool {
    matches!(
        cmd,
        ChannelCommand::None
            | ChannelCommand::Steer(_)
            | ChannelCommand::Queue(_)
            | ChannelCommand::Thread(_)
            | ChannelCommand::Clear
    )
}

/// Whether the command opens/continues a conversation: only run
/// triggers do (a plain message, `/steer`, `/queue`, `/thread`). Every
/// other command is scoped feedback — including unknown/malformed
/// commands — and must not open a one-reply thread when sent at chat
/// level (see [`crate::channels::hub_routing::command_reply_anchor`]).
pub(crate) fn opens_conversation(cmd: &ChannelCommand) -> bool {
    matches!(
        cmd,
        ChannelCommand::None
            | ChannelCommand::Steer(_)
            | ChannelCommand::Queue(_)
            | ChannelCommand::Thread(_)
    )
}

pub(crate) fn parse_channel_command(raw_text: Option<&str>) -> ChannelCommand {
    let Some(text) = raw_text.map(str::trim).filter(|text| !text.is_empty()) else {
        return ChannelCommand::None;
    };
    let mut parts = text.split_whitespace();
    let Some(cmd) = parts.next() else {
        return ChannelCommand::None;
    };

    let Some(command) = resolve_command(cmd) else {
        // Command-shaped but unknown — surface the typo instead of
        // silently sending it to the agent. Paths (`/tmp/x`) and prose
        // are not command-shaped and pass through as messages.
        return if is_command_shaped(cmd) {
            ChannelCommand::Unknown(cmd.to_string())
        } else {
            ChannelCommand::None
        };
    };

    match command {
        CMD_CLEAR if parts.next().is_none() => ChannelCommand::Clear,
        CMD_COMPACT if parts.next().is_none() => ChannelCommand::Compact,
        CMD_STOP if parts.next().is_none() => ChannelCommand::Stop,
        CMD_INFO if parts.next().is_none() => ChannelCommand::Info,
        CMD_HELP if parts.next().is_none() => ChannelCommand::Help,
        CMD_STEER | CMD_QUEUE => {
            let rest = parts.collect::<Vec<_>>().join(" ");
            if rest.is_empty() {
                if command == CMD_QUEUE {
                    ChannelCommand::InvalidQueueCommand
                } else {
                    ChannelCommand::InvalidSteerCommand
                }
            } else if command == CMD_QUEUE {
                ChannelCommand::Queue(rest)
            } else {
                ChannelCommand::Steer(rest)
            }
        }
        CMD_MODELS | CMD_MODEL => match (parts.next(), parts.next()) {
            (None, None) if command == CMD_MODELS => ChannelCommand::ListModels,
            (None, None) => ChannelCommand::CurrentModel,
            (Some(key), None) => ChannelCommand::SwitchModel(key.to_string()),
            _ => ChannelCommand::InvalidModelCommand,
        },
        CMD_PERMITS if parts.next().is_none() => ChannelCommand::Permits,
        CMD_APPROVE => match (parts.next(), parts.next()) {
            (Some(id), extra) if extra.is_none() || parts.next().is_none() => {
                match id.parse::<i64>() {
                    Ok(id) => ChannelCommand::Approve {
                        id,
                        perm: extra.map(str::to_string),
                    },
                    Err(_) => ChannelCommand::InvalidApprovalCommand,
                }
            }
            _ => ChannelCommand::InvalidApprovalCommand,
        },
        CMD_DENY => match (parts.next(), parts.next()) {
            (Some(id), None) => match id.parse::<i64>() {
                Ok(id) => ChannelCommand::Deny { id },
                Err(_) => ChannelCommand::InvalidApprovalCommand,
            },
            _ => ChannelCommand::InvalidApprovalCommand,
        },
        CMD_RESTART if parts.next().is_none() => ChannelCommand::Restart,
        CMD_THREAD => {
            let rest = parts.collect::<Vec<_>>().join(" ");
            if rest.is_empty() {
                ChannelCommand::InvalidThreadCommand
            } else {
                ChannelCommand::Thread(rest)
            }
        }
        CMD_SUBSCRIBE => {
            let mut recursive = false;
            let mut target_chat_id = None;
            let mut invalid = false;
            for arg in parts {
                match arg {
                    "-r" | "--recursive" => recursive = true,
                    _ if arg.starts_with("oc_") && target_chat_id.is_none() => {
                        target_chat_id = Some(arg.to_string());
                    }
                    _ => {
                        invalid = true;
                        break;
                    }
                }
            }
            if invalid {
                ChannelCommand::InvalidSubscribeCommand
            } else {
                ChannelCommand::Subscribe {
                    recursive,
                    target_chat_id,
                }
            }
        }
        CMD_UNSUBSCRIBE => {
            if parts.next().is_none() {
                ChannelCommand::Unsubscribe
            } else {
                ChannelCommand::InvalidSubscribeCommand
            }
        }
        CMD_MENTION => match (parts.next(), parts.next()) {
            (None, None) => ChannelCommand::Mention(None),
            (Some("on"), None) => ChannelCommand::Mention(Some(OverrideMode::On)),
            (Some("off"), None) => ChannelCommand::Mention(Some(OverrideMode::Off)),
            (Some("reset"), None) => ChannelCommand::Mention(Some(OverrideMode::Reset)),
            _ => ChannelCommand::InvalidMentionCommand,
        },
        CMD_THREADS => match (parts.next(), parts.next()) {
            (None, None) => ChannelCommand::Threads(None),
            (Some("on"), None) => ChannelCommand::Threads(Some(OverrideMode::On)),
            (Some("off"), None) => ChannelCommand::Threads(Some(OverrideMode::Off)),
            (Some("reset"), None) => ChannelCommand::Threads(Some(OverrideMode::Reset)),
            _ => ChannelCommand::InvalidThreadsCommand,
        },
        CMD_SETTINGS => ChannelCommand::Settings,
        CMD_CRON => ChannelCommand::Cron,
        CMD_MAILBOX => match (parts.next(), parts.next(), parts.next()) {
            (None, None, None) => {
                ChannelCommand::Mailbox(crate::channels::mailbox::MailboxSub::Show)
            }
            (Some("clear"), scope, None) => match scope {
                None | Some("all") => ChannelCommand::Mailbox(
                    crate::channels::mailbox::MailboxSub::Clear(crate::comms::MailboxScope::All),
                ),
                Some("steer") => ChannelCommand::Mailbox(
                    crate::channels::mailbox::MailboxSub::Clear(crate::comms::MailboxScope::Steer),
                ),
                Some("queue") => ChannelCommand::Mailbox(
                    crate::channels::mailbox::MailboxSub::Clear(crate::comms::MailboxScope::Queue),
                ),
                _ => ChannelCommand::InvalidMailboxCommand,
            },
            (Some("retract"), Some(n), None) => match n.parse::<usize>() {
                Ok(n) if n > 0 => {
                    ChannelCommand::Mailbox(crate::channels::mailbox::MailboxSub::Retract(n))
                }
                _ => ChannelCommand::InvalidMailboxCommand,
            },
            _ => ChannelCommand::InvalidMailboxCommand,
        },
        CMD_SHELL => match (parts.next(), parts.next()) {
            (None, None) => ChannelCommand::BackgroundTasks { all: false },
            (Some("--all" | "-a"), None) => ChannelCommand::BackgroundTasks { all: true },
            _ => ChannelCommand::Unknown(CMD_SHELL.to_string()),
        },
        CMD_BIND => match (parts.next(), parts.next()) {
            (None, None) => ChannelCommand::Bind(None),
            (Some(id), None) => ChannelCommand::Bind(Some(id.to_string())),
            _ => ChannelCommand::InvalidBindCommand,
        },
        CMD_SESSIONS => match (parts.next(), parts.next()) {
            (None, None) => ChannelCommand::Sessions(0),
            (Some(n), None) => n
                .parse::<usize>()
                .map(ChannelCommand::Sessions)
                .unwrap_or(ChannelCommand::InvalidSessionsCommand),
            _ => ChannelCommand::InvalidSessionsCommand,
        },
        CMD_STATUS if parts.next().is_none() => ChannelCommand::Status,
        CMD_USAGE => match (parts.next(), parts.next()) {
            (None, None) => ChannelCommand::Usage(USAGE_DEFAULT_DAYS),
            (Some(n), None) => match n.parse::<usize>() {
                Ok(days) if (1..=USAGE_MAX_DAYS).contains(&days) => ChannelCommand::Usage(days),
                _ => ChannelCommand::InvalidUsageCommand,
            },
            _ => ChannelCommand::InvalidUsageCommand,
        },
        CMD_WORKFLOW => match parts.next() {
            None | Some("ls" | "list") if parts.next().is_none() => ChannelCommand::WorkflowList,
            Some("run") => match parts.next() {
                Some(name) => ChannelCommand::WorkflowRun {
                    name: name.to_string(),
                    args: parts.map(str::to_string).collect(),
                },
                None => ChannelCommand::InvalidWorkflowCommand,
            },
            Some("rm" | "remove") => match (parts.next(), parts.next()) {
                (Some(name), None) => ChannelCommand::WorkflowRemove(name.to_string()),
                _ => ChannelCommand::InvalidWorkflowCommand,
            },
            _ => ChannelCommand::InvalidWorkflowCommand,
        },
        _ => ChannelCommand::None,
    }
}

/// The command token without an `@bot` suffix (`/c@yomi_bot` → `/c`).
pub(crate) fn command_base(token: &str) -> &str {
    token.split('@').next().unwrap_or(token)
}

/// Resolve a command token to its canonical name: an exact match on the
/// canonical name or an alias, allowing an `@bot` suffix (`/clear`,
/// `/c@yomi_bot`) — never a longer word (`/clearance` is not a command).
pub(crate) fn resolve_command(token: &str) -> Option<&'static str> {
    let base = command_base(token);
    COMMANDS
        .iter()
        .find(|(name, aliases)| *name == base || aliases.contains(&base))
        .map(|(name, _)| *name)
}

/// Whether a token is command-shaped: `/word` (word chars only, optional
/// `@bot` suffix). Paths (`/tmp/x`) and prose are not, so they pass
/// through to the agent as messages.
pub(crate) fn is_command_shaped(token: &str) -> bool {
    command_base(token).strip_prefix('/').is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    })
}

pub(crate) fn has_channel_command_prefix(raw_text: &str) -> bool {
    let command = raw_text.split_whitespace().next().unwrap_or_default();
    resolve_command(command).is_some()
}

/// Whether a fetched history message is a channel command (`/info`,
/// `@bot /clear`). Commands are control-plane: their replies bypass
/// sessions, so echoing them in chat history presents an exchange the bot
/// cannot see. Leading `@mention` tokens are stripped for
/// detection only (a group command fetches back as `@_user_1 /info`) —
/// anything not command-shaped stays, rendered verbatim.
/// `pub(crate)` for the doc-comment thread history (same filter).
pub(crate) fn is_command_text(text: &str) -> bool {
    let mut rest = text.trim_start();
    while let Some(t) = rest.strip_prefix('@') {
        rest = t
            .split_once(char::is_whitespace)
            .map_or("", |(_, r)| r.trim_start());
    }
    has_channel_command_prefix(rest)
}

pub(crate) fn format_model_list(models: &[crate::kernel::ModelInfo], current: &str) -> String {
    if models.is_empty() {
        return "No models are currently available.".to_string();
    }

    let mut lines = Vec::new();
    for model in models {
        let marker = if model.name == current {
            " **← current**"
        } else {
            ""
        };
        lines.push(format!(
            "- `{}` · {} · `{}` · {}k ctx{}",
            model.name,
            model.provider,
            model.model_id,
            model.context_window / 1000,
            marker
        ));
    }
    lines.push(String::new());
    lines.push("Switch with `/model <model_key>`.".to_string());
    lines.join("\n")
}

pub(crate) fn format_current_model(models: &[crate::kernel::ModelInfo], current: &str) -> String {
    models
        .iter()
        .find(|model| model.name == current)
        .map_or_else(
            || format!("`{current}` — not one of the configured models.\n\nUse `/models` to list available models."),
            |model| {
                format!(
                "`{}` · {} · `{}` · {}k ctx\n\nSwitch with `/model <model_key>`.",
                model.name,
                model.provider,
                model.model_id,
                model.context_window / 1000
            )
            },
        )
}

pub(crate) fn format_session_info(
    session: &crate::types::SessionResponse,
    model_key: &str,
    models: &[crate::kernel::ModelInfo],
    running_subagents: usize,
    shells: &[crate::agent::BackgroundShellTask],
    context_tokens: Option<u32>,
) -> String {
    let found = models.iter().find(|m| m.name == model_key);
    let model = found.map_or_else(
        || format!("`{model_key}`"),
        |m| {
            format!(
                "`{}` · {} · `{}` · {}k ctx",
                m.name,
                m.provider,
                m.model_id,
                m.context_window / 1000
            )
        },
    );
    // Sessions without a persisted model key resolve to the default model.
    let default_marker = if session.model_key.is_none() {
        " (default)"
    } else {
        ""
    };
    // Current context occupancy: absolute + window share when the model's
    // window is known; `—` until the first response records usage.
    let context = match context_tokens {
        Some(tokens) => match found {
            Some(m) => format!(
                "{}/{} ({})",
                fmt_tokens(u64::from(tokens)),
                fmt_tokens(u64::from(m.context_window)),
                ctx_footer(tokens, m.context_window)
            ),
            None => fmt_tokens(u64::from(tokens)),
        },
        None => "—".to_string(),
    };
    let shells_text = if shells.is_empty() {
        "- **Background shells**: 0".to_string()
    } else {
        let mut text = format!("- **Background shells**: {}", shells.len());
        for s in shells {
            // Commands are user-influenceable: neutralize the two chars
            // that can break the row's inline-code markup (cosmetic).
            let cmd = s.command.replace('`', "｀").replace('\n', " ");
            let _ = write!(
                text,
                "\n  - `{cmd}` · pid {} · {}",
                s.pid,
                format_age(s.started_at)
            );
        }
        text
    };
    [
        format!("- **ID**: `{}`", session.id.0),
        format!("- **Model**: {model}{default_marker}"),
        format!("- **Context**: {context}"),
        format!("- **Status**: {}", session.phase),
        format!(
            "- **Created**: {} · **Active**: {}",
            format_age(session.created_at),
            format_age(session.updated_at)
        ),
        format!(
            "- **Permission**: {}",
            session.auto_approve_level.as_deref().unwrap_or("default")
        ),
        format!("- **Subagents**: {running_subagents} running"),
        shells_text,
        format!(
            "- **Daemon**: yomi v{} · wire v{}",
            env!("CARGO_PKG_VERSION"),
            crate::wire::WIRE_PROTOCOL_VERSION
        ),
    ]
    .join("\n")
}

pub(crate) fn format_unknown_model(key: &str, models: &[crate::kernel::ModelInfo]) -> String {
    let keys = models
        .iter()
        .map(|model| format!("`{}`", model.name))
        .collect::<Vec<_>>()
        .join(", ");
    if keys.is_empty() {
        format!("Model `{key}` was not found. No models are currently available.")
    } else {
        format!(
            "Model `{key}` was not found.\n\nAvailable model keys: {keys}\n\nUse `/models` for details."
        )
    }
}

/// `/usage` window default and cap (days).
pub(crate) const USAGE_DEFAULT_DAYS: usize = 7;

pub(crate) const USAGE_MAX_DAYS: usize = 90;

/// Uptime since boot: the `format_age` buckets as a bare duration
/// ("2d", "3h", "5m", "<1m").
pub(crate) fn format_uptime(boot: chrono::DateTime<chrono::Utc>) -> String {
    let up = chrono::Utc::now() - boot;
    if up.num_days() > 0 {
        format!("{}d", up.num_days())
    } else if up.num_hours() > 0 {
        format!("{}h", up.num_hours())
    } else if up.num_minutes() > 0 {
        format!("{}m", up.num_minutes())
    } else {
        "<1m".to_string()
    }
}

/// `/workflow` 子命令用法（解析失败时的回执）。
pub(crate) const WORKFLOW_USAGE: &str = "Usage: `/workflow ls` · `/workflow run <name> [args]` · `/workflow rm <name>` — scripts live in `~/.yomi/workflows/` (run/rm admin).";

/// `/workflow ls` body: one line per script, non-executable ones flagged
/// (they cannot `run` until `chmod +x`).
pub(crate) fn format_workflow_list(
    dir: &std::path::Path,
    entries: &[crate::workflow::WorkflowEntry],
) -> String {
    if entries.is_empty() {
        return format!(
            "No workflows yet. Drop an executable script (shebang + `chmod +x`) into `{}`.",
            dir.display()
        );
    }
    let mut lines: Vec<String> = entries
        .iter()
        .map(|e| {
            if e.executable {
                format!("- `{}`", e.name)
            } else {
                format!("- `{}` · not executable", e.name)
            }
        })
        .collect();
    lines.push(String::new());
    lines.push(format!(
        "Run with `/workflow run <name> [args]` · dir `{}`",
        dir.display()
    ));
    lines.join("\n")
}

/// `/workflow run` 输出回显上限（字节）：头尾保留截断。
pub(crate) const WORKFLOW_OUTPUT_MAX: usize = 3000;

/// `/workflow run` 完成回执：状态行 + 代码块包裹的输出（头尾截断）。
pub(crate) fn format_workflow_result(name: &str, outcome: &crate::workflow::RunOutcome) -> String {
    let status = if outcome.timed_out {
        format!(
            "⏱ **Timed out** after {}s (killed)",
            crate::workflow::RUN_TIMEOUT.as_secs()
        )
    } else {
        match outcome.exit_code {
            Some(0) => "✅ **exit 0**".to_string(),
            Some(code) => format!("⚠️ **exit {code}**"),
            None => "⚠️ **killed by signal**".to_string(),
        }
    };
    let head = format!(
        "`{name}` · {status} · {:.1}s",
        outcome.elapsed.as_secs_f64()
    );
    let output = outcome.output.trim();
    if output.is_empty() {
        return format!("{head}\n\n(no output)");
    }
    let truncated = crate::tools::helper::truncate::truncate_keep_edges(
        output,
        WORKFLOW_OUTPUT_MAX,
        "\n\n… [output truncated] …\n\n",
    );
    format!("{head}\n\n```\n{truncated}\n```")
}

/// `/status` body: one line per runtime gauge. `cron_jobs` is `None`
/// when the cron store is disabled; channels render as `name (state)`.
pub(crate) fn format_runtime_status(
    boot: chrono::DateTime<chrono::Utc>,
    active_runs: usize,
    shells: usize,
    subagents: usize,
    cron_jobs: Option<usize>,
    channels: &[crate::channels::ChannelInfo],
) -> String {
    // ChannelStatus 的实际状态机：receiver 任务存活期间恒为
    // Connecting（启动值），干净退出才翻 Idle，出错翻 Error（见
    // hub/mod.rs 的 status_recv 写入点）——所以存活映射 "up"。
    let state = |s: &crate::channels::ChannelStatus| match s {
        crate::channels::ChannelStatus::Connecting => "up",
        crate::channels::ChannelStatus::Idle => "stopped",
        crate::channels::ChannelStatus::Error => "error",
    };
    let mut lines = vec![
        format!(
            "- **Daemon**: yomi v{} · wire v{} · up {}",
            env!("CARGO_PKG_VERSION"),
            crate::wire::WIRE_PROTOCOL_VERSION,
            format_uptime(boot)
        ),
        format!("- **Active runs**: {active_runs}"),
        format!("- **Background shells**: {shells}"),
        format!("- **Running subagents**: {subagents}"),
    ];
    if let Some(n) = cron_jobs {
        lines.push(format!("- **Cron jobs**: {n} active"));
    }
    if !channels.is_empty() {
        let list = channels
            .iter()
            .map(|c| format!("{} ({})", c.name, state(&c.status)))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("- **Channels**: {list}"));
    }
    lines.join("\n")
}

/// `/usage` body: window totals, today, top models (≤5), newest daily
/// rows (≤7). `daily` is ascending and only covers days with usage, so
/// the "today" line appears only when the latest row really is today.
pub(crate) fn format_usage(
    days: usize,
    summary: &crate::storage::usage::UsageSummary,
    daily: &[crate::storage::usage::DailyUsage],
    models: &[crate::storage::usage::ModelUsage],
) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut lines = vec![format!(
        "- **Total ({days}d)**: {} tok ({} cached) · {} req",
        fmt_tokens(summary.total_tokens()),
        fmt_tokens(summary.cached_tokens),
        summary.request_count
    )];
    if let Some(d) = daily.last().filter(|d| d.date == today) {
        lines.push(format!(
            "- **Today**: {} tok · {} req",
            fmt_tokens(d.total_tokens()),
            d.request_count
        ));
    }
    if !models.is_empty() {
        lines.push(String::new());
        lines.push("**By model**".to_string());
        for m in models.iter().take(5) {
            lines.push(format!(
                "- `{}` · {} tok · {} req",
                m.model,
                fmt_tokens(m.total_tokens()),
                m.request_count
            ));
        }
    }
    if daily.len() > 1 {
        lines.push(String::new());
        lines.push("**Daily**".to_string());
        for d in daily.iter().rev().take(7) {
            lines.push(format!(
                "- {} · {} tok · {} req",
                &d.date[5..],
                fmt_tokens(d.total_tokens()),
                d.request_count
            ));
        }
    }
    lines.join("\n")
}

/// 打错命令时的建议：与任一命令名/别名编辑距离 ≤2 即提示（取最近者）。
/// 距离用 OSA（相邻交换算 1 步，覆盖最常见的 typo）。
pub(crate) fn suggest_command(cmd: &str) -> Option<&'static str> {
    let cmd = cmd.trim_start_matches('/');
    let mut best: Option<(&'static str, usize)> = None;
    for &(name, aliases) in COMMANDS {
        for candidate in std::iter::once(name).chain(aliases.iter().copied()) {
            let d = levenshtein(cmd, candidate.trim_start_matches('/'));
            if d <= 2 && d < best.map_or(usize::MAX, |(_, bd)| bd) {
                best = Some((name, d));
            }
        }
    }
    best.map(|(name, _)| name)
}

/// 经典 Levenshtein（命令名都很短，O(n·m) 足够）。
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate().take(n + 1) {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate().take(m + 1) {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

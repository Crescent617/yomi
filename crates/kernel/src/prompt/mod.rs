use crate::memory;
use crate::skill::Skill;
use chrono::Local;
use std::fmt::Write;
use std::sync::Arc;

/// Builder for system prompts with skill integration
#[derive(Debug, Default)]
pub struct SystemPromptBuilder<'a> {
    base_prompt: Option<&'a str>,
    skills: &'a [Arc<Skill>],
    working_dir: Option<&'a std::path::Path>,
    session_id: Option<&'a str>,
}

const SKILL_SECTION_HEADER: &str = "# Skills\nIMPORTANT: before replying, you must scan available skills and load skill content with `read` tool when task hits its description.\n\n";

/// Attachment contract for every non-sub-agent session (when the
/// `attachments` feature is on): files declared in a `<yomi_attachments>`
/// block reach the user as attachments alongside the message — channels
/// deliver the files, the app shows clickable items. Appended to the base
/// prompt by the conductor at spawn time. Sub-agents never get it: a
/// sub-agent's parent decides what becomes an attachment.
pub(crate) const ATTACHMENTS_SECTION: &str = "# Attachments\nTo attach files to your reply, include an attachments block, one path per line (absolute, or relative to the workspace) — each is delivered to the user as an attachment alongside your message:\n\n<yomi_attachments>\noutput/report.pdf\n</yomi_attachments>\n\nTo show this syntax to the user instead of attaching files, wrap it in a fenced code block.";

/// Mention contract for channel-routed sessions: `<@USER_ID>` in a reply
/// is rewritten by each platform adapter into its native mention (feishu
/// `<at id=…>`, telegram `tg://user?id=…`). Sub-agents and local sessions
/// never get it — no platform is there to render it.
pub(crate) const MENTIONS_SECTION: &str = "# Mentions\nTo mention a user in your reply, write `<@USER_ID>` — the platform renders it as a real mention with notification. Use it only when warranted: the user asked you to @ someone, or you are addressing a bot — in that case the mention is required (it won't receive your message otherwise). Never @ any human gratuitously.";

/// Watch-observer contract for a watched chat's observer session
/// (`/watch`, mapping kind `watch`): every non-command message of the
/// chat is mirrored to it and it is the chat's ONLY message consumer
/// (mention triggers are suspended while watch is on) — but the channel
/// delivers NOTHING for it: its final text is never posted, no status
/// card, no reactions. Its only voice is the platform skill from its own
/// skill list; without one it is a pure read-only observer. Appended to
/// the base prompt by the conductor at spawn (derived from the session's
/// routing row), so the contract survives context compaction. `paused`
/// = the mirror tap is currently closed (`/watch off`): the delivery
/// suppression still holds, but the "sole listener" clause is dropped —
/// no new messages are arriving.
pub(crate) fn watch_section(channel_name: &str, chat_id: &str, paused: bool) -> String {
    let intake = if paused {
        "watch is currently PAUSED: no new messages are being mirrored to you."
    } else {
        "every non-command message of the chat — including threads and @-mentions of you — \
         is delivered to you alone."
    };
    format!(
        "# Watch mode\n\
         You are the sole listener of group chat `{chat_id}` on channel `{channel_name}`: {intake}\n\
         - The channel delivers NOTHING for you: your reply text is never posted, and no cards \
         or reactions mark your runs. To speak, use the skill covering this platform from your \
         own skill list (e.g. the `lark` skill for feishu) via shell — target messages or \
         threads by the `[msg_id: …]` / `[thread: …]` ids in each message's header. If no \
         installed skill covers this platform, you cannot speak at all: stay a pure observer.\n\
         - Messages that @-mention you are direct addresses: usually respond to those (via \
         skill). Everything else, silence is the default — speak only when you have something \
         worth interrupting the chat for.\n\
         - There is no separate conversation session while watch is on: nobody else answers \
         mentions. If you stay silent, a mention goes unanswered."
    )
}

/// Contract sections appended after the base prompt for non-sub-agent
/// sessions (the caller owns that gate): attachment syntax when the
/// feature is on, mention syntax when a platform is there to render it
/// (channel-routed sessions). Each enabled section leads with a blank
/// line, so the result appends to the base prompt verbatim.
pub(crate) fn contract_sections(enable_attachments: bool, channel_routed: bool) -> String {
    let mut sections = String::new();
    if enable_attachments {
        sections = format!("{sections}\n\n{ATTACHMENTS_SECTION}");
    }
    if channel_routed {
        sections = format!("{sections}\n\n{MENTIONS_SECTION}");
    }
    sections
}

/// Per-session rules: `<data_dir>/sessions/rules/<sid>.md`, appended to
/// the base prompt **verbatim** at spawn when the file exists and is
/// non-empty — no header, no framing, the file speaks for itself (the
/// capability contract lives in the `yomi-self` skill, not in the
/// prompt). No file → no injection: zero prompt noise for sessions
/// without rules. Capped at [`SESSION_RULES_MAX_BYTES`] with a
/// truncation marker so an oversized file can't bloat every prompt.
pub(crate) const SESSION_RULES_MAX_BYTES: usize = 4096;

/// Read the session's RULE.md for prompt injection (see above).
pub(crate) async fn session_rules_section(
    data_dir: &std::path::Path,
    session_id: &str,
) -> Option<String> {
    let path = data_dir
        .join("sessions")
        .join("rules")
        .join(format!("{session_id}.md"));
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    let content = content.trim();
    if content.is_empty() {
        return None;
    }
    if content.len() > SESSION_RULES_MAX_BYTES {
        // UTF-8 安全截断：floor_char_boundary（夜间 MSRV 可用 stable 替代：
        // 逐字节回退到 char 边界）。
        let mut end = SESSION_RULES_MAX_BYTES;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        return Some(format!("{}\n\n(truncated)", &content[..end]));
    }
    Some(content.to_string())
}

/// Everything needed to assemble a session's spawn-time system prompt.
pub(crate) struct SystemPromptParts<'a> {
    pub base_prompt: String,
    pub template_body: Option<String>,
    pub is_sub_agent: bool,
    pub enable_attachments: bool,
    pub channel_routed: bool,
    /// `(channel_name, chat_id, paused)` — set when the session is a watch
    /// observer (derived from its routing row; see [`watch_section`]).
    pub watch: Option<(&'a str, &'a str, bool)>,
    pub data_dir: &'a std::path::Path,
    pub session_id: &'a str,
}

/// Assemble the system prompt for one spawn. Template body wins outright;
/// otherwise a main session gets base + capability contract sections
/// (attachments, mentions) + the watch-observer section when it observes
/// a chat, while sub-agents keep the bare base. Either way the session's
/// RULE.md is appended verbatim when present (see
/// [`session_rules_section`]).
pub(crate) async fn compose_system_prompt(parts: SystemPromptParts<'_>) -> String {
    let mut prompt = match &parts.template_body {
        Some(t) => t.clone(),
        None if !parts.is_sub_agent => {
            let mut p = parts.base_prompt;
            p.push_str(&contract_sections(
                parts.enable_attachments,
                parts.channel_routed,
            ));
            if let Some((channel, chat_id, paused)) = parts.watch {
                p = format!("{p}\n\n{}", watch_section(channel, chat_id, paused));
            }
            p
        }
        None => parts.base_prompt,
    };
    if let Some(rules) = session_rules_section(parts.data_dir, parts.session_id).await {
        prompt = format!("{prompt}\n\n{rules}");
    }
    prompt
}

/// Memory library pointer, injected only when a memory index actually exists
/// (project `.agents/memory/MEMORY.md` and/or global `~/.agents/memory/MEMORY.md`).
/// The convention lives in the system prompt; the facts live in the files —
/// same pattern as the skills section.
const MEMORY_SECTION_HEADER: &str = "# Memory\nYou have a persistent memory library. Skim its index before starting work, and record new durable facts (build gotchas, root causes, user preferences) back into it: one fact per line in MEMORY.md, details in topic files beside it.\n\n";

impl<'a> SystemPromptBuilder<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn base_prompt(mut self, prompt: &'a str) -> Self {
        self.base_prompt = Some(prompt);
        self
    }

    #[must_use]
    pub const fn with_skills(mut self, skills: &'a [Arc<Skill>]) -> Self {
        self.skills = skills;
        self
    }

    #[must_use]
    pub const fn with_working_dir(mut self, dir: &'a std::path::Path) -> Self {
        self.working_dir = Some(dir);
        self
    }

    #[must_use]
    pub fn with_session_id(mut self, session_id: &'a str) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Build system prompt, loading project memory from `working_dir` if set
    pub async fn build(self) -> String {
        let base = self
            .base_prompt
            .unwrap_or("You are a helpful AI coding assistant.")
            .trim();

        let mut prompt = base.to_string();

        // Load and append project memory from working_dir if available
        if let Some(cwd) = self.working_dir {
            if let Ok(memory) = memory::load(cwd).await {
                for file in memory.files() {
                    prompt.push_str("\n\n");
                    prompt.push_str(file.content.trim());
                }
            }
        }

        // Memory library pointer (existence-gated): absent directories
        // inject nothing, so non-memory projects pay zero prompt cost.
        let mut memory_lines: Vec<String> = Vec::new();
        if let Some(cwd) = self.working_dir {
            let project_index = cwd.join(".agents/memory/MEMORY.md");
            if tokio::fs::try_exists(&project_index).await.unwrap_or(false) {
                memory_lines.push(format!("- Project: {}", project_index.display()));
            }
        }
        let global_index = crate::utils::path::expand_tilde("~/.agents/memory/MEMORY.md");
        if tokio::fs::try_exists(&global_index).await.unwrap_or(false) {
            memory_lines.push(format!("- Global: {}", global_index.display()));
        }
        if !memory_lines.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(MEMORY_SECTION_HEADER);
            for line in memory_lines {
                prompt.push_str(&line);
                prompt.push('\n');
            }
        }

        prompt.push_str("\n\n");

        if !self.skills.is_empty() {
            prompt.push_str(SKILL_SECTION_HEADER);
            prompt.push_str("## Available Skills\n");
            for skill in self.skills {
                let _ = write!(
                    prompt,
                    "name: {}\ndescription: {}\npath: {}\n\n",
                    skill.name,
                    skill.description,
                    skill.source_path.display()
                );
            }
        }

        prompt.push_str("# Environment\n");
        let _ = write!(
            prompt,
            "agent kernel: Yomi\nDate: {}",
            Local::now().format("%Y-%m-%d")
        );
        if let Some(cwd) = self.working_dir {
            let _ = write!(prompt, "\nCWD: {}", cwd.display());
        }
        let _ = write!(
            prompt,
            "\nOS: {} ({})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        if let Some(session_id) = self.session_id {
            let _ = write!(prompt, "\nSession: {session_id}");
        }
        prompt
    }
}

#[cfg(test)]
#[path = "prompt_test.rs"]
mod tests;

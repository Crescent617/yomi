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

/// Watch-observer contract for a watched chat's session (`/watch`,
/// mapping kind `watch`). Deliberately minimal: state the mode (every
/// message is mirrored for observation), the hard boundary (nothing the
/// session outputs reaches the chat), and the only way out (speak via
/// the platform skill, anchored by header ids). When to speak is the
/// agent's own judgement — the contract must not script it. Appended to
/// the base prompt by the conductor at spawn (while the routing row's
/// kind is `watch`), so it survives context compaction.
pub(crate) fn watch_section(channel_name: &str, chat_id: &str) -> String {
    format!(
        "# Watch mode\n\
         You are in watch mode for chat `{chat_id}` on channel `{channel_name}`: every message \
         here is mirrored to you for observation.\n\
         Nothing you output reaches the chat: your reply text is never posted, and no cards or \
         reactions mark your runs.\n\
         To speak when you judge it worthwhile, use the platform skill from your own skill list \
         (e.g. `lark` for feishu) via shell, targeting messages or threads by the \
         `[msg_id: …]` / `[thread: …]` ids in each message's header."
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

/// Path of a session's RULE.md, or `None` when the id can't safely name
/// a file: session ids may originate from client RPC strings
/// ([`crate::types::SessionId`] is serde-transparent), so an unvalidated
/// id like `../../etc/x` would make the daemon read an arbitrary `.md`
/// file into the system prompt (exfiltrated via the agent's reply).
/// Only `[A-Za-z0-9_-]` ids (ULID-style) may name a rules file.
pub(crate) fn session_rules_path(
    data_dir: &std::path::Path,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some(
        data_dir
            .join("sessions")
            .join("rules")
            .join(format!("{session_id}.md")),
    )
}

/// Read the session's RULE.md for prompt injection (see above).
pub(crate) async fn session_rules_section(
    data_dir: &std::path::Path,
    session_id: &str,
) -> Option<String> {
    use tokio::io::AsyncReadExt;

    let path = session_rules_path(data_dir, session_id)?;
    // Bounded read: at most MAX+1 bytes are loaded — the extra byte is
    // enough to know the file is oversized without reading it whole.
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            // 权限等错误降级为"无规则"，但留下可诊断的日志。
            tracing::warn!(path = %path.display(), "cannot open session rules: {e}");
            return None;
        }
    };
    let mut buf = Vec::new();
    if let Err(e) = file
        .take(SESSION_RULES_MAX_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .await
    {
        tracing::warn!(path = %path.display(), "cannot read session rules: {e}");
        return None;
    }

    let truncated = buf.len() > SESSION_RULES_MAX_BYTES;
    // UTF-8 安全截断：逐字节回退到 char 边界（截断点可能落在多字节字符
    // 中间；未截断时 end == buf.len() 即全文末尾，天然是边界）。
    let mut end = buf.len().min(SESSION_RULES_MAX_BYTES);
    while end > 0 && end < buf.len() && (buf[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    // 非 UTF-8 文件按"无规则"处理（与 read_to_string 失败的旧语义一致）。
    let content = std::str::from_utf8(&buf[..end]).ok()?.trim();
    if content.is_empty() {
        return None;
    }
    if truncated {
        return Some(format!("{content}\n\n(truncated)"));
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
    /// `(channel_name, chat_id)` — set while the session's chat is
    /// watch-on (derived from its routing row; see [`watch_section`]).
    pub watch: Option<(&'a str, &'a str)>,
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
            if let Some((channel, chat_id)) = parts.watch {
                p = format!("{p}\n\n{}", watch_section(channel, chat_id));
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

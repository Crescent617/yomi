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
        let _ = write!(prompt, "Date: {}", Local::now().format("%Y-%m-%d"));
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

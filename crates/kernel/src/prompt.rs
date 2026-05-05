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
}

const SKILL_SECTION_HEADER: &str = "# Skills\nIMPORTANT: before replying, you must scan available skills and load skill content with `read` tool when task hits its description.\n\n";

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

    pub fn build(self) -> String {
        let base = self
            .base_prompt
            .unwrap_or("You are a helpful AI coding assistant.")
            .trim();
        let mut prompt = base.to_string();
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
        prompt
    }
}

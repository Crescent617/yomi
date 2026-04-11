use crate::skill::Skill;
use crate::types::Message;
use std::fmt::Write;
use std::sync::Arc;

/// Builder for constructing prompts with context
#[derive(Debug, Default)]
pub struct PromptBuilder {
    system: Option<String>,
    context: Vec<Message>,
}

impl PromptBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn system(mut self, prompt: impl Into<String>) -> Self {
        self.system = Some(prompt.into());
        self
    }

    #[must_use]
    pub fn with_context(mut self, messages: Vec<Message>) -> Self {
        self.context = messages;
        self
    }

    pub fn build(self) -> Vec<Message> {
        let mut messages = Vec::new();
        if let Some(system) = self.system {
            messages.push(Message::system(system));
        }
        messages.extend(self.context);
        messages
    }
}

/// Builder for system prompts with skill integration
#[derive(Debug, Default)]
pub struct SystemPromptBuilder<'a> {
    base_prompt: Option<&'a str>,
    skills: &'a [Arc<Skill>],
}

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

    pub fn build(self) -> String {
        let base = self
            .base_prompt
            .unwrap_or("You are a helpful AI coding assistant.")
            .trim();

        if self.skills.is_empty() {
            base.to_string()
        } else {
            let mut prompt = base.to_string();
            prompt.push_str("\n\n## Available Skills\n\n");
            for skill in self.skills {
                let location = skill.source_path.to_string_lossy();
                let _ = write!(
                    prompt,
                    r#"<skill name="{}" location="{}">"#,
                    skill.name, location
                );
                // Use CDATA to safely embed skill content
                prompt.push_str(&skill.description);
                prompt.push_str("</skill>\n");
            }
            let _ = prompt.pop(); // Remove last newline
            prompt
        }
    }
}

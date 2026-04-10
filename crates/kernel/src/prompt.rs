use crate::types::Message;

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

use crate::event::{AgentEvent, Event};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{AgentId, KernelError, Result, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

pub const ASK_USER_TOOL_NAME: &str = "askUser";

/// A single option presented to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    pub description: String,
    /// Optional preview content (e.g. ASCII mockups, code snippets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// A single question with 2–4 mutually exclusive (or multi-select) options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestion {
    pub question: String,
    /// Short chip / tag displayed in the UI (max 12 chars).
    pub header: String,
    pub options: Vec<AskOption>,
    #[serde(default = "default_false")]
    pub multi_select: bool,
}

fn default_false() -> bool {
    false
}

/// The full input payload the model produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserInput {
    pub questions: Vec<AskQuestion>,
    /// Pre-populated answers (usually empty, filled by the UI layer).
    #[serde(default)]
    pub answers: HashMap<String, String>,
}

/// Response from the user after answering the questions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserResponse {
    pub answers: HashMap<String, String>,
}

/// Shared state for ask-user requests across a session.
#[derive(Clone)]
pub struct AskUserState {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<AskUserResponse>>>>,
}

impl AskUserState {
    /// Create new shared ask-user state.
    pub fn new() -> (Self, AskUserResponder) {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let responder = AskUserResponder {
            pending: Arc::clone(&pending),
        };
        let state = Self { pending };
        (state, responder)
    }

    /// Create a responder for this state.
    pub fn create_responder(&self) -> AskUserResponder {
        AskUserResponder {
            pending: Arc::clone(&self.pending),
        }
    }
}

/// Responder used by the session / TUI to deliver user answers.
#[derive(Clone, Debug)]
pub struct AskUserResponder {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<AskUserResponse>>>>,
}

impl AskUserResponder {
    /// Respond to an ask-user request.
    /// Returns `true` if the request existed and the response was delivered.
    pub async fn respond(&self, req_id: &str, response: AskUserResponse) -> bool {
        let mut pending = self.pending.lock().await;
        pending.remove(req_id).map_or_else(
            || {
                tracing::warn!("AskUser response for unknown/timed out req_id: {}", req_id);
                false
            },
            |sender| {
                if sender.send(response).is_err() {
                    tracing::warn!("AskUser response receiver dropped for req_id={}", req_id);
                    return false;
                }
                tracing::info!("AskUser response delivered for req_id={}", req_id);
                true
            },
        )
    }
}

/// Tool that blocks until the user answers a set of multiple-choice questions.
pub struct AskUserTool {
    agent_id: AgentId,
    event_tx: mpsc::Sender<Event>,
    ask_user_state: AskUserState,
}

impl AskUserTool {
    pub fn new(
        agent_id: AgentId,
        event_tx: mpsc::Sender<Event>,
        ask_user_state: AskUserState,
    ) -> Self {
        Self {
            agent_id,
            event_tx,
            ask_user_state,
        }
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        ASK_USER_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Ask the user one or more multiple-choice questions to gather preferences, clarify ambiguity, or make decisions."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "description": "Questions to ask the user (1-4 questions).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The complete question to ask. Should be clear, specific, and end with a question mark."
                            },
                            "header": {
                                "type": "string",
                                "description": "Very short label displayed as a chip/tag (max 12 chars). Examples: 'Auth method', 'Library', 'Approach'."
                            },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "description": "Available choices. Must be 2-4 distinct options. Do not include an 'Other' option — it is provided automatically.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Display text for this option (1-5 words)."
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "Explanation of what this option means or what will happen if chosen."
                                        }
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multi_select": {
                                "type": "boolean",
                                "default": false,
                                "description": "Allow selecting multiple options instead of just one."
                            }
                        },
                        "required": ["question", "header", "options"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // Parse and validate input
        let input: AskUserInput = serde_json::from_value(args)
            .map_err(|e| KernelError::tool(format!("Invalid ask_user arguments: {e}")))?;

        // Validate uniqueness
        let question_texts: Vec<&str> = input
            .questions
            .iter()
            .map(|q| q.question.as_str())
            .collect();
        if question_texts.len()
            != question_texts
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        {
            return Err(KernelError::tool("Question texts must be unique"));
        }
        for q in &input.questions {
            let labels: Vec<&str> = q.options.iter().map(|o| o.label.as_str()).collect();
            if labels.len()
                != labels
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
            {
                return Err(KernelError::tool(format!(
                    "Option labels must be unique within question '{}'",
                    q.question
                )));
            }
        }

        // If answers are already provided (e.g. pre-filled by the UI layer),
        // return them immediately without blocking.
        if input.answers.len() == input.questions.len() {
            let text = format_answers(&input.answers);
            return Ok(ToolOutput::text(text));
        }

        // Otherwise, emit an event and wait for the user to respond.
        let req_id = Uuid::now_v7().to_string();
        let (tx, rx) = oneshot::channel::<AskUserResponse>();

        {
            let mut pending = self.ask_user_state.pending.lock().await;
            pending.insert(req_id.clone(), tx);
        }

        self.event_tx
            .send(Event::Agent(AgentEvent::AskUserQuestion {
                agent_id: self.agent_id.clone(),
                req_id: req_id.clone(),
                questions: input.questions,
            }))
            .await
            .map_err(|e| KernelError::io(format!("Failed to send AskUserQuestion event: {e}")))?;

        tracing::info!(
            "AskUserQuestion sent with req_id={} for agent {}",
            req_id,
            self.agent_id
        );

        // Wait for response (5-minute timeout to avoid hanging forever)
        match tokio::time::timeout(std::time::Duration::from_mins(5), rx).await {
            Ok(Ok(response)) => {
                let text = format_answers(&response.answers);
                Ok(ToolOutput::text(text))
            }
            Ok(Err(_)) => {
                self.ask_user_state.pending.lock().await.remove(&req_id);
                Ok(ToolOutput::error(
                    "User response channel closed unexpectedly".to_string(),
                ))
            }
            Err(_) => {
                self.ask_user_state.pending.lock().await.remove(&req_id);
                tracing::warn!("AskUser request {} timed out (5 min)", req_id);
                Ok(ToolOutput::error(
                    "Ask user request timed out (5 minutes)".to_string(),
                ))
            }
        }
    }
}

fn format_answers(answers: &HashMap<String, String>) -> String {
    let parts: Vec<String> = answers
        .iter()
        .map(|(question, answer)| format!("Q:{question}\nA:{answer}"))
        .collect();
    if parts.is_empty() {
        "User declined to answer questions.".to_string()
    } else {
        format!("User answered your questions:\n\n{}\n\nYou can now continue with the user's answers in mind.", parts.join("\n\n"))
    }
}

impl Default for AskUserState {
    fn default() -> Self {
        let (state, _) = Self::new();
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_answers() {
        let mut answers = HashMap::new();
        answers.insert("Which library?".to_string(), "chrono".to_string());

        let text = format_answers(&answers);
        assert!(text.contains("chrono"));
        assert!(text.contains("Which library?"));
    }
}

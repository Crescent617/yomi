use crate::agent::AgentInput;
use crate::comms::EventBusHandle;
use crate::event::{AgentEvent, Event};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub const ASK_USER_TOOL_NAME: &str = "ask_user";

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

/// Tool that blocks until the user answers a set of multiple-choice questions.
pub struct AskUserTool {
    event_bus: EventBusHandle,
    input_bus: Arc<crate::comms::InputBus>,
}

impl AskUserTool {
    pub fn new(event_bus: EventBusHandle, input_bus: Arc<crate::comms::InputBus>) -> Self {
        Self {
            event_bus,
            input_bus,
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

        let req_id = ulid::Ulid::new().to_string();
        let session_id = crate::types::SessionId::from(_ctx.session_id.clone());
        let session_id_str = session_id.0.to_string();

        // Subscribe to input bus BEFORE emitting the event to avoid missing the response.
        let mut subscriber = self.input_bus.subscribe(session_id.clone());

        self.event_bus
            .send(crate::event::Envelope::new(
                session_id.clone(),
                Event::Agent(AgentEvent::AskUserQuestion {
                    req_id: req_id.clone(),
                    session_id: session_id_str,
                    questions: input.questions,
                }),
            ))
            .await
            .map_err(|e| KernelError::io(format!("Failed to send AskUserQuestion event: {e}")))?;

        tracing::info!("AskUserQuestion sent with req_id={}", req_id);

        // Wait for response via input bus (2-minute timeout)
        let result = tokio::time::timeout(Duration::from_mins(2), async {
            while let Some((_, input)) = subscriber.recv().await {
                if let AgentInput::AskUserResponse {
                    req_id: id,
                    response,
                } = input
                {
                    if id == req_id {
                        return response;
                    }
                }
            }
            AskUserResponse {
                answers: HashMap::new(),
            }
        })
        .await;

        let result = match result {
            Ok(response) => Ok(ToolOutput::text(format_answers(&response.answers))),
            Err(_) => {
                tracing::warn!("AskUser request {} timed out (2 min)", req_id);
                Ok(ToolOutput::error(
                    "Ask user request timed out (2 minutes)".to_string(),
                ))
            }
        };

        let _ = self
            .event_bus
            .send(crate::event::Envelope::new(
                session_id.clone(),
                Event::Agent(AgentEvent::AskUserAck {
                    req_id: req_id.clone(),
                }),
            ))
            .await;

        result
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

#[cfg(test)]
#[path = "ask_user_test.rs"]
mod tests;

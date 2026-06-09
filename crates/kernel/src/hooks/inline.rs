use super::{HookContext, HookEvent, HookHandler, HookResult};
use crate::types::Result;
use async_trait::async_trait;
use serde_json::Value;

/// Lightweight declarative hook handler.
/// No external process spawn — rules are evaluated in-process.
#[derive(Debug)]
pub struct InlineHookHandler {
    name: String,
    events: Vec<HookEvent>,
    matcher: regex::Regex,
    rule: InlineRule,
}

#[derive(Debug, Clone)]
pub enum InlineRule {
    /// Block if any pattern matches the tool input/output text.
    Block {
        patterns: Vec<String>,
        message: String,
    },
    /// Append text after tool output.
    Append { text: String },
    /// Modify tool input by replacing a pattern.
    Replace { from: String, to: String },
}

impl InlineHookHandler {
    pub fn new(
        name: impl Into<String>,
        event: HookEvent,
        matcher: impl AsRef<str>,
        rule: InlineRule,
    ) -> Result<Self> {
        Ok(Self {
            name: name.into(),
            events: vec![event],
            matcher: super::case_insensitive_regex(matcher.as_ref())?,
            rule,
        })
    }
}

#[async_trait]
impl HookHandler for InlineHookHandler {
    fn name(&self) -> &str {
        &self.name
    }

    fn events(&self) -> &[HookEvent] {
        &self.events
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookResult> {
        if !ctx.tool_matches(&self.matcher) {
            return Ok(HookResult::Passthrough);
        }
        match (&self.rule, ctx.event) {
            (InlineRule::Block { patterns, message }, HookEvent::PreToolUse) => {
                let input_json = ctx
                    .tool_input
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                for pat in patterns {
                    if input_json.contains(pat) {
                        return Ok(HookResult::PreTool(super::PreToolDecision {
                            action: super::PreToolAction::Block,
                            reason: Some(message.clone()),
                            ..Default::default()
                        }));
                    }
                }
                Ok(HookResult::Passthrough)
            }
            (InlineRule::Append { text }, HookEvent::PostToolUse) => {
                Ok(HookResult::PostTool(super::PostToolDecision {
                    append_output: Some(text.clone()),
                    ..Default::default()
                }))
            }
            (InlineRule::Replace { from, to }, HookEvent::PreToolUse) => {
                if let Some(input) = ctx.tool_input.clone() {
                    let input_str = input.to_string();
                    if input_str.contains(from) {
                        let new_str = input_str.replace(from, to);
                        let new_value: Value = serde_json::from_str(&new_str).unwrap_or(input);
                        return Ok(HookResult::PreTool(super::PreToolDecision {
                            action: super::PreToolAction::Allow,
                            updated_input: Some(new_value),
                            ..Default::default()
                        }));
                    }
                }
                Ok(HookResult::Passthrough)
            }
            _ => Ok(HookResult::Passthrough),
        }
    }
}

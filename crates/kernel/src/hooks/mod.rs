//! Hook system for Yomi agent lifecycle events.
//!
//! Inspired by Claude Code and Codex hooks, but integrated natively
//! into the Rust codebase for zero-overhead when not in use.

use crate::types::ToolOutput;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub mod command;
pub mod goal;
pub mod inline;
pub mod registry;
pub mod skill;

pub use command::CommandHookHandler;
pub use goal::GoalPreStopHandler;
pub use inline::InlineHookHandler;
pub use registry::HookRegistry;
pub use skill::SkillHookHandler;

/// Lifecycle events that can trigger hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Before a tool is executed (after permission check).
    #[default]
    #[serde(alias = "PreToolUse")]
    PreToolUse,
    /// After a tool has executed, before result is committed to message buffer.
    #[serde(alias = "PostToolUse")]
    PostToolUse,
    /// When the agent is about to stop after a streaming turn (no tool calls).
    /// Hook can decide to continue the session by injecting steer messages.
    #[serde(alias = "PreStop")]
    PreStop,
}

/// Context passed to every hook handler.
///
/// Serialized as camelCase to match Claude Code / Codex hook conventions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HookContext {
    pub event: HookEvent,
    pub session_id: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub cwd: PathBuf,
    /// Tool input arguments (`PreToolUse` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    /// Tool execution result (`PostToolUse` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<HookToolOutput>,
}

/// Serializable subset of `ToolOutput` for hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl From<&ToolOutput> for HookToolOutput {
    fn from(o: &ToolOutput) -> Self {
        Self {
            text: o.text_content(),
            is_error: !o.success(),
        }
    }
}

/// Decision returned by a `PreToolUse` hook.
///
/// Compatible with Claude Code / Codex hook output schema.
/// `permissionDecision` is accepted as an alias for `action`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PreToolDecision {
    /// What to do with the tool call (`allow` or `block`).
    #[serde(default, alias = "permissionDecision")]
    pub action: PreToolAction,
    /// Reason shown when blocking.
    #[serde(
        skip_serializing_if = "Option::is_none",
        alias = "permissionDecisionReason"
    )]
    pub reason: Option<String>,
    /// Replacement arguments — can be set regardless of action value.
    /// Claude Code convention: `permissionDecision: "allow"` + `updatedInput`.
    #[serde(skip_serializing_if = "Option::is_none", alias = "updatedInput")]
    pub updated_input: Option<Value>,
    /// Extra context injected into the conversation as an independent message
    /// (aligned with Claude Code `additionalContext`).
    #[serde(skip_serializing_if = "Option::is_none", alias = "additionalContext")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreToolAction {
    /// Let the tool call proceed as-is (optionally with `updated_input`).
    #[default]
    Allow,
    /// Block the tool call entirely.
    #[serde(alias = "deny")]
    Block,
}

/// Decision returned by a `PostToolUse` hook.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PostToolDecision {
    /// If false, the session stops after this turn.
    #[serde(default = "default_true")]
    pub continue_session: bool,
    /// Replacement text for the tool output.
    #[serde(skip_serializing_if = "Option::is_none", alias = "updatedOutput")]
    pub updated_output: Option<String>,
    /// Text appended after the original output.
    #[serde(skip_serializing_if = "Option::is_none", alias = "appendOutput")]
    pub append_output: Option<String>,
    /// Extra context injected into the conversation as an independent message
    /// (aligned with Claude Code `additionalContext`).
    #[serde(skip_serializing_if = "Option::is_none", alias = "additionalContext")]
    pub context: Option<String>,
}

/// Decision returned by a `PreStop` hook.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PreStopDecision {
    /// If true, the agent continues to another streaming turn instead of stopping.
    #[serde(default = "default_true")]
    pub continue_session: bool,
    /// Extra steer messages injected into the conversation before the next streaming turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steer_blocks: Option<Vec<crate::types::ContentBlock>>,
}

fn default_true() -> bool {
    true
}

/// Unified result type for hook handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookResult {
    PreTool(PreToolDecision),
    PostTool(PostToolDecision),
    PreStop(PreStopDecision),
    /// No decision; passthrough.
    Passthrough,
}

/// Core trait for hook handlers.
#[async_trait]
pub trait HookHandler: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Which events this handler cares about.
    fn events(&self) -> &[HookEvent];

    /// Execute the hook.
    async fn run(&self, ctx: &HookContext) -> crate::types::Result<HookResult>;
}

impl HookContext {
    /// Build a `PreToolUse` context.
    pub fn pre_tool(
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
        cwd: impl Into<PathBuf>,
        tool_input: Value,
    ) -> Self {
        Self {
            event: HookEvent::PreToolUse,
            session_id: session_id.into(),
            tool_name: Some(tool_name.into()),
            tool_call_id: Some(tool_call_id.into()),
            cwd: cwd.into(),
            tool_input: Some(tool_input),
            ..Default::default()
        }
    }

    /// Build a `PostToolUse` context.
    pub fn post_tool(
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
        cwd: impl Into<PathBuf>,
        tool_output: &ToolOutput,
    ) -> Self {
        Self {
            event: HookEvent::PostToolUse,
            session_id: session_id.into(),
            tool_name: Some(tool_name.into()),
            tool_call_id: Some(tool_call_id.into()),
            cwd: cwd.into(),
            tool_output: Some(tool_output.into()),
            ..Default::default()
        }
    }

    /// Build a `PreStop` context.
    pub fn pre_stop(session_id: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            event: HookEvent::PreStop,
            session_id: session_id.into(),
            cwd: cwd.into(),
            ..Default::default()
        }
    }

    /// Check whether a regex pattern matches this tool.
    ///
    /// The regex is expected to be case-insensitive (see `case_insensitive_regex`).
    /// In addition to the raw tool name, known aliases are checked so that
    /// a pattern like `"Bash"` also matches Yomi's `shell` tool.
    pub fn tool_matches(&self, pattern: &regex::Regex) -> bool {
        if let Some(ref name) = self.tool_name {
            if pattern.is_match(name) {
                return true;
            }
            let alias = tool_alias(&name.to_lowercase());
            !alias.is_empty() && pattern.is_match(alias)
        } else {
            false
        }
    }
}

/// Return the Claude Code / Codex alias for a Yomi tool name.
/// Only names that differ between ecosystems are listed.
fn tool_alias(name: &str) -> &'static str {
    match name {
        "shell" => "bash",
        "agent" => "subagent",
        _ => "",
    }
}

/// Build a case-insensitive regex for hook matchers.
pub(crate) fn case_insensitive_regex(pat: &str) -> crate::types::Result<regex::Regex> {
    regex::RegexBuilder::new(pat)
        .case_insensitive(true)
        .build()
        .map_err(|e| crate::types::KernelError::config(format!("Invalid hook matcher: {e}")))
}

pub(crate) fn default_timeout() -> u64 {
    30
}

/// Hook entry for global configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    pub name: String,
    pub event: HookEvent,
    pub matcher: String,
    #[serde(default, rename = "type")]
    pub handler_type: String,
    /// Shell command to execute (always run through `sh -c` / `cmd /C`).
    pub command: String,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

/// Build a `HookRegistry` from global configuration entries.
pub fn build_registry(entries: &[HookEntry], allow_commands: bool) -> HookRegistry {
    let mut registry = HookRegistry::new();
    let mut seen = std::collections::HashSet::new();

    for entry in entries {
        if !seen.insert(&entry.name) {
            tracing::warn!(
                "Duplicate hook name '{}', skipping subsequent definitions",
                entry.name
            );
            continue;
        }
        match entry.handler_type.as_str() {
            "command" | "" => {
                if !allow_commands {
                    tracing::warn!(
                        "Command hook '{}' blocked: hooks feature is disabled",
                        entry.name
                    );
                    continue;
                }
                let Ok(handler) = CommandHookHandler::new(
                    &entry.name,
                    entry.event,
                    &entry.matcher,
                    &entry.command,
                ) else {
                    tracing::warn!("Invalid matcher for hook '{}'", entry.name);
                    continue;
                };
                let handler = handler.with_timeout(entry.timeout);
                registry.register(std::sync::Arc::new(handler));
                tracing::info!("Registered user hook: {} ({:?})", entry.name, entry.event);
            }
            other => {
                tracing::warn!("Unsupported hook type '{}' for '{}'", other, entry.name);
            }
        }
    }

    registry
}

/// Build a `HookRegistry` from a shared base (config hooks) plus skill-level hooks
/// and an optional goal store.
///
/// This is used at agent spawn time so the registry construction logic stays in one place.
/// The optional `base` parameter allows extending a pre-built base registry (e.g., for
/// workspace-level hooks that are discovered per-session).
pub async fn build_hook_registry_with_skills(
    base: Option<&HookRegistry>,
    skills: &[std::sync::Arc<crate::skill::Skill>],
    allow_commands: bool,
    goal_store: Option<std::sync::Arc<dyn crate::goal::GoalStore>>,
    background_tasks: std::sync::Arc<crate::agent::BgTaskTracker>,
) -> HookRegistry {
    let mut registry = match base {
        Some(h) => h.clone(),
        None => HookRegistry::default(),
    };

    if base.is_some() {
        for skill in skills {
            if let Some(ref hooks_value) = skill.hooks {
                if let Err(e) = SkillHookHandler::load_and_register_from_value(
                    &skill.name,
                    hooks_value,
                    &mut registry,
                    allow_commands,
                ) {
                    tracing::warn!("Failed to load hooks for skill '{}': {}", skill.name, e);
                }
            }
        }
    }

    // Register the built-in goal pre-stop hook if a goal store is available.
    if let Some(store) = goal_store {
        registry.register(std::sync::Arc::new(GoalPreStopHandler::new(
            store,
            background_tasks,
        )));
    }

    registry
}

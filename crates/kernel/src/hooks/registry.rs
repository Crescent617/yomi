use super::{HookContext, HookEvent, HookHandler, HookResult};
use std::sync::Arc;

/// Registry of hook handlers.
///
/// Handlers are evaluated in registration order. All matching handlers run;
/// the caller merges decisions according to the event semantics.
#[derive(Default)]
pub struct HookRegistry {
    handlers: Vec<Arc<dyn HookHandler>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Register a hook handler.
    pub fn register(&mut self, handler: Arc<dyn HookHandler>) {
        tracing::debug!("Registered hook handler: {}", handler.name());
        self.handlers.push(handler);
    }

    /// Find all handlers that match the given event and context.
    pub fn matching(&self, event: HookEvent, ctx: &HookContext) -> Vec<Arc<dyn HookHandler>> {
        self.handlers
            .iter()
            .filter(|h| h.events().contains(&event) && h.matches(ctx))
            .cloned()
            .collect()
    }

    /// Convenience: run all matching `PreToolUse` hooks and merge decisions.
    /// Returns (`final_decision`, `context_messages`).
    pub async fn run_pre_tool(&self, ctx: &HookContext) -> (HookResult, Vec<String>) {
        let handlers = self.matching(HookEvent::PreToolUse, ctx);
        if handlers.is_empty() {
            return (HookResult::Passthrough, Vec::new());
        }

        let mut decision = super::PreToolDecision {
            action: super::PreToolAction::Allow,
            ..Default::default()
        };
        let mut contexts = Vec::new();

        for h in handlers {
            tracing::debug!("Running PreToolUse hook: {}", h.name());
            match h.run(ctx).await {
                Ok(HookResult::PreTool(d)) => {
                    if matches!(d.action, super::PreToolAction::Block) {
                        decision.action = super::PreToolAction::Block;
                        decision.reason = d.reason.or(decision.reason);
                        // Block clears any previously collected updated_input.
                        decision.updated_input = None;
                    } else if d.updated_input.is_some() {
                        // Allow (or passthrough) with updated_input — last writer wins
                        // unless already blocked.
                        decision.updated_input = d.updated_input;
                    }
                    if let Some(ctx) = d.context {
                        contexts.push(ctx);
                    }
                }
                Err(e) => {
                    tracing::warn!("Hook '{}' failed: {}", h.name(), e);
                }
                _ => {}
            }
        }

        (HookResult::PreTool(decision), contexts)
    }

    /// Convenience: run all matching `PostToolUse` hooks and merge decisions.
    /// Returns (`final_decision`, `context_messages`).
    pub async fn run_post_tool(&self, ctx: &HookContext) -> (HookResult, Vec<String>) {
        let handlers = self.matching(HookEvent::PostToolUse, ctx);
        if handlers.is_empty() {
            return (HookResult::Passthrough, Vec::new());
        }

        let mut decision = super::PostToolDecision::default();
        let mut contexts = Vec::new();

        for h in handlers {
            tracing::debug!("Running PostToolUse hook: {}", h.name());
            match h.run(ctx).await {
                Ok(HookResult::PostTool(d)) => {
                    if !d.continue_session {
                        decision.continue_session = false;
                    }
                    if d.updated_output.is_some() {
                        decision.updated_output = d.updated_output;
                    }
                    if let Some(append) = d.append_output {
                        decision.append_output =
                            Some(decision.append_output.unwrap_or_default() + &append);
                    }
                    if let Some(ctx) = d.context {
                        contexts.push(ctx);
                    }
                }
                Err(e) => {
                    tracing::warn!("Hook '{}' failed: {}", h.name(), e);
                }
                _ => {}
            }
        }

        (HookResult::PostTool(decision), contexts)
    }
}

impl Clone for HookRegistry {
    fn clone(&self) -> Self {
        Self {
            handlers: self.handlers.clone(),
        }
    }
}

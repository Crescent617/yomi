use super::{HookEvent, HookHandler, HookRegistry, HookResult, InlineHookHandler};
use crate::hooks::inline::InlineRule;
use crate::types::Result;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;

/// Declarative hook config inside a SKILL.md frontmatter.
#[derive(Debug, Deserialize)]
pub struct SkillHookConfig {
    pub event: HookEvent,
    pub matcher: String,
    #[serde(rename = "type", default)]
    pub handler_type: String,
    /// Block patterns (for inline block rules).
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Message shown when blocking.
    #[serde(default)]
    pub message: String,
    /// Append text (for inline append rules).
    #[serde(default)]
    pub append: String,
    /// Replace pattern (for inline replace rules).
    #[serde(default)]
    pub replace_from: String,
    /// Replacement text (for inline replace rules).
    #[serde(default)]
    pub replace_to: String,
    /// Command to run (for command hooks declared in skill).
    #[serde(default)]
    pub command: String,
    /// Timeout in seconds.
    #[serde(default = "super::default_timeout")]
    pub timeout: u64,
}

/// A hook handler that wraps an inline rule loaded from a skill.
pub struct SkillHookHandler {
    name: String,
    inner: Arc<dyn HookHandler>,
}

impl SkillHookHandler {
    pub fn from_config(skill_name: &str, idx: usize, cfg: SkillHookConfig) -> Result<Self> {
        let name = format!("{skill_name}::hook-{idx}");
        let event = cfg.event;
        let matcher = &cfg.matcher;

        let inner: Arc<dyn HookHandler> = match cfg.handler_type.as_str() {
            "inline" | "" => {
                if !cfg.patterns.is_empty() {
                    Arc::new(InlineHookHandler::new(
                        name.clone(),
                        event,
                        matcher,
                        InlineRule::Block {
                            patterns: cfg.patterns,
                            message: if cfg.message.is_empty() {
                                "Blocked by skill".to_string()
                            } else {
                                cfg.message
                            },
                        },
                    )?)
                } else if !cfg.append.is_empty() {
                    Arc::new(InlineHookHandler::new(
                        name.clone(),
                        event,
                        matcher,
                        InlineRule::Append { text: cfg.append },
                    )?)
                } else if !cfg.replace_from.is_empty() {
                    Arc::new(InlineHookHandler::new(
                        name.clone(),
                        event,
                        matcher,
                        InlineRule::Replace {
                            from: cfg.replace_from,
                            to: cfg.replace_to,
                        },
                    )?)
                } else {
                    return Err(crate::types::KernelError::skill(format!(
                        "Skill hook '{name}' has no rule (patterns, append, or replace)"
                    )));
                }
            }
            "command" => Arc::new(
                super::CommandHookHandler::new(name.clone(), event, matcher, &cfg.command)?
                    .with_timeout(cfg.timeout),
            ),
            other => {
                return Err(crate::types::KernelError::skill(format!(
                    "Unknown skill hook type: {other}"
                )))
            }
        };

        Ok(Self { name, inner })
    }

    /// Parse hooks from raw YAML string and register them into a registry.
    pub fn load_and_register(
        skill_name: &str,
        yaml: &str,
        registry: &mut HookRegistry,
    ) -> Result<()> {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml)
            .map_err(|e| crate::types::KernelError::skill(format!("Invalid hooks YAML: {e}")))?;
        Self::load_and_register_from_value(skill_name, &value, registry)
    }

    /// Parse hooks from a YAML value and register them into a registry.
    pub fn load_and_register_from_value(
        skill_name: &str,
        value: &serde_yaml::Value,
        registry: &mut HookRegistry,
    ) -> Result<()> {
        let configs: Vec<SkillHookConfig> = serde_yaml::from_value(value.clone())
            .map_err(|e| crate::types::KernelError::skill(format!("Invalid hooks YAML: {e}")))?;
        for (idx, cfg) in configs.into_iter().enumerate() {
            let handler = Self::from_config(skill_name, idx, cfg)?;
            registry.register(Arc::new(handler));
        }
        Ok(())
    }
}

#[async_trait]
impl HookHandler for SkillHookHandler {
    fn name(&self) -> &str {
        &self.name
    }

    fn events(&self) -> &[HookEvent] {
        self.inner.events()
    }

    fn matches(&self, ctx: &super::HookContext) -> bool {
        self.inner.matches(ctx)
    }

    async fn run(&self, ctx: &super::HookContext) -> Result<HookResult> {
        self.inner.run(ctx).await
    }
}

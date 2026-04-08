use crate::types::{ToolDefinition, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Core trait for tools
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, args: Value) -> Result<ToolOutput>;
    fn requires_confirmation(&self) -> bool {
        true
    }
    async fn is_allowed(&self, _args: &Value) -> Result<bool> {
        Ok(true)
    }
}

/// Tool registry - manages available tools
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters_schema(),
            })
            .collect()
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
}

/// Tool sandbox for permission management
#[derive(Clone, Default)]
pub struct ToolSandbox {
    enabled: bool,
    require_confirmation: HashMap<String, bool>,
    yolo_mode: bool,
}

impl ToolSandbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn enable(mut self) -> Self {
        self.enabled = true;
        self
    }

    pub const fn yolo(mut self) -> Self {
        self.yolo_mode = true;
        self
    }

    pub fn set_confirmation(&mut self, tool_name: &str, required: bool) {
        self.require_confirmation
            .insert(tool_name.to_string(), required);
    }

    pub fn needs_confirmation(&self, tool_name: &str, tool_requires: bool) -> bool {
        if !self.enabled || self.yolo_mode {
            return false;
        }
        if let Some(&required) = self.require_confirmation.get(tool_name) {
            return required;
        }
        tool_requires
    }

    pub const fn default_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(30)
    }
}

// parallel execution is re-exported from tools module

/// Global YOLO mode flag
static YOLO_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn enable_yolo_mode() {
    YOLO_MODE.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub fn is_yolo_mode() -> bool {
    YOLO_MODE.load(std::sync::atomic::Ordering::SeqCst)
}

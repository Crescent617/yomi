use crate::task::{SharedTaskStore, TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use crate::types::{ToolDefinition, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub mod bash;
pub mod edit;
pub mod edit_utils;
pub mod file_state;
pub mod line_numbers;
pub mod parallel;
pub mod read;
pub mod subagent;

pub use bash::{BashTool, BashToolCtx};
pub use edit::EditTool;
pub use parallel::execute_tools_parallel;
pub use read::ReadTool;
pub use subagent::SubAgentTool;

/// Core trait for tools
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn desc(&self) -> &str;
    fn params(&self) -> Value;
    async fn exec(&self, args: Value) -> Result<ToolOutput>;
}

/// Tool registry - manages available tools for an agent
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}


impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool (mutable because registry is built during agent initialization)
    pub fn register(&mut self, tool: impl Tool + 'static) {
        let name = tool.name().to_string();
        self.tools.insert(name, Arc::new(tool));
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let defs: Vec<ToolDefinition> = self
            .tools
            .values()
            .map(|tool| ToolDefinition {
                name: tool.name().to_string(),
                description: tool.desc().to_string(),
                parameters: tool.params(),
            })
            .collect();
        tracing::debug!(
            "ToolRegistry.definitions() returning {} tools: {:?}",
            defs.len(),
            defs.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
        defs
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
}

/// Global YOLO mode flag
static YOLO_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn enable_yolo_mode() {
    YOLO_MODE.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub fn is_yolo_mode() -> bool {
    YOLO_MODE.load(std::sync::atomic::Ordering::SeqCst)
}

impl ToolRegistry {
    pub fn register_task_tools(&mut self, store: SharedTaskStore, task_list_id: String) {
        self.register(TaskCreateTool::new(store.clone(), task_list_id.clone()));
        self.register(TaskUpdateTool::new(store.clone(), task_list_id.clone()));
        self.register(TaskListTool::new(store.clone(), task_list_id.clone()));
        self.register(TaskGetTool::new(store, task_list_id));
    }
}

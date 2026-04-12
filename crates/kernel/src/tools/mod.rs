use crate::types::{ToolDefinition, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub mod bash;
pub mod edit;
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

/// Tool registry - manages available tools
#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.write().unwrap().insert(name, tool);
    }

    pub fn unregister(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.write().unwrap().remove(name)
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.read().unwrap().get(name).cloned()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let defs: Vec<ToolDefinition> = self
            .tools
            .read()
            .unwrap()
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
        self.tools.read().unwrap().keys().cloned().collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.read().unwrap().contains_key(name)
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

use crate::task::{SharedTaskStore, TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};

impl ToolRegistry {
    pub fn register_task_tools<F>(
        &self,
        store: SharedTaskStore,
        get_session_id: F,
    )
    where
        F: Fn() -> String + Send + Sync + Clone + 'static,
    {
        self.register(Arc::new(TaskCreateTool::new(
            store.clone(),
            get_session_id.clone(),
        )));
        self.register(Arc::new(TaskUpdateTool::new(
            store.clone(),
            get_session_id.clone(),
        )));
        self.register(Arc::new(TaskListTool::new(
            store.clone(),
            get_session_id.clone(),
        )));
        self.register(Arc::new(TaskGetTool::new(
            store,
            get_session_id,
        )));
    }
}

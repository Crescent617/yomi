//! yomi-core - Core library for the yomi AI coding assistant

/// Environment variable prefix - change this to rebrand the entire CLI
/// Default: "YOMI_" (produces env vars like `YOMI_API_KEY`)
pub const ENV_PREFIX: &str = "YOMI_";

/// Compile-time string concatenation for env var names
/// Usage: `env_name!("API_KEY")` expands to `"YOMI_API_KEY"`
#[macro_export]
macro_rules! env_name {
    ($suffix:expr) => {
        std::concat!("YOMI_", $suffix)
    };
}

pub mod agent;
pub mod config;
pub mod event;
pub mod prompt;
pub mod provider;
pub mod storage;
pub mod tool;
pub mod types;

#[cfg(feature = "providers")]
pub mod providers;
#[cfg(feature = "tools")]
pub mod tools;

// Re-export commonly used types
pub use config::{env_names, expand_tilde, Config, ModelProvider, DEFAULT_DATA_DIR};
pub use event::{
    AgentEvent, AgentResult, ContentChunk, Event, ModelEvent, ProgressUpdate, SystemEvent,
    ToolEvent, UserEvent,
};
pub use prompt::PromptBuilder;
pub use provider::{
    ModelConfig, ModelStream, ModelStreamItem, RetryingProvider, ThinkingConfig, ToolCallRequest,
};
pub use storage::{Storage, StorageConfig};
pub use tool::{enable_yolo_mode, is_yolo_mode, Tool, ToolRegistry, ToolSandbox};
pub use types::*;

// Conditional re-exports for convenience
#[cfg(feature = "providers")]
pub use providers::{AnthropicProvider, OpenAIProvider};
#[cfg(feature = "storage")]
pub use storage::sqlite::SqliteStorage;
#[cfg(feature = "tools")]
pub use tools::{execute_tools_parallel, BashTool, FileTool};

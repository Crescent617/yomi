//! nekoclaw-core - Core library for the nekoclaw AI coding assistant

/// Environment variable prefix - change this to rebrand the entire CLI
/// Default: "NEKOCLAW_" (produces env vars like `NEKOCLAW_API_KEY`)
pub const ENV_PREFIX: &str = "NEKOCLAW_";

/// Compile-time string concatenation for env var names
/// Usage: `env_name!("API_KEY")` expands to `"NEKOCLAW_API_KEY"`
#[macro_export]
macro_rules! env_name {
    ($suffix:expr) => {
        std::concat!("NEKOCLAW_", $suffix)
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
pub use config::{Config, ModelProvider, DEFAULT_DATA_DIR, env_names};
pub use event::{AgentEvent, AgentResult, ContentChunk, Event, ModelEvent, ProgressUpdate, SystemEvent, ToolEvent, UserEvent};
pub use provider::{ModelConfig, ModelStream, ModelStreamItem, RetryingProvider, ThinkingConfig, ToolCallRequest};
pub use storage::{Storage, StorageConfig};
pub use tool::{Tool, ToolRegistry, ToolSandbox, enable_yolo_mode, is_yolo_mode};
pub use prompt::PromptBuilder;
pub use types::*;

// Conditional re-exports for convenience
#[cfg(feature = "providers")]
pub use providers::{AnthropicProvider, OpenAIProvider};
#[cfg(feature = "storage")]
pub use storage::sqlite::SqliteStorage;
#[cfg(feature = "tools")]
pub use tools::{BashTool, FileTool, execute_tools_parallel};

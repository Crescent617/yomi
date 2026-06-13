#[allow(clippy::module_inception)]
mod agent;
mod cancel;
mod compaction;
mod handle;
mod hooks;
mod interceptor;
mod message_buffer;
mod simple;
mod stream_collector;
mod streaming;
mod turn;
mod types;

pub use compaction::CompactionManager;
pub use streaming::StreamingHandler;
pub use turn::Turn;

pub use agent::{Agent, AgentInput};
pub use cancel::CancelToken;
pub use handle::AgentHandle;
pub use interceptor::{
    InterceptCtx, Interceptors, TodoReminderInterceptor, UserMessageInterceptor,
};
pub use message_buffer::MessageBuffer;
pub use simple::{cancelled_error, is_cancelled_error, ExecuteMetrics, SimpleAgent};
pub use stream_collector::{StreamCollectionResult, StreamCollectorState};
pub use types::{
    AgentConfig, AgentError, AgentExecutionContext, AgentShared, AgentSpawnArgs, AgentState,
    SubAgentMode,
};

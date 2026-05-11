#[allow(clippy::module_inception)]
mod agent;
mod cancel;
mod handle;
mod interceptor;
mod message_buffer;
mod simple;
mod stream_collector;
mod todo_reminder;
mod types;

pub use agent::{Agent, AgentInput};
pub use cancel::CancelToken;
pub use handle::AgentHandle;
pub use interceptor::{InterceptCtx, Interceptors, UserMessageInterceptor};
pub use message_buffer::MessageBuffer;
pub use todo_reminder::TodoReminderInterceptor;
pub use simple::{cancelled_error, is_cancelled_error, ExecuteMetrics, SimpleAgent};
pub use stream_collector::{StreamCollectionResult, StreamCollectorState};
pub use types::{
    AgentConfig, AgentError, AgentExecutionContext, AgentShared, AgentSpawnArgs, AgentState,
    SubAgentMode,
};

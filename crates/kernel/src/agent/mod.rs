#[allow(clippy::module_inception)]
mod agent;
mod bg_task;
mod cancel;
mod interceptor;
mod message_buffer;
mod stream_collector;
mod turn;
mod types;

pub use turn::Turn;

pub use agent::{Agent, AgentInput};
pub use bg_task::{BackgroundShellTask, BackgroundTaskKind, BgTaskGuard, BgTaskTracker};
pub use cancel::{is_cancelled_error, CancelToken};
pub use interceptor::{InterceptCtx, Interceptors, TodoReminderInterceptor, UserMsgInterceptor};
pub use message_buffer::MessageBuffer;
pub use stream_collector::{StreamCollectionResult, StreamCollectorState};
pub use types::{
    AgentConfig, AgentError, AgentExecutionContext, AgentShared, AgentSpawnArgs, AgentState,
    SubAgentMode,
};

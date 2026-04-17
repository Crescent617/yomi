#[allow(clippy::module_inception)]
mod agent;
mod cancel;
mod handle;
mod message_buffer;
mod simple;
mod stream_collector;
mod types;

pub use agent::{Agent, AgentInput};
pub use cancel::CancelToken;
pub use handle::AgentHandle;
pub use simple::{cancelled_error, is_cancelled_error, SimpleAgent};
pub use stream_collector::{StreamCollectionResult, StreamCollectorState};
pub use types::{
    AgentConfig, AgentError, AgentExecutionContext, AgentShared, AgentSpawnArgs, AgentState,
    SubAgentMode,
};

use crate::agent::AgentInput;
use crate::comms::PubSub;
use crate::comms::PubSubSubscriber;
use crate::types::SessionId;

pub type InputBus = PubSub<AgentInput, SessionId>;
pub type InputBusSubscriber = PubSubSubscriber<AgentInput, SessionId>;

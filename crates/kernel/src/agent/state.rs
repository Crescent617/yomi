/// Agent state machine - single threaded, no locks needed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    WaitingForInput,
    Streaming,
    ExecutingTool,
    Completed,
    Failed,
    Cancelled,
}

impl AgentState {
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// 返回从此状态可以合法转移到的目标状态
    pub const fn valid_transitions(&self) -> &'static [Self] {
        match self {
            Self::Idle => &[Self::WaitingForInput],
            Self::WaitingForInput => &[Self::Streaming, Self::Cancelled],
            Self::Streaming => &[
                Self::ExecutingTool,
                Self::WaitingForInput,
                Self::Failed,
                Self::Cancelled,
            ],
            Self::ExecutingTool => &[Self::Streaming, Self::Failed, Self::Cancelled],
            // Terminal states - no valid transitions
            Self::Completed | Self::Failed | Self::Cancelled => &[],
        }
    }

    /// 检查是否可以转换到目标状态
    pub fn can_transition_to(&self, target: Self) -> bool {
        self.valid_transitions().contains(&target)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::WaitingForInput => "waiting_for_input",
            Self::Streaming => "streaming",
            Self::ExecutingTool => "executing_tool",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_can_only_go_to_waiting() {
        assert!(AgentState::Idle.can_transition_to(AgentState::WaitingForInput));
        assert!(!AgentState::Idle.can_transition_to(AgentState::Streaming));
    }

    #[test]
    fn test_terminal_states_have_no_transitions() {
        assert!(AgentState::Completed.valid_transitions().is_empty());
        assert!(AgentState::Failed.valid_transitions().is_empty());
        assert!(AgentState::Cancelled.valid_transitions().is_empty());
    }

    #[test]
    fn test_streaming_can_execute_tool_or_finish() {
        assert!(AgentState::Streaming.can_transition_to(AgentState::ExecutingTool));
        assert!(AgentState::Streaming.can_transition_to(AgentState::WaitingForInput));
        assert!(AgentState::Streaming.can_transition_to(AgentState::Failed));
        assert!(!AgentState::Streaming.can_transition_to(AgentState::Completed));
        // 必须经过 WaitingForInput
    }

    #[test]
    fn test_executing_tool_transitions() {
        assert!(AgentState::ExecutingTool.can_transition_to(AgentState::Streaming));
        assert!(AgentState::ExecutingTool.can_transition_to(AgentState::Failed));
        assert!(AgentState::ExecutingTool.can_transition_to(AgentState::Cancelled));
        assert!(!AgentState::ExecutingTool.can_transition_to(AgentState::Completed));
    }

    #[test]
    fn test_waiting_for_input_transitions() {
        assert!(AgentState::WaitingForInput.can_transition_to(AgentState::Streaming));
        assert!(AgentState::WaitingForInput.can_transition_to(AgentState::Cancelled));
        assert!(!AgentState::WaitingForInput.can_transition_to(AgentState::ExecutingTool));
    }
}

use serde::{Deserialize, Serialize};

/// Current status of a goal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// Goal is actively being pursued (auto-continue via `PreStop` hook)
    Active,
    /// Goal was paused by user (`PreStop` hook does not auto-continue)
    Paused,
    /// Goal was completed successfully
    Completed,
    /// Goal is blocked and cannot make progress without user input
    Blocked,
}

impl GoalStatus {
    /// Return a stable lowercase string representation for the wire protocol.
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Paused => "paused",
            GoalStatus::Completed => "completed",
            GoalStatus::Blocked => "blocked",
        }
    }
}

/// Runtime state for an active goal.
///
/// This is persisted via `GoalStore` so that resume can restore the goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GoalState {
    /// The high-level goal description (e.g. "implement user auth API")
    pub description: String,

    /// Current status of the goal
    pub status: GoalStatus,
}

impl GoalState {
    /// Create a new active goal with the given description.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            status: GoalStatus::Active,
        }
    }

    /// Build the auto-continue prompt for the current goal.
    /// Wrapped in `<system_reminder>`.
    pub fn build_continue_prompt(&self) -> String {
        let body =
            include_str!("prompts/continuation.md").replace("{{objective}}", &self.description);
        format!("<system_reminder>\n{body}\n</system_reminder>")
    }

    /// Build the prompt injected when the user edits an active goal.
    pub fn objective_updated_prompt(&self) -> String {
        include_str!("prompts/objective_updated.md").replace("{{objective}}", &self.description)
    }
}

impl Default for GoalState {
    fn default() -> Self {
        Self {
            description: String::new(),
            status: GoalStatus::Active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state() {
        let s = GoalState::default();
        assert!(s.description.is_empty());
        assert!(matches!(s.status, GoalStatus::Active));
    }

    #[test]
    fn test_builder() {
        let s = GoalState::new("do stuff");
        assert_eq!(s.description, "do stuff");
    }

    #[test]
    fn test_serde_roundtrip() {
        let s = GoalState::new("test goal");
        let json = serde_json::to_string(&s).unwrap();
        let decoded: GoalState = serde_json::from_str(&json).unwrap();
        assert_eq!(s.description, decoded.description);
        assert_eq!(s.status, decoded.status);
    }

    #[test]
    fn test_continue_prompt() {
        let state = GoalState::new("test goal");
        let p = state.build_continue_prompt();
        assert!(p.contains("Continue working toward the active goal"));
        assert!(p.contains("test goal"));
        assert!(p.contains("Completion audit"));
        assert!(p.contains("Blocked audit"));
        assert!(p.contains("updateGoal"));
    }
}

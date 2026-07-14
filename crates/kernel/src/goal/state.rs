use serde::{Deserialize, Serialize};

/// Current status of a goal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// Goal is actively being pursued
    Active,
    /// Goal was paused by user (agent does not auto-continue)
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
#[path = "state_test.rs"]
mod tests;

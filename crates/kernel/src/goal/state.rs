use crate::types::{Message, Role};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

/// Current status of a goal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalStatus {
    /// Goal is actively being pursued
    Active,
    /// Goal was completed successfully
    Completed,
    /// Goal failed (see `GoalFailureReason`)
    Failed(GoalFailureReason),
    /// Goal was cancelled by the user
    Cancelled,
}

/// Reason why a goal failed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalFailureReason {
    /// Reached maximum iterations without completing
    MaxIterations,
    /// Detected a doom loop (repeating the same actions)
    DoomLoop,
    /// An unrecoverable error occurred during execution
    Error,
}

/// Runtime state for an active goal.
///
/// This is persisted via `GoalStore` so that resume can restore the goal.
/// `recent_signatures` is **not** persisted; it is rebuilt on resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GoalState {
    // --- user-facing configuration ---
    /// The high-level goal description (e.g. "implement user auth API")
    pub description: String,

    /// Marker string that signals completion (default: `<goal_complete>`)
    #[serde(default = "default_completion_marker")]
    pub completion_marker: String,

    /// Max iterations for this goal.
    /// `None` means no goal-specific limit (the agent's own limit still applies).
    pub max_iterations: Option<usize>,

    /// If true, clear non-system context between iterations to prevent
    /// context degradation. The model should persist important state to files.
    pub clear_context: bool,

    /// Inject a progress reminder every N iterations.
    /// `None` means disabled.
    pub progress_interval: Option<usize>,

    // --- runtime state ---
    pub status: GoalStatus,
    pub iteration_count: usize,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,

    /// Recent turn signatures for doom-loop detection.
    /// Not persisted; rebuilt on resume.
    #[serde(skip)]
    pub recent_signatures: VecDeque<String>,
}

fn default_completion_marker() -> String {
    "<goal_complete>".to_string()
}

/// Signature used for doom-loop detection
const DOOM_LOOP_WINDOW: usize = 4;
const DOOM_LOOP_CAPACITY: usize = 6;

impl GoalState {
    /// Create a new active goal with the given description.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            ..Default::default()
        }
    }

    /// Set a custom completion marker.
    #[must_use]
    pub fn with_completion_marker(mut self, marker: impl Into<String>) -> Self {
        self.completion_marker = marker.into();
        self
    }

    /// Set max iterations.
    #[must_use]
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Enable context clearing between iterations.
    #[must_use]
    pub fn with_clear_context(mut self) -> Self {
        self.clear_context = true;
        self
    }

    /// Set progress reminder interval.
    #[must_use]
    pub fn with_progress_interval(mut self, interval: usize) -> Self {
        self.progress_interval = Some(interval);
        self
    }

    /// Build the initial goal message inserted as a User message.
    /// Wrapped in `<system_reminder>` so the TUI can identify it.
    /// Includes the full goal-mode rules so that runtime `/goal` works
    /// even when the system prompt was built without `with_goal_rules`.
    pub fn to_user_message(&self) -> String {
        format!(
            "<system_reminder>\n\
             # Goal Mode\n\
             When a goal is active, work autonomously toward it. Take actions, read files, write code, run tests, and iterate until the goal is fully achieved.\n\
             Goal: {}\n\
             IMPORTANT: If you have completed the goal, output '{}'.\n\
             </system_reminder>",
            self.description, self.completion_marker
        )
    }

    /// Build the auto-continue prompt for the current iteration count.
    /// Wrapped in `<system_reminder>`.
    pub fn build_continue_prompt(&self) -> String {
        let body = if self
            .progress_interval
            .is_some_and(|n| self.iteration_count > 0 && self.iteration_count % n == 0)
        {
            format!(
                "Continue working toward the goal. You have completed {} iterations so far. \
                 Review your progress and keep going. If you are done, output '{}'.",
                self.iteration_count, self.completion_marker
            )
        } else {
            format!(
                "Continue working toward the goal. If you have completed it, output '{}'.",
                self.completion_marker
            )
        };
        format!("<system_reminder>\n{body}\n</system_reminder>")
    }

    /// Check if the assistant's last message signals goal completion
    pub fn check_completion(&self, last_assistant_msg: &Message) -> bool {
        if last_assistant_msg.role != Role::Assistant {
            return false;
        }
        let text: String = last_assistant_msg
            .content
            .iter()
            .filter_map(|b| b.as_text())
            .collect();
        text.contains(&self.completion_marker)
    }

    /// Record a turn signature for doom-loop detection
    pub fn record_turn(&mut self, msg: &Message) {
        let content_sig = msg
            .content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<String>();

        let tool_sig = msg
            .tool_calls
            .as_ref()
            .map(|calls| {
                calls
                    .iter()
                    .map(|c| format!("{}:{}", c.name, c.id))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();

        let signature = format!("{content_sig}|{tool_sig}");

        self.recent_signatures.push_back(signature);
        if self.recent_signatures.len() > DOOM_LOOP_CAPACITY {
            self.recent_signatures.pop_front();
        }
        self.iteration_count += 1;
    }

    /// Detect doom loop: last `DOOM_LOOP_WINDOW` turns have identical signatures
    pub fn is_doom_loop(&self) -> bool {
        if self.recent_signatures.len() < DOOM_LOOP_WINDOW {
            return false;
        }
        let last = self.recent_signatures.back().unwrap();
        self.recent_signatures
            .iter()
            .rev()
            .take(DOOM_LOOP_WINDOW)
            .all(|s| s == last)
    }

    /// Mark the goal as completed
    pub fn complete(&mut self) {
        self.status = GoalStatus::Completed;
        self.completed_at = Some(Utc::now());
    }

    /// Mark the goal as failed
    pub fn fail(&mut self, reason: GoalFailureReason) {
        self.status = GoalStatus::Failed(reason);
        self.completed_at = Some(Utc::now());
    }

    /// Mark the goal as cancelled
    pub fn cancel(&mut self) {
        self.status = GoalStatus::Cancelled;
        self.completed_at = Some(Utc::now());
    }
}

impl Default for GoalState {
    fn default() -> Self {
        Self {
            description: String::new(),
            completion_marker: default_completion_marker(),
            max_iterations: None,
            clear_context: false,
            progress_interval: None,
            status: GoalStatus::Active,
            iteration_count: 0,
            created_at: Utc::now(),
            completed_at: None,
            recent_signatures: VecDeque::with_capacity(DOOM_LOOP_CAPACITY),
        }
    }
}

/// Wraps a [`GoalState`] together with its [`GoalStore`] so that every mutating
/// operation is automatically persisted.
#[derive(Clone)]
pub struct GoalContext {
    state: GoalState,
    store: Option<Arc<dyn crate::goal::GoalStore>>,
    session_id: String,
}

impl std::fmt::Debug for GoalContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalContext")
            .field("state", &self.state)
            .field("store", &self.store.is_some())
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl GoalContext {
    pub fn new(
        state: GoalState,
        store: Option<Arc<dyn crate::goal::GoalStore>>,
        session_id: String,
    ) -> Self {
        Self {
            state,
            store,
            session_id,
        }
    }

    /// Mark the goal as completed and persist.
    pub async fn mark_complete(&mut self) {
        self.state.complete();
        self.persist().await;
    }

    /// Mark the goal as failed and persist.
    pub async fn mark_fail(&mut self, reason: GoalFailureReason) {
        self.state.fail(reason);
        self.persist().await;
    }

    /// Record a turn and persist.
    pub async fn record_turn_and_save(&mut self, msg: &Message) {
        self.state.record_turn(msg);
        self.persist().await;
    }

    /// Persist the current state to the store if one is configured.
    async fn persist(&self) {
        if let Some(ref store) = self.store {
            if let Err(e) = store.save(&self.session_id, &self.state).await {
                tracing::warn!("Failed to persist goal state: {}", e);
            }
        }
    }
}

impl std::ops::Deref for GoalContext {
    type Target = GoalState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContentBlock;

    fn make_assistant_msg(text: &str) -> Message {
        Message::with_blocks(
            Role::Assistant,
            vec![ContentBlock::Text { text: text.into() }],
        )
    }

    #[test]
    fn test_default_state() {
        let s = GoalState::default();
        assert!(s.description.is_empty());
        assert_eq!(s.completion_marker, "<goal_complete>");
        assert!(s.max_iterations.is_none());
        assert!(!s.clear_context);
        assert!(s.progress_interval.is_none());
        assert!(matches!(s.status, GoalStatus::Active));
        assert_eq!(s.iteration_count, 0);
    }

    #[test]
    fn test_builder() {
        let s = GoalState::new("do stuff")
            .with_max_iterations(50)
            .with_clear_context()
            .with_progress_interval(10);
        assert_eq!(s.description, "do stuff");
        assert_eq!(s.max_iterations, Some(50));
        assert!(s.clear_context);
        assert_eq!(s.progress_interval, Some(10));
    }

    #[test]
    fn test_serde_roundtrip() {
        let s = GoalState::new("test goal").with_max_iterations(42);
        let json = serde_json::to_string(&s).unwrap();
        let decoded: GoalState = serde_json::from_str(&json).unwrap();
        assert_eq!(s.description, decoded.description);
        assert_eq!(s.completion_marker, decoded.completion_marker);
        assert_eq!(s.max_iterations, decoded.max_iterations);
        assert_eq!(s.clear_context, decoded.clear_context);
        assert_eq!(s.progress_interval, decoded.progress_interval);
        assert_eq!(s.status, decoded.status);
        assert_eq!(s.iteration_count, decoded.iteration_count);
        // recent_signatures is skipped
        assert!(decoded.recent_signatures.is_empty());
    }

    #[test]
    fn test_check_completion() {
        let state = GoalState::new("test");

        let msg = make_assistant_msg("I am done. <goal_complete>");
        assert!(state.check_completion(&msg));

        let msg2 = make_assistant_msg("Still working...");
        assert!(!state.check_completion(&msg2));
    }

    #[test]
    fn test_doom_loop_detection() {
        let mut state = GoalState::new("test");

        assert!(!state.is_doom_loop());

        for _ in 0..4 {
            state.record_turn(&make_assistant_msg("same"));
        }
        assert!(state.is_doom_loop());

        state.record_turn(&make_assistant_msg("different"));
        assert!(!state.is_doom_loop());
    }

    #[test]
    fn test_continue_prompt_variants() {
        let mut state = GoalState::new("test").with_progress_interval(2);

        let p1 = state.build_continue_prompt();
        assert!(p1.contains("Continue working toward the goal"));
        assert!(!p1.contains("iterations"));

        state.record_turn(&make_assistant_msg("x"));
        state.record_turn(&make_assistant_msg("x"));
        let p2 = state.build_continue_prompt();
        assert!(p2.contains("2 iterations"));
    }
}

use crate::agent::{AgentConfig, AgentError, AgentInput, AgentShared, AgentState, CancelToken};
use crate::permissions::Responder;
use crate::tools::AskUserResponder;
use crate::types::{AgentId, ContentBlock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Handle for controlling a running Agent from the outside
#[derive(Clone)]
pub struct AgentHandle {
    pub id: AgentId,
    pub(super) input_tx: mpsc::Sender<AgentInput>,
    pub(super) state_rx: tokio::sync::watch::Receiver<AgentState>,
    cancel_token: CancelToken,
    pub(super) permission_responder: Option<Responder>,
    pub(super) ask_user_responder: Option<AskUserResponder>,
    /// Generation counter: inputs with lower generation are stale (cancelled before send)
    input_stale_since: Arc<AtomicU64>,
    /// Channel for sending steer messages that are injected before the next streaming
    pub(super) steer_tx: mpsc::Sender<Vec<ContentBlock>>,
    /// Whether the agent is currently compacting messages
    pub(super) compacting: Arc<AtomicBool>,
}

impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("id", &self.id)
            .field("cancel_token", &self.cancel_token)
            .field("permission_responder", &self.permission_responder.is_some())
            .field("ask_user_responder", &self.ask_user_responder.is_some())
            .field(
                "input_generation",
                &self.input_stale_since.load(Ordering::Acquire),
            )
            .field("compacting", &self.compacting.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl AgentHandle {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: AgentId,
        input_tx: mpsc::Sender<AgentInput>,
        state_rx: tokio::sync::watch::Receiver<AgentState>,
        cancel_token: CancelToken,
        permission_responder: Option<Responder>,
        ask_user_responder: Option<AskUserResponder>,
        input_stale_since: Arc<AtomicU64>,
        steer_tx: mpsc::Sender<Vec<ContentBlock>>,
        compacting: Arc<AtomicBool>,
    ) -> Self {
        Self {
            id,
            input_tx,
            state_rx,
            cancel_token,
            permission_responder,
            ask_user_responder,
            input_stale_since,
            steer_tx,
            compacting,
        }
    }

    /// Send a user message to the Agent (supports multi-modal content)
    pub async fn send_message(&self, content: Vec<ContentBlock>) -> Result<(), AgentError> {
        let generation = self.input_stale_since.load(Ordering::Acquire);
        let input = AgentInput::User {
            content,
            generation,
        };
        self.input_tx
            .send(input)
            .await
            .map_err(|_| AgentError::ChannelClosed)
    }

    /// Send a user text message to the Agent (convenience method)
    pub async fn send_text(&self, text: String) -> Result<(), AgentError> {
        self.send_message(vec![ContentBlock::Text { text }]).await
    }

    /// Send a permission response to the Agent
    pub async fn send_permission_response(
        &self,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<(), AgentError> {
        if let Some(ref responder) = self.permission_responder {
            responder.respond(req_id, approved, remember).await;
            Ok(())
        } else {
            Err(AgentError::NoPermissionChecker)
        }
    }

    /// Send an `ask_user` response to the Agent
    pub async fn send_ask_user_response(
        &self,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<(), AgentError> {
        if let Some(ref responder) = self.ask_user_responder {
            responder.respond(req_id, response).await;
            Ok(())
        } else {
            Err(AgentError::NoPermissionChecker)
        }
    }

    /// Get the current state
    pub fn state(&self) -> AgentState {
        *self.state_rx.borrow()
    }

    /// Whether the agent is currently compacting messages
    pub fn is_compacting(&self) -> bool {
        self.compacting.load(Ordering::Relaxed)
    }

    /// Wait for a state change
    pub async fn wait_for_state_change(&mut self) -> AgentState {
        let _ = self.state_rx.changed().await;
        *self.state_rx.borrow()
    }

    /// Request cancellation, also incrementing the generation so that
    /// any input sent before this cancellation becomes stale.
    pub fn cancel(&self) {
        self.input_stale_since.fetch_add(1, Ordering::SeqCst);
        self.cancel_token.cancel();
    }

    /// Gracefully shut down the Agent (sends Close signal, distinct from Cancel)
    pub async fn close(&self) -> Result<(), AgentError> {
        self.input_tx
            .send(super::AgentInput::Shutdown)
            .await
            .map_err(|_| AgentError::ChannelClosed)
    }

    /// Request forced compaction of the message buffer
    pub async fn force_compact(&self) -> Result<(), AgentError> {
        self.input_tx
            .send(AgentInput::Compact)
            .await
            .map_err(|_| AgentError::ChannelClosed)
    }

    /// Dynamically reload the skill list for a running agent
    pub async fn reload_skills(
        &self,
        skills: Vec<Arc<crate::skill::Skill>>,
    ) -> Result<(), AgentError> {
        self.input_tx
            .send(AgentInput::ReloadSkills(skills))
            .await
            .map_err(|_| AgentError::ChannelClosed)
    }

    /// Dynamically reload the full agent configuration for a running agent
    pub async fn reload_config(
        &self,
        config: AgentConfig,
        shared: Arc<AgentShared>,
    ) -> Result<(), AgentError> {
        self.input_tx
            .send(AgentInput::ReloadConfig(Box::new(config), shared))
            .await
            .map_err(|_| AgentError::ChannelClosed)
    }

    /// Rewind to a specific checkpoint
    pub async fn rewind(
        &self,
        message_id: crate::types::MessageId,
        target: crate::checkpoint::RewindTarget,
    ) -> Result<Result<(), String>, AgentError> {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        self.input_tx
            .send(AgentInput::Rewind {
                message_id,
                target,
                result_tx,
            })
            .await
            .map_err(|_| AgentError::ChannelClosed)?;

        result_rx.await.map_err(|_| AgentError::ChannelClosed)
    }

    /// Send a steer message to be injected before the next streaming turn.
    /// Uses `try_send` to avoid blocking if the agent is backlogged.
    pub fn send_steer(&self, content: Vec<ContentBlock>) -> Result<(), AgentError> {
        self.steer_tx.try_send(content).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => AgentError::ChannelFull,
            mpsc::error::TrySendError::Closed(_) => AgentError::ChannelClosed,
        })
    }

    /// Send a continue command to transition the agent from Idle to Streaming.
    pub fn send_continue(&self) -> Result<(), AgentError> {
        self.input_tx
            .try_send(AgentInput::Continue)
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => AgentError::ChannelFull,
                mpsc::error::TrySendError::Closed(_) => AgentError::ChannelClosed,
            })
    }
}

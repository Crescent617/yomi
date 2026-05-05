use crate::agent::AgentShared;
use crate::app::session::{Session, SessionConfig};
use crate::event::{Event, SystemEvent};
use crate::permissions::Level;
use crate::providers::{ModelConfig, Provider};
use crate::storage::{MessageStore, SessionStore, StorageSet};
use crate::types::{KernelError, Result, SessionId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

pub struct Coordinator {
    agent_shared: Arc<AgentShared>,
    sessions: Arc<RwLock<HashMap<SessionId, Arc<RwLock<Session>>>>>,
    /// Broadcast channels for session events (for forwarding and cleanup)
    session_event_senders: Arc<RwLock<HashMap<SessionId, broadcast::Sender<Event>>>>,
}

impl Coordinator {
    /// Get session store from `agent_shared`
    pub fn session_store(&self) -> &Arc<dyn SessionStore> {
        self.agent_shared
            .session_store
            .as_ref()
            .expect("session_store not configured")
    }

    /// Get message store from `agent_shared`
    pub fn message_store(&self) -> &Arc<dyn MessageStore> {
        self.agent_shared
            .message_store
            .as_ref()
            .expect("message_store not configured")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: &StorageSet,
        provider: Arc<dyn Provider>,
        model_config: ModelConfig,
        task_store: Option<Arc<crate::task::TaskStore>>,
        compactor: Option<crate::compactor::Compactor>,
        skill_folders: Vec<std::path::PathBuf>,
    ) -> Self {
        let session_store = storage.session_store();
        let message_store = storage.message_store();
        let agent_shared = Arc::new(AgentShared::new(
            provider,
            Arc::new(model_config),
            task_store,
            Some(storage.todo_store()),
            compactor,
            Some(session_store),
            Some(message_store),
            Some(storage.usage_store()),
            None,
            skill_folders,
            None,
        ));
        Self {
            agent_shared,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_event_senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new session with the given configuration
    pub async fn create_session(&self, config: SessionConfig) -> Result<SessionId> {
        let working_dir = config.project_path.to_string_lossy().to_string();
        let id = self.session_store().create(Some(&working_dir)).await?;
        self.init_session(id.clone(), config).await?;
        tracing::info!("Session {} created", id.0);
        Ok(id)
    }

    /// Initialize a session in memory
    async fn init_session(&self, session_id: SessionId, config: SessionConfig) -> Result<()> {
        // Check if session already exists in memory
        if self.get_session(&session_id).await.is_some() {
            return Err(KernelError::session(format!(
                "Session {} already initialized",
                session_id.0
            )));
        }

        // Initialize session and get the event receiver directly
        let (session, event_rx) =
            Session::init(session_id.clone(), config, Arc::clone(&self.agent_shared)).await?;

        // Get the main agent ID for monitoring
        let main_agent_id = session.main_agent_id().cloned();

        // Create broadcast channel for external consumers
        let (broadcast_tx, _) = broadcast::channel::<Event>(256);

        // Store the broadcast sender
        self.session_event_senders
            .write()
            .await
            .insert(session_id.clone(), broadcast_tx.clone());

        // Spawn event forwarding task
        let sessions_clone = Arc::clone(&self.sessions);
        let senders_clone = Arc::clone(&self.session_event_senders);
        let sid_clone = session_id.clone();
        tokio::spawn(async move {
            Self::forward_session_events(
                sid_clone,
                event_rx,
                broadcast_tx,
                main_agent_id,
                sessions_clone,
                senders_clone,
            )
            .await;
        });

        self.sessions
            .write()
            .await
            .insert(session_id, Arc::new(RwLock::new(session)));

        Ok(())
    }

    /// Forward events from agent to broadcast channel and handle cleanup
    async fn forward_session_events(
        session_id: SessionId,
        mut agent_rx: mpsc::Receiver<Event>,
        broadcast_tx: broadcast::Sender<Event>,
        _main_agent_id: Option<crate::types::AgentId>,
        sessions: Arc<RwLock<HashMap<SessionId, Arc<RwLock<Session>>>>>,
        senders: Arc<RwLock<HashMap<SessionId, broadcast::Sender<Event>>>>,
    ) {
        let sid_str = session_id.0.clone();
        tracing::info!("Event forwarding started for session {}", sid_str);

        // Forward events until the channel closes (agent ended)
        while let Some(event) = agent_rx.recv().await {
            if broadcast_tx.send(event).is_err() {
                // No active subscribers (this is ok, receivers can come and go)
                tracing::trace!("No active subscribers for session {} events", sid_str);
            }
        }

        // Agent channel closed - session is shutting down
        tracing::info!("Main agent for session {} closed", sid_str);

        // Broadcast shutdown event
        let shutdown_event = Event::System(SystemEvent::Shutdown {
            session_id: session_id.clone(),
            error: None, // TODO: capture error from agent if needed
        });
        let _ = broadcast_tx.send(shutdown_event);

        // Remove session from coordinator
        sessions.write().await.remove(&session_id);
        senders.write().await.remove(&session_id);
        tracing::info!("Session {} removed from coordinator", sid_str);
    }

    /// Restore a session from storage by its ID
    pub async fn restore_session(
        &self,
        session_id: &SessionId,
        config: SessionConfig,
    ) -> Result<SessionId> {
        // Verify session exists in storage
        let session_info = self.session_store().get(session_id).await?.ok_or_else(|| {
            KernelError::session(format!("Session not found in storage: {}", session_id.0))
        })?;

        tracing::info!("Restoring session {} from storage", session_id.0);
        self.init_session(session_info.id.clone(), config).await?;
        tracing::info!("Session {} restored", session_info.id.0);
        Ok(session_info.id)
    }

    /// Fork a session: create new session with copied history from parent
    pub async fn fork_session(
        &self,
        parent_id: &SessionId,
        config: SessionConfig,
    ) -> Result<SessionId> {
        // Create new session with copied history in storage
        let new_id = self.session_store().fork(parent_id).await?;
        tracing::info!("Forked session {} from {}", new_id.0, parent_id.0);

        self.init_session(new_id.clone(), config).await?;
        tracing::info!("Forked session {} initialized", new_id.0);
        Ok(new_id)
    }

    pub async fn get_session(&self, id: &SessionId) -> Option<Arc<RwLock<Session>>> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Get session or return not found error
    async fn require_session(&self, session_id: &SessionId) -> Result<Arc<RwLock<Session>>> {
        self.get_session(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("Session not found: {}", session_id.0)))
    }

    pub async fn list_sessions(&self) -> Vec<SessionId> {
        self.sessions.read().await.keys().cloned().collect()
    }

    pub async fn send_message(&self, session_id: &SessionId, content: String) -> Result<()> {
        tracing::debug!(
            "Sending message to session {} ({} bytes)",
            session_id.0,
            content.len()
        );
        let session = self.require_session(session_id).await?;
        let result = session.read().await.send_message(content).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to send message to session {}: {}", session_id.0, e);
        }
        result
    }

    /// Send a multi-modal message with content blocks
    pub async fn send_blocks(
        &self,
        session_id: &SessionId,
        blocks: Vec<crate::types::ContentBlock>,
    ) -> Result<()> {
        tracing::debug!(
            "Sending {} content blocks to session {}",
            blocks.len(),
            session_id.0
        );
        let session = self.require_session(session_id).await?;
        let result = session.read().await.send_blocks(blocks).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to send blocks to session {}: {}", session_id.0, e);
        }
        result
    }

    /// Subscribe to events for a session (to be called by TUI)
    /// Returns None if session not found
    /// Each call returns a new receiver that will receive all future events
    pub async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Option<broadcast::Receiver<Event>> {
        Some(
            self.session_event_senders
                .read()
                .await
                .get(session_id)?
                .subscribe(),
        )
    }

    pub async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id).await?;
        session.read().await.cancel();
        Ok(())
    }

    pub async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        let session = self.require_session(session_id).await?;
        session
            .read()
            .await
            .send_permission_response(req_id, approved, remember)
            .await?;
        Ok(())
    }

    pub async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        let session = self.require_session(session_id).await?;
        session.read().await.set_permission_level(level).await;
        tracing::info!(
            "Permission level set to {:?} for session {}",
            level,
            session_id.0
        );
        Ok(())
    }

    /// Request compaction for a session's message buffer
    pub async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id).await?;
        let result = session.read().await.compact().await;
        if let Err(ref e) = result {
            tracing::error!("Failed to compact session {}: {}", session_id.0, e);
        } else {
            tracing::info!("Compaction requested for session {}", session_id.0);
        }
        result
    }

    /// Delete a session from storage
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.session_store().delete(session_id).await
    }

    /// Get messages for a session from storage
    pub async fn get_session_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::Message>> {
        self.message_store().get(&session_id.0).await
    }
}

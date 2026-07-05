//! Message compaction for Agent
//!
//! Handles auto-compaction and forced compaction of message history.

use super::message_buffer::MessageBuffer;
use crate::compactor::{CompactionError, Compactor};
use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, StopReason};
use crate::providers::{ModelConfig, Provider};
use crate::storage::UsageStore;
use crate::types::{Message, Role, SessionId};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Compaction manager for an agent
pub struct CompactionManager {
    /// Session ID
    pub session_id: String,
    /// Event sender
    pub event_tx: mpsc::Sender<Event>,
    /// Compactor configuration
    pub compactor: Option<Compactor>,
    /// Model configuration
    pub model_config: Arc<ModelConfig>,
    /// Provider for API calls
    pub provider: Arc<dyn Provider>,
    /// Usage store for recording compactor tokens
    pub usage_store: Option<Arc<dyn UsageStore>>,
    /// File state store (cleared on compaction)
    pub file_state_store: Option<Arc<crate::tools::helper::FileStateStore>>,
    /// Message store for persisting compacted messages
    pub message_store: Option<Arc<dyn crate::storage::MessageStore>>,
}

impl CompactionManager {
    /// Check if compaction is enabled
    pub fn is_enabled(&self) -> bool {
        self.compactor.is_some()
    }

    /// Force compaction regardless of threshold
    pub async fn force_compact(
        &self,
        message_buffer: &mut MessageBuffer,
        cancel_token: &super::CancelToken,
    ) -> Result<String, String> {
        let compactor = self.compactor.as_ref().ok_or("No compactor configured")?;
        let old_count = message_buffer.len();

        self.emit_compaction_event(true);

        let result = compactor
            .auto_compact(
                message_buffer.messages(),
                Arc::clone(&self.provider),
                &self.model_config,
                Some(cancel_token.runtime_token()),
            )
            .await;

        self.handle_compaction_result(result, old_count, message_buffer)
            .await
    }

    /// Force full compaction (skip micro-compaction)
    pub async fn force_full_compact(
        &self,
        message_buffer: &mut MessageBuffer,
        cancel_token: &super::CancelToken,
    ) -> Result<String, String> {
        let compactor = self.compactor.as_ref().ok_or("No compactor configured")?;
        let old_count = message_buffer.len();

        self.emit_compaction_event(true);

        let result = compactor
            .full_compact(
                message_buffer.messages(),
                Arc::clone(&self.provider),
                &self.model_config,
                Some(cancel_token.runtime_token()),
            )
            .await
            .map(Some);

        self.handle_compaction_result(result, old_count, message_buffer)
            .await
    }

    /// Check and run compaction if needed
    /// Returns true if compaction occurred
    #[tracing::instrument(skip(self, message_buffer, cancel_token))]
    pub async fn maybe_compact(
        &self,
        message_buffer: &mut MessageBuffer,
        cancel_token: &super::CancelToken,
    ) -> bool {
        let Some(compactor) = self.compactor.as_ref() else {
            return false;
        };

        if !compactor.should_compact(message_buffer.messages()) {
            return false;
        }

        match self.force_compact(message_buffer, cancel_token).await {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("auto-compaction failed: {}", e);
                false
            }
        }
    }

    /// Handle compaction result, update state, and return user message
    #[tracing::instrument(skip(self, result, message_buffer))]
    async fn handle_compaction_result(
        &self,
        result: Result<Option<crate::compactor::CompactionResult>, CompactionError>,
        old_count: usize,
        message_buffer: &mut MessageBuffer,
    ) -> Result<String, String> {
        let compact_result = match result {
            Ok(None) => Ok("No compaction needed".to_string()),
            Ok(Some(compaction_result)) => {
                // Record compactor token usage
                self.record_compactor_token_usage(compaction_result.token_usage)
                    .await;

                self.apply_compacted_messages(compaction_result.messages, message_buffer)
                    .await;

                let new_count = message_buffer.len();
                let compacted_count = old_count.saturating_sub(new_count);

                // Clear file state only if messages were actually reduced
                if compacted_count > 0 {
                    if let Some(ref file_state_store) = self.file_state_store {
                        tracing::info!(
                            "clearing file state due to compaction ({} -> {} messages)",
                            old_count,
                            new_count
                        );
                        file_state_store.clear().await;
                    }
                }

                Ok(if compacted_count > 0 {
                    info!(
                        "compaction completed: {} -> {} messages (compacted {})",
                        old_count, new_count, compacted_count
                    );
                    format!("Compacted {compacted_count} messages")
                } else {
                    "Micro-compaction completed".to_string()
                })
            }
            Err(CompactionError::Cancelled) => {
                tracing::info!("compaction cancelled");
                self.emit_operation_cancelled("compaction");
                Err("Compaction was cancelled".to_string())
            }
            Err(CompactionError::Api(e)) => {
                tracing::warn!("compaction failed: {}", e);
                self.emit_error(crate::event::ErrorPhase::Compaction, &e, false);
                Err(format!("Compaction failed: {e}"))
            }
        };

        self.emit_compaction_event(false);
        compact_result
    }

    /// Apply compacted messages: update buffer and persist to storage
    async fn apply_compacted_messages(
        &self,
        messages: Vec<Arc<Message>>,
        message_buffer: &mut MessageBuffer,
    ) {
        // Reconstruct buffer: keep system messages + compacted messages
        let new_messages: Vec<Arc<Message>> = message_buffer
            .messages()
            .iter()
            .filter(|m| m.role == Role::System)
            .take(1) // Only keep the first system message
            .cloned()
            .chain(messages.iter().filter(|m| m.role != Role::System).cloned())
            .collect();

        *message_buffer.messages_mut() = new_messages;

        // Persist compacted messages
        if let Some(store) = &self.message_store {
            let to_persist: Vec<Message> = messages
                .iter()
                .filter(|m| m.role != Role::System)
                .map(|m| (**m).clone())
                .collect();
            if let Err(e) = store.replace(&self.session_id, &to_persist).await {
                warn!("failed to persist compacted messages: {}", e);
            }
        }
    }

    /// Emit compaction start/end event
    fn emit_compaction_event(&self, active: bool) {
        if let Err(e) = self
            .event_tx
            .try_send(Event::Model(ModelEvent::Compacting { active }))
        {
            tracing::warn!("Failed to send compacting event (active={}): {}", active, e);
        }
    }

    /// Emit operation cancelled event
    fn emit_operation_cancelled(&self, operation: &str) {
        if let Err(e) = self.event_tx.try_send(Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Stopped {
                reason: StopReason::Cancelled {
                    operation: Some(operation.to_string()),
                },
            },
        })) {
            tracing::warn!("Failed to emit operation cancelled event: {}", e);
        }
    }

    /// Emit error event
    fn emit_error(&self, phase: crate::event::ErrorPhase, error: &str, is_recoverable: bool) {
        if let Err(e) = self.event_tx.try_send(Event::Agent(AgentEvent::Error {
            phase,
            error: error.to_string(),
            is_recoverable,
        })) {
            tracing::warn!("Failed to emit error event: {}", e);
        }
    }

    /// Record compactor token usage
    async fn record_compactor_token_usage(&self, usage: crate::providers::TokenUsage) {
        if usage.prompt_tokens == 0 && usage.completion_tokens == 0 {
            return;
        }
        if let Some(store) = &self.usage_store {
            let record = crate::storage::UsageRecord::new(
                SessionId::from(self.session_id.clone()),
                usage,
                self.model_config.model_id.clone(),
                self.model_config.provider.to_string(),
                crate::storage::UsageType::Compactor,
            );
            if let Err(e) = store.record(&record).await {
                tracing::warn!("Failed to record compactor token usage: {}", e);
            }
        }
    }
}

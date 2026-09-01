//! Local (in-process) `KernelApi` implementation: thin async wrappers
//! delegating to `Kernel`'s inherent methods. The inherent methods hold
//! the real logic (they are called directly by server-side RPC handlers);
//! this bridge only adapts them to the trait's async/`Result` shape.
//!
//! Note: `Self::foo(self, ...)` resolves to the *inherent* method on
//! `Kernel`, not this trait method — inherent items take precedence in
//! name resolution, so there is no recursion.

use super::{KernelApi, PaginatedSessions, SessionJsonlChunk};
use crate::checkpoint::RewindTarget;
use crate::kernel::{CreateSessionInput, Kernel};
use crate::notification::Notification;
use crate::permission::Level;
use crate::storage::session::SessionListScope;
use crate::types::{ContentBlock, KernelError, MessageId, Project, ProjectId, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

#[async_trait]
impl KernelApi for Kernel {
    async fn stop(&self) {
        Self::stop(self).await;
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn get_config(&self) -> Result<crate::config::KernelConfig> {
        crate::config::Config::get_kernel_config()
    }

    async fn set_config(&self, content: String) -> Result<()> {
        crate::config::Config::set_kernel_config(&content)
    }

    async fn restart(&self) -> Result<()> {
        Err(KernelError::config(
            "restart is only available through a daemon server",
        ))
    }

    async fn read_file(
        &self,
        source: crate::utils::file_read::FileSource,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<crate::utils::file_read::FileBytes> {
        crate::utils::file_read::read_file(&source, &self.data_dir().await, offset, limit).await
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        Self::list_projects(self).await
    }

    async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        Self::create_project(self, dir, name).await
    }

    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        Self::get_project(self, id).await
    }

    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        Self::rename_project(self, id, name).await
    }

    async fn delete_project(&self, id: &ProjectId) -> Result<crate::storage::GcReport> {
        Self::delete_project(self, id).await
    }

    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        Self::create_session(self, input).await
    }

    async fn restore_session(&self, id: &SessionId) -> Result<SessionId> {
        Self::restore_session(self, id).await
    }

    async fn fork_session(
        &self,
        parent: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        Self::fork_session(self, parent, auto_approve_level).await
    }

    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        Self::send_message(self, session_id, blocks).await
    }

    async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        Self::cancel(self, session_id);
        Ok(())
    }

    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        _remember: bool,
    ) -> Result<()> {
        Self::send_permission_response(self, session_id, req_id, approved, _remember)
    }

    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        Self::set_permission_level(self, session_id, level).await
    }

    async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        Self::compact_session(self, session_id)
    }

    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()> {
        Self::rewind_session(self, session_id, message_id, target).await
    }

    async fn rename_session(&self, session_id: &SessionId, title: String) -> Result<()> {
        Self::rename_session(self, session_id, title).await
    }

    async fn pin_session(&self, session_id: &SessionId, emoji: Option<String>) -> Result<()> {
        Self::pin_session(self, session_id, emoji).await
    }

    async fn unpin_session(&self, session_id: &SessionId) -> Result<()> {
        Self::unpin_session(self, session_id).await
    }

    async fn set_pinned_session_emoji(
        &self,
        session_id: &SessionId,
        emoji: Option<String>,
    ) -> Result<()> {
        Self::set_pinned_session_emoji(self, session_id, emoji).await
    }

    async fn list_pinned_sessions(
        &self,
    ) -> Result<Vec<crate::storage::pinned_session::PinnedSessionDetail>> {
        Self::list_pinned_sessions(self).await
    }

    async fn add_favorite(
        &self,
        input: crate::storage::AddFavoriteInput,
    ) -> Result<crate::storage::FavoriteAnswer> {
        Self::add_favorite(self, input).await
    }

    async fn remove_favorite(&self, id: &str) -> Result<()> {
        Self::remove_favorite(self, id).await
    }

    async fn remove_favorite_by_message(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<()> {
        Self::remove_favorite_by_message(self, session_id, message_id).await
    }

    async fn list_favorites(
        &self,
        query: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::storage::FavoriteAnswer>> {
        Self::list_favorites(self, query, limit, offset).await
    }

    async fn update_favorite_note(&self, id: &str, note: Option<String>) -> Result<()> {
        Self::update_favorite_note(self, id, note).await
    }

    async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        Self::delete_session(self, session_id).await
    }

    async fn clear_session(&self, session_id: &SessionId) -> Result<()> {
        Self::clear_session(self, session_id)
    }

    async fn mailbox_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::comms::MailboxSnapshot> {
        Ok(Self::mailbox_snapshot(self, session_id).await)
    }

    async fn remove_mailbox_item(&self, session_id: &SessionId, item_id: &str) -> Result<bool> {
        Ok(Self::remove_mailbox_item(self, session_id, item_id).await)
    }

    async fn steer_mailbox_item(&self, session_id: &SessionId, item_id: &str) -> Result<bool> {
        Ok(Self::steer_mailbox_item(self, session_id, item_id).await)
    }

    async fn clear_mailbox(
        &self,
        session_id: &SessionId,
        scope: crate::comms::MailboxScope,
    ) -> Result<usize> {
        Ok(Self::clear_mailbox(self, session_id, scope).await)
    }

    async fn list_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::SessionMessage>> {
        Self::list_messages(self, session_id).await
    }

    async fn get_session(&self, session_id: &SessionId) -> Result<crate::types::SessionResponse> {
        Ok(Self::get_session(self, session_id).await?)
    }

    async fn get_session_rules(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::types::SessionRulesResponse> {
        Self::get_session_rules(self, session_id).await
    }

    async fn read_session_jsonl(
        &self,
        session_id: &SessionId,
        before_offset: Option<u64>,
        after_offset: Option<u64>,
    ) -> Result<SessionJsonlChunk> {
        Self::read_session_jsonl(self, session_id, before_offset, after_offset).await
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
        _after_event_id: Option<crate::types::EventId>,
    ) -> Result<crate::comms::EventBusSubscriber> {
        Ok(Self::subscribe_session_events(self, session_id))
    }

    async fn subscribe_all_events(&self) -> Result<crate::comms::EventBusSubscriber> {
        // Same Internal filter as `subscribe_session_events` — internal
        // events never leave the kernel.
        Ok(Self::event_bus(self)
            .expect("event_bus must be configured")
            .subscribe_all_filtered(|envelope| {
                !matches!(envelope.event, crate::event::Event::Internal(_))
            }))
    }

    async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        scope: SessionListScope,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<PaginatedSessions> {
        Self::list_sessions(self, project_id, scope, before, limit).await
    }

    async fn list_running_sessions(&self) -> Result<Vec<crate::types::RunningSessionResponse>> {
        Self::list_running_sessions(self).await
    }

    async fn list_subagents(
        &self,
        parent_session_id: &SessionId,
    ) -> Result<Vec<crate::types::SubagentResponse>> {
        Self::list_subagents(self, parent_session_id).await
    }

    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        Self::get_checkpoints(self, session_id).await
    }

    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        Self::get_todos(self, session_id).await
    }

    async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()> {
        Self::send_ask_user_response(self, session_id, req_id, response)
    }

    async fn send_steer(&self, session_id: &SessionId, content: Vec<ContentBlock>) -> Result<()> {
        Self::send_steer(self, session_id, content).await;
        Ok(())
    }

    async fn send_continue(&self, session_id: &SessionId) -> Result<()> {
        Self::send_continue(self, session_id);
        Ok(())
    }

    async fn list_session_skills(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<Arc<crate::skill::Skill>>> {
        Self::list_session_skills(self, session_id).await
    }

    async fn subscribe_notifications(&self) -> Result<mpsc::Receiver<Notification>> {
        let mut rx = self.notification_bus().subscribe();
        let (tx, mpsc_rx) = mpsc::channel(256);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(noti) => {
                        if tx.send(noti).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Notification subscriber lagged, dropped {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(mpsc_rx)
    }

    async fn get_usage_summary(&self, days: i64) -> Result<crate::storage::usage::UsageSummary> {
        Self::get_usage_summary(self, days).await
    }

    async fn get_daily_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::DailyUsage>> {
        Self::get_daily_usage(self, days).await
    }

    async fn get_model_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::ModelUsage>> {
        Self::get_model_usage(self, days).await
    }

    async fn get_model_usage_since(
        &self,
        start: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::storage::usage::ModelUsage>> {
        Self::get_model_usage_since(self, start).await
    }

    async fn get_usage_records(
        &self,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::storage::usage::UsageRecord>> {
        Self::get_usage_records(self, before_id, limit).await
    }

    async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId> {
        Self::create_cron_job(self, input).await
    }

    async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>> {
        Self::list_cron_jobs(self, status, limit).await
    }

    async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>> {
        Self::get_cron_job(self, id).await
    }

    async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool> {
        Self::update_cron_job(self, id, input).await
    }

    async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        Self::delete_cron_job(self, id).await
    }

    async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()> {
        Self::trigger_cron_job(self, id).await
    }

    async fn channel_new_thread(
        &self,
        channel: Option<String>,
        platform: Option<String>,
        chat_id: String,
        title: Option<String>,
        text: String,
    ) -> Result<serde_json::Value> {
        let hub = self.channel_manager().ok_or_else(|| {
            crate::types::KernelError::Config("no channels are running".to_string())
        })?;
        hub.create_thread_in_chat(
            self,
            channel.as_deref(),
            platform
                .as_deref()
                .unwrap_or(crate::channels::DEFAULT_PLATFORM),
            &chat_id,
            title.as_deref(),
            &text,
        )
        .await
    }

    async fn set_channel_watch(
        &self,
        channel: Option<String>,
        platform: Option<String>,
        chat_id: String,
        on: Option<bool>,
    ) -> Result<serde_json::Value> {
        let hub = self.channel_manager().ok_or_else(|| {
            crate::types::KernelError::Config("no channels are running".to_string())
        })?;
        let status = hub
            .rpc_set_channel_watch(
                self,
                channel.as_deref(),
                platform
                    .as_deref()
                    .unwrap_or(crate::channels::DEFAULT_PLATFORM),
                &chat_id,
                on,
            )
            .await?;
        Ok(serde_json::to_value(status)?)
    }

    async fn list_models(&self) -> Result<Vec<crate::kernel::ModelInfo>> {
        Self::list_models(self).await
    }

    async fn get_session_model(&self, session_id: &SessionId) -> Result<String> {
        Ok(Self::get_session_model(self, session_id).await)
    }

    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()> {
        Self::set_session_model(self, session_id, key).await
    }

    async fn list_agent_templates(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<Vec<crate::agent_tmpl::AgentTemplate>> {
        Self::list_agent_templates(self, session_id).await
    }

    async fn save_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
        body: &str,
    ) -> Result<()> {
        Self::save_agent_template(self, session_id, scope, name, body).await
    }

    async fn delete_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
    ) -> Result<()> {
        Self::delete_agent_template(self, session_id, scope, name).await
    }
}

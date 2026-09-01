//! Typed `KernelApi` implementation for [`RemoteKernel`]: each method is a
//! thin wrapper serializing arguments into a [`ReqMethod`] and decoding the
//! JSON result via [`RemoteKernel::call_json`]/[`RemoteKernel::call_unit`].
//! Methods with extra client-side logic (reconnect-aware `restart`,
//! subscription plumbing, write-then-verify cron precheck) stay hand-written.

use super::{RemoteKernel, ALL_EVENTS_ROUTER_KEY, CONNECT_RETRY_TIMEOUT};
use crate::checkpoint::RewindTarget;
use crate::client::RESTART_CONFIG_NOT_APPLIED;
use crate::client::{KernelApi, PaginatedSessions, SessionJsonlChunk};
use crate::event::Command;
use crate::kernel::CreateSessionInput;
use crate::notification::Notification;
use crate::permission::Level;
use crate::storage::session::SessionListScope;
use crate::types::{
    ContentBlock, KernelError, MessageId, Project, ProjectId, Result, SessionError, SessionId,
};
use crate::wire::ReqMethod;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

#[async_trait]
impl KernelApi for RemoteKernel {
    async fn stop(&self) {
        // Remote kernel lifecycle is managed server-side.
        // Just drop any local connection resources.
        let conn = self.connection.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let mut guard = conn.lock().await;
                if let Some(c) = guard.take() {
                    c.cancel.cancel();
                }
            });
        } else {
            // 无 runtime（`swap_kernel` 的 block_on fallback 路径）：
            // tokio Mutex 的 `lock()` 不依赖 reactor——必须真正等
            // 到锁并 cancel（`try_lock` 锁占时静默跳过 = 连接与后
            // 台 heartbeat 泄漏，`Connection` 无 `Drop` 兜底——
            // 复审 must-fix）。
            let mut guard = futures::executor::block_on(conn.lock());
            if let Some(c) = guard.take() {
                c.cancel.cancel();
            }
        }
    }

    async fn is_connected(&self) -> bool {
        // try_lock: ensure_connected holds the mutex across its reconnect
        // loop (up to 10s); a locked connection is unusable anyway, so
        // report not-connected instead of stalling the caller.
        match self.connection.try_lock() {
            Ok(guard) => matches!(guard.as_ref(), Some(conn) if !conn.cancel.is_cancelled()),
            Err(_) => false,
        }
    }

    async fn get_config(&self) -> Result<crate::config::KernelConfig> {
        self.call_json(ReqMethod::GetConfig).await
    }

    async fn set_config(&self, content: String) -> Result<()> {
        self.call_unit(ReqMethod::SetConfig { content }).await
    }

    async fn restart(&self) -> Result<()> {
        let old_instance_id = self.server_instance_id().await?;
        self.call(ReqMethod::Restart).await?;
        self.invalidate_connection().await;

        let start = tokio::time::Instant::now();
        loop {
            match self.server_instance_id().await {
                Ok(instance_id) if instance_id != old_instance_id => break,
                Ok(_) | Err(_) if start.elapsed() < CONNECT_RETRY_TIMEOUT => {
                    self.invalidate_connection().await;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Ok(_) => {
                    return Err(SessionError::Other(
                        "daemon restart timed out waiting for a replacement instance".to_string(),
                    )
                    .into());
                }
                Err(error) => return Err(error),
            }
        }

        let config = self.get_config().await?;
        if config.full_config.is_empty() {
            return Err(KernelError::config(RESTART_CONFIG_NOT_APPLIED));
        }
        Ok(())
    }

    async fn read_file(
        &self,
        source: crate::utils::file_read::FileSource,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<crate::utils::file_read::FileBytes> {
        self.call_json(ReqMethod::ReadFile {
            source,
            offset,
            limit,
        })
        .await
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        self.call_json(ReqMethod::ListProjects).await
    }

    async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        self.call_json(ReqMethod::CreateProject {
            dir: dir.to_string_lossy().to_string(),
            name,
        })
        .await
    }

    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        self.call_json(ReqMethod::GetProject {
            project_id: id.0.to_string(),
        })
        .await
    }

    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        self.call_unit(ReqMethod::RenameProject {
            project_id: id.0.to_string(),
            name,
        })
        .await
    }

    async fn delete_project(&self, id: &ProjectId) -> Result<crate::storage::GcReport> {
        self.call_json(ReqMethod::DeleteProject {
            project_id: id.0.to_string(),
        })
        .await
    }

    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        let sid: String = self
            .call_json(ReqMethod::CreateSession {
                project_id: input.project_id.map(|p| p.0.to_string()),
                working_dir: input.working_dir.map(|p| p.to_string_lossy().to_string()),
                auto_approve_level: input.auto_approve_level,
                model_key: input.model_key,
            })
            .await?;
        Ok(SessionId::from(sid))
    }

    async fn restore_session(&self, id: &SessionId) -> Result<SessionId> {
        let sid: String = self
            .call_json(ReqMethod::RestoreSession {
                session_id: id.0.to_string(),
            })
            .await?;
        Ok(SessionId::from(sid))
    }

    async fn fork_session(
        &self,
        parent: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        let sid: String = self
            .call_json(ReqMethod::ForkSession {
                parent_id: parent.0.to_string(),
                auto_approve_level,
            })
            .await?;
        Ok(SessionId::from(sid))
    }

    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        self.call_unit(ReqMethod::SendMessage {
            session_id: session_id.0.to_string(),
            blocks,
        })
        .await
    }

    async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        self.call_unit(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Cancel,
        })
        .await
    }

    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        self.call_unit(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Response {
                req_id: req_id.to_string(),
                approved,
                remember,
            },
        })
        .await
    }

    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        self.call_unit(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::SetLevel(level),
        })
        .await
    }

    async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        self.call_unit(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Compact,
        })
        .await
    }

    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()> {
        self.call_unit(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Rewind { message_id, target },
        })
        .await
    }

    async fn rename_session(&self, session_id: &SessionId, title: String) -> Result<()> {
        self.call_unit(ReqMethod::RenameSession {
            session_id: session_id.0.to_string(),
            title,
        })
        .await
    }

    async fn pin_session(&self, session_id: &SessionId, emoji: Option<String>) -> Result<()> {
        self.call_unit(ReqMethod::PinSession {
            session_id: session_id.0.to_string(),
            icon_emoji: emoji,
        })
        .await
    }

    async fn unpin_session(&self, session_id: &SessionId) -> Result<()> {
        self.call_unit(ReqMethod::UnpinSession {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn set_pinned_session_emoji(
        &self,
        session_id: &SessionId,
        emoji: Option<String>,
    ) -> Result<()> {
        self.call_unit(ReqMethod::SetPinnedSessionEmoji {
            session_id: session_id.0.to_string(),
            icon_emoji: emoji,
        })
        .await
    }

    async fn list_pinned_sessions(
        &self,
    ) -> Result<Vec<crate::storage::pinned_session::PinnedSessionDetail>> {
        self.call_json(ReqMethod::ListPinnedSessions).await
    }

    async fn add_favorite(
        &self,
        input: crate::storage::AddFavoriteInput,
    ) -> Result<crate::storage::FavoriteAnswer> {
        self.call_json(ReqMethod::AddFavorite { input }).await
    }

    async fn remove_favorite(&self, id: &str) -> Result<()> {
        self.call_unit(ReqMethod::RemoveFavorite {
            favorite_id: id.to_string(),
        })
        .await
    }

    async fn remove_favorite_by_message(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<()> {
        self.call_unit(ReqMethod::RemoveFavoriteByMessage {
            session_id: session_id.0.to_string(),
            message_id: message_id.0.to_string(),
        })
        .await
    }

    async fn list_favorites(
        &self,
        query: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::storage::FavoriteAnswer>> {
        self.call_json(ReqMethod::ListFavorites {
            query,
            limit,
            offset,
        })
        .await
    }

    async fn update_favorite_note(&self, id: &str, note: Option<String>) -> Result<()> {
        self.call_unit(ReqMethod::UpdateFavoriteNote {
            favorite_id: id.to_string(),
            note,
        })
        .await
    }

    async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.call_unit(ReqMethod::DeleteSession {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn clear_session(&self, session_id: &SessionId) -> Result<()> {
        self.call_unit(ReqMethod::ClearSession {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn mailbox_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::comms::MailboxSnapshot> {
        self.call_json(ReqMethod::MailboxSnapshot {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn remove_mailbox_item(&self, session_id: &SessionId, item_id: &str) -> Result<bool> {
        let value = self
            .call(ReqMethod::RemoveMailboxItem {
                session_id: session_id.0.to_string(),
                item_id: item_id.to_string(),
            })
            .await?;
        Ok(value["removed"].as_bool().unwrap_or(false))
    }

    async fn steer_mailbox_item(&self, session_id: &SessionId, item_id: &str) -> Result<bool> {
        let value = self
            .call(ReqMethod::SteerMailboxItem {
                session_id: session_id.0.to_string(),
                item_id: item_id.to_string(),
            })
            .await?;
        Ok(value["moved"].as_bool().unwrap_or(false))
    }

    async fn clear_mailbox(
        &self,
        session_id: &SessionId,
        scope: crate::comms::MailboxScope,
    ) -> Result<usize> {
        let value = self
            .call(ReqMethod::ClearMailbox {
                session_id: session_id.0.to_string(),
                scope,
            })
            .await?;
        Ok(value["removed"].as_u64().unwrap_or(0) as usize)
    }

    async fn list_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::SessionMessage>> {
        self.call_json(ReqMethod::ListMessages {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn get_session(&self, session_id: &SessionId) -> Result<crate::types::SessionResponse> {
        self.call_json(ReqMethod::GetSession {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn get_session_rules(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::types::SessionRulesResponse> {
        self.call_json(ReqMethod::GetRules {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn read_session_jsonl(
        &self,
        session_id: &SessionId,
        before_offset: Option<u64>,
        after_offset: Option<u64>,
    ) -> Result<SessionJsonlChunk> {
        self.call_json(ReqMethod::ReadSessionJsonl {
            session_id: session_id.0.to_string(),
            before_offset,
            after_offset,
        })
        .await
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
        after_event_id: Option<crate::types::EventId>,
    ) -> Result<crate::comms::EventBusSubscriber> {
        self.subscribe_events_internal(session_id, after_event_id)
            .await
    }

    async fn subscribe_all_events(&self) -> Result<crate::comms::EventBusSubscriber> {
        use dashmap::mapref::entry::Entry;

        let tx = match self.event_routers.entry(ALL_EVENTS_ROUTER_KEY.to_string()) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let (tx, _rx) = broadcast::channel(256);
                entry.insert(tx.clone());
                tx
            }
        };

        self.call(ReqMethod::SubscribeAll).await?;

        let mut broadcast_rx = tx.subscribe();
        let (mpsc_tx, mpsc_rx) = mpsc::channel::<(SessionId, crate::wire::Envelope)>(256);
        tokio::spawn(async move {
            while let Ok(ev) = broadcast_rx.recv().await {
                let sid = ev.session_id.clone();
                if mpsc_tx.send((sid, ev)).await.is_err() {
                    break;
                }
            }
        });

        Ok(crate::comms::EventBusSubscriber::from_receiver(mpsc_rx))
    }

    async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        scope: SessionListScope,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<PaginatedSessions> {
        self.call_json(ReqMethod::ListSessions {
            project_id: project_id.map(|p| p.0.to_string()),
            scope,
            before,
            limit,
        })
        .await
    }

    async fn list_running_sessions(&self) -> Result<Vec<crate::types::RunningSessionResponse>> {
        self.call_json(ReqMethod::ListRunningSessions).await
    }

    async fn list_subagents(
        &self,
        parent_session_id: &SessionId,
    ) -> Result<Vec<crate::types::SubagentResponse>> {
        self.call_json(ReqMethod::ListSubagents {
            parent_session_id: parent_session_id.0.to_string(),
        })
        .await
    }

    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        self.call_json(ReqMethod::GetCheckpoints {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        self.call_json(ReqMethod::GetTodos {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()> {
        self.call_unit(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::AskUserResponse {
                req_id: req_id.to_string(),
                answers: response.answers.into_iter().collect(),
            },
        })
        .await
    }

    async fn send_steer(&self, session_id: &SessionId, content: Vec<ContentBlock>) -> Result<()> {
        self.call_unit(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Steer { content },
        })
        .await
    }

    async fn send_continue(&self, session_id: &SessionId) -> Result<()> {
        self.call_unit(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Continue,
        })
        .await
    }

    async fn list_session_skills(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<Arc<crate::skill::Skill>>> {
        self.call_json(ReqMethod::ListSessionSkills {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn subscribe_notifications(&self) -> Result<mpsc::Receiver<Notification>> {
        let mut rx = self.notification_tx.subscribe();
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
        self.call_json(ReqMethod::GetUsageSummary { days: Some(days) })
            .await
    }

    async fn get_daily_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::DailyUsage>> {
        self.call_json(ReqMethod::GetDailyUsage { days }).await
    }

    async fn get_model_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::ModelUsage>> {
        self.call_json(ReqMethod::GetModelUsage { days }).await
    }

    async fn get_model_usage_since(
        &self,
        start: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::storage::usage::ModelUsage>> {
        self.call_json(ReqMethod::GetModelUsageSince { start })
            .await
    }

    async fn get_usage_records(
        &self,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::storage::usage::UsageRecord>> {
        self.call_json(ReqMethod::GetUsageRecords {
            before_id: before_id.map(String::from),
            limit,
        })
        .await
    }

    async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId> {
        #[derive(serde::Deserialize)]
        struct JobIdResponse {
            job_id: String,
        }
        // 老 daemon 会静默忽略不认识的 precheck 字段——写入后回读校验，
        // 宁可显式报错也不让"以为设上了其实没有"溜过去。
        let expected_precheck = input
            .precheck
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .cloned();
        let result = self
            .call(ReqMethod::CreateCronJob {
                name: input.name,
                schedule: input.schedule,
                action: input.action,
                max_runs: input.max_runs,
                expires_at: input.expires_at,
                precheck: input.precheck,
            })
            .await?;
        let resp: JobIdResponse = serde_json::from_value(result)
            .map_err(|e| crate::types::KernelError::storage(format!("parse job_id: {e}")))?;
        let id = crate::cron::CronJobId::from(resp.job_id);
        if let Some(expected) = expected_precheck {
            let stored = self.get_cron_job(&id).await?.and_then(|j| j.precheck);
            if stored.as_deref() != Some(expected.as_str()) {
                return Err(crate::types::KernelError::storage(format!(
                    "precheck was not stored (got {stored:?}). Either the job name already \
                     exists (create is idempotent and does not modify it — use update), or \
                     the daemon is too old to support precheck (upgrade the daemon)"
                )));
            }
        }
        Ok(id)
    }

    async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>> {
        self.call_json(ReqMethod::ListCronJobs {
            status: status.map(|s| s.as_str().to_string()),
            limit,
        })
        .await
    }

    async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>> {
        self.call_json(ReqMethod::GetCronJob {
            job_id: id.0.to_string(),
        })
        .await
    }

    async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool> {
        // 同 create：老 daemon 静默丢 precheck 字段时显式报错。
        let precheck_update = input.precheck.clone();
        let updated: bool = self
            .call_json(ReqMethod::UpdateCronJob {
                job_id: id.0.to_string(),
                name: input.name,
                schedule: input.schedule,
                action: input.action,
                status: input.status.map(|s| s.as_str().to_string()),
                max_runs: input.max_runs,
                expires_at: input.expires_at,
                precheck: input.precheck,
            })
            .await?;
        if updated {
            if let Some(v) = precheck_update {
                let expected = if v.trim().is_empty() { None } else { Some(v) };
                let stored = self.get_cron_job(id).await?.and_then(|j| j.precheck);
                if stored != expected {
                    return Err(crate::types::KernelError::storage(format!(
                        "precheck was not stored (got {stored:?}); \
                         the daemon is too old to support precheck (upgrade the daemon)"
                    )));
                }
            }
        }
        Ok(updated)
    }

    async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        self.call_json(ReqMethod::DeleteCronJob {
            job_id: id.0.to_string(),
        })
        .await
    }

    async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()> {
        self.call_unit(ReqMethod::TriggerCronJob {
            job_id: id.0.to_string(),
        })
        .await
    }

    async fn channel_new_thread(
        &self,
        channel: Option<String>,
        platform: Option<String>,
        chat_id: String,
        title: Option<String>,
        text: String,
    ) -> Result<serde_json::Value> {
        self.call(ReqMethod::ChannelNewThread {
            channel,
            platform,
            chat_id,
            title,
            text,
        })
        .await
    }

    async fn set_channel_watch(
        &self,
        channel: Option<String>,
        platform: Option<String>,
        chat_id: String,
        on: Option<bool>,
    ) -> Result<serde_json::Value> {
        self.call(ReqMethod::SetChannelWatch {
            channel,
            platform,
            chat_id,
            on,
        })
        .await
    }

    async fn list_models(&self) -> Result<Vec<crate::kernel::ModelInfo>> {
        self.call_json(ReqMethod::ListModels).await
    }

    async fn get_session_model(&self, session_id: &SessionId) -> Result<String> {
        self.call_json(ReqMethod::GetSessionModel {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()> {
        self.call_unit(ReqMethod::SetSessionModel {
            session_id: session_id.0.to_string(),
            key: key.to_string(),
        })
        .await
    }

    async fn get_session_context_window(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::kernel::ContextWindowInfo> {
        self.call_json(ReqMethod::GetSessionContextWindow {
            session_id: session_id.0.to_string(),
        })
        .await
    }

    async fn set_session_context_window(
        &self,
        session_id: &SessionId,
        tokens: Option<u32>,
    ) -> Result<()> {
        self.call_unit(ReqMethod::SetSessionContextWindow {
            session_id: session_id.0.to_string(),
            tokens,
        })
        .await
    }

    async fn list_agent_templates(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<Vec<crate::agent_tmpl::AgentTemplate>> {
        self.call_json(ReqMethod::ListAgentTemplates {
            session_id: session_id.map(|s| s.0.to_string()),
        })
        .await
    }

    async fn save_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
        body: &str,
    ) -> Result<()> {
        self.call_unit(ReqMethod::SaveAgentTemplate {
            session_id: session_id.map(|s| s.0.to_string()),
            scope,
            name: name.to_string(),
            body: body.to_string(),
        })
        .await
    }

    async fn delete_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
    ) -> Result<()> {
        self.call_unit(ReqMethod::DeleteAgentTemplate {
            session_id: session_id.map(|s| s.0.to_string()),
            scope,
            name: name.to_string(),
        })
        .await
    }
}

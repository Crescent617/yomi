//! Application layer - kernel and conductor management

pub mod conductor;
mod tasks;
pub use conductor::Conductor;

use crate::agent::{AgentConfig, AgentInput, AgentShared, AgentState};
use crate::comms::InputBus;
use crate::notification::{AgentActivity, Notification};
use crate::permission::Level;
use crate::storage::usage::{DailyUsage, ModelUsage, UsageRecord, UsageSummary};
use crate::storage::{MessageStore, ProjectStore, SessionStore, StorageSet, UsageStore};
use crate::tools::AskUserResponse;
use crate::types::{KernelError, Project, ProjectId, Result, SessionError, SessionId};
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Model info for GUI/API listing
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub model_id: String,
    pub provider: String,
    pub context_window: u32,
}

/// Input for creating a new session
#[derive(Debug, Clone)]
pub struct CreateSessionInput {
    pub project_id: Option<ProjectId>,
    /// `None` 时继承 `project_id` 的项目目录（创建时定型）；两者都缺省
    /// 则运行时回落 `<data_dir>/workspace`。
    pub working_dir: Option<std::path::PathBuf>,
    /// `None` 在创建时回落到配置的 `auto_approve`（创建时定型存储，
    /// 之后修改配置不影响已建会话）。
    pub auto_approve_level: Option<Level>,
    pub tool_blocklist: Vec<String>,
    /// Initial model key persisted for this session. When absent, runtime
    /// model resolution falls back to `agent.default_model` without storing it.
    pub model_key: Option<String>,
}

pub struct Kernel {
    agent_shared: Arc<AgentShared>,
    input_bus: Arc<InputBus>,
    conductor: Arc<Conductor>,
    /// Full storage set (for gc / cascade deletion)
    storage: StorageSet,
    /// Default agent configuration for new sessions.
    agent_config: AgentConfig,
    /// Read-only model registry (`BTreeMap` for ordering), built from Config.models
    models: Arc<std::collections::BTreeMap<String, crate::provider::ModelConfig>>,
    /// Configuration for lightweight model-backed tasks.
    tasks_config: crate::config::TasksConfig,
    /// Garbage-collection policy for session resources (incl. auto-run settings).
    gc_config: crate::config::GcConfig,
    /// Whether model-backed session title generation is enabled.
    update_session_title: bool,
    /// Project store for project operations
    project_store: Arc<dyn ProjectStore>,
    /// Pinned session store for sidebar pinning and emoji.
    pinned_session_store: Arc<dyn crate::storage::PinnedSessionStore>,
    /// Favorite answer store for bookmarked assistant answers.
    favorite_store: Arc<dyn crate::storage::FavoriteStore>,
    /// Cron store for scheduled job operations.
    pub(crate) cron_store: Option<Arc<dyn crate::cron::CronStore>>,
    /// Shared slot for the running cron scheduler (shared with `AgentShared`).
    pub(crate) cron_scheduler: Arc<std::sync::Mutex<Option<Arc<crate::cron::CronScheduler>>>>,
    /// Shared slot for the daemon restart sink (filled by `KernelServer`).
    pub(crate) restart_tx: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<()>>>>,
    /// Disposable persistent KV cache (`cache.db`), shared with channel adapters.
    pub(crate) kv_cache: Option<Arc<crate::kv_cache::KvCache>>,
    pub(crate) channel_manager: Option<Arc<crate::channels::hub::ChannelHub>>,
    /// Global notification bus for state changes and other broadcasts.
    notification_bus: Arc<crate::notification::NotificationBus>,
    /// Global shutdown token for graceful stop.
    shutdown: tokio_util::sync::CancellationToken,
}

const SESSION_JSONL_CHUNK_BYTES: u64 = 256 * 1024;

/// Wall-clock duration until the next local midnight (fallback: 24h).
/// Recomputed before every auto-gc sleep, so clock changes and OS
/// suspend/resume self-correct on the next iteration.
fn duration_until_next_midnight() -> std::time::Duration {
    use chrono::TimeZone as _;
    let now = chrono::Local::now();
    let tomorrow = now.date_naive() + chrono::Duration::days(1);
    tomorrow
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| chrono::Local.from_local_datetime(&naive).single())
        .and_then(|midnight| (midnight - now).to_std().ok())
        .unwrap_or(std::time::Duration::from_hours(24))
}

impl Kernel {
    /// Get session store from `agent_shared`
    pub async fn session_store(&self) -> Arc<dyn SessionStore> {
        self.agent_shared
            .session_store
            .clone()
            .expect("session_store not configured")
    }

    /// List all channels and their status.
    pub fn list_channels(&self) -> Vec<crate::channels::ChannelInfo> {
        match &self.channel_manager {
            Some(mgr) => mgr.list_channels(),
            None => Vec::new(),
        }
    }

    /// Get the channel manager (if channels are configured).
    pub fn channel_manager(&self) -> Option<Arc<crate::channels::hub::ChannelHub>> {
        self.channel_manager.clone()
    }

    /// Get pinned session store
    pub fn pinned_session_store(&self) -> Arc<dyn crate::storage::PinnedSessionStore> {
        self.pinned_session_store.clone()
    }

    /// List background shell tasks belonging to a session.
    pub fn list_background_shells(
        &self,
        session_id: &SessionId,
    ) -> Vec<crate::agent::BackgroundShellTask> {
        self.agent_shared
            .background_tasks
            .shell_tasks_for(session_id)
    }

    /// List ALL tracked background shell tasks across sessions
    /// (`/bg --all`).
    pub fn list_all_background_shells(&self) -> Vec<crate::agent::BackgroundShellTask> {
        self.agent_shared.background_tasks.shell_tasks()
    }

    /// List running subagents across ALL parent sessions (`/bg --all`):
    /// `sub_`-prefixed sessions whose phase is not idle.
    pub async fn list_all_running_subagents(&self) -> Result<Vec<crate::types::SubagentResponse>> {
        let mut out = Vec::new();
        let mut before = None;
        loop {
            let page = self
                .list_sessions(
                    None,
                    crate::storage::session::SessionListScope::All,
                    before,
                    50,
                )
                .await?;
            let page_has_more = page.next_cursor.is_some();
            let last_updated = page.sessions.last().map(|s| s.updated_at);
            for info in page.sessions {
                if !info.id.0.starts_with(crate::types::SUB_PREFIX) {
                    continue;
                }
                let session = self.session_response(info);
                if session.phase == "idle" {
                    continue;
                }
                let parent = session
                    .parent_id
                    .clone()
                    .unwrap_or_else(|| SessionId::from(String::new()));
                out.push(crate::types::SubagentResponse {
                    id: session.id,
                    alias: session.title,
                    parent_session_id: parent,
                    phase: session.phase,
                    is_running: true,
                    model_key: session.model_key,
                    created_at: session.created_at,
                });
            }
            if !page_has_more {
                break;
            }
            let Some(ts) = last_updated else { break };
            before = Some(ts);
        }
        Ok(out)
    }

    /// SIGTERM a background shell's whole process GROUP by task id. The
    /// child called `setsid` at spawn (shell.rs), so its pid doubles as
    /// the process-group id and signaling the group can't hit the daemon
    /// — but DOES reach grandchildren (`sh -c "sleep 60"`'s sleep),
    /// which would otherwise survive as orphans. Only signals: cleanup
    /// (tracker removal + `BackgroundTasksChanged`) rides the normal
    /// guard lifecycle when the process actually exits — decrementing
    /// anything here would double-count against the guard's own Drop.
    /// Returns false when the task id is unknown (already gone).
    pub async fn kill_background_shell(&self, session_id: &SessionId, task_id: &str) -> bool {
        let Some(task) = self
            .list_background_shells(session_id)
            .into_iter()
            .find(|t| t.task_id == task_id)
        else {
            return false;
        };
        match tokio::process::Command::new("kill")
            .args(["-TERM", "--", &format!("-{}", task.pid)])
            .status()
            .await
        {
            Ok(status) => status.success(),
            Err(e) => {
                tracing::warn!(pid = task.pid, error = %e, "failed to SIGTERM background shell");
                false
            }
        }
    }

    /// Get favorite answer store
    pub fn favorite_store(&self) -> Arc<dyn crate::storage::FavoriteStore> {
        self.favorite_store.clone()
    }

    /// Get message store from `agent_shared`
    pub async fn message_store(&self) -> Arc<dyn MessageStore> {
        self.agent_shared
            .message_store
            .clone()
            .expect("message_store not configured")
    }

    /// The session's last recorded context occupancy in tokens (the most
    /// recent message's usage), for `/info`-style displays. `None` when no
    /// usage has been recorded or no message store is configured.
    pub async fn get_session_context_tokens(&self, session_id: &SessionId) -> Option<u32> {
        let store = self.agent_shared.message_store.as_ref()?;
        let messages = store
            .get(&session_id.0)
            .await
            .map_err(|e| {
                tracing::warn!(session_id = %session_id.0, error = %e, "context-token read failed");
                e
            })
            .ok()?;
        messages
            .iter()
            .rev()
            .find_map(|m| m.token_usage.map(|u| u.total_tokens))
    }

    /// Get checkpoint store from `agent_shared`
    pub async fn checkpoint_store(&self) -> Arc<dyn crate::checkpoint::CheckpointStore> {
        self.agent_shared
            .checkpoint_store
            .clone()
            .expect("checkpoint_store not configured")
    }

    /// Get cron store if configured.
    pub fn cron_store(&self) -> Option<Arc<dyn crate::cron::CronStore>> {
        self.cron_store.clone()
    }

    /// Shared slot for the cron scheduler. `KernelServer` adopts this slot and
    /// fills it on start, so agents (cron tool) and the RPC dispatcher notify
    /// the same scheduler instance.
    pub fn cron_scheduler_slot(
        &self,
    ) -> Arc<std::sync::Mutex<Option<Arc<crate::cron::CronScheduler>>>> {
        Arc::clone(&self.cron_scheduler)
    }

    /// Shared slot for the daemon restart sink. `KernelServer` fills it
    /// when the daemon lifecycle supports restart (otherwise `None`).
    pub fn restart_slot(&self) -> Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<()>>>> {
        Arc::clone(&self.restart_tx)
    }

    /// Whether this kernel runs under a daemon that supports restart.
    pub fn can_restart(&self) -> bool {
        self.restart_tx.lock().unwrap().is_some()
    }

    /// Request a daemon restart. A request already in flight wins, making
    /// concurrent calls no-ops — the restart happens either way.
    pub fn request_restart(&self) {
        if let Some(tx) = self.restart_tx.lock().unwrap().clone() {
            let _ = tx.try_send(());
        }
    }

    /// Get the disposable persistent KV cache (`None` when it failed to open).
    pub fn kv_cache(&self) -> Option<Arc<crate::kv_cache::KvCache>> {
        self.kv_cache.clone()
    }

    /// Get data directory from `agent_shared`
    pub async fn data_dir(&self) -> std::path::PathBuf {
        self.agent_shared.data_dir.clone()
    }

    /// The session's effective working directory: its stored `working_dir`,
    /// or the default workspace — the same rule the conductor applies at
    /// spawn time, including when the store has no entry for the session.
    pub(crate) async fn session_cwd(&self, session_id: &SessionId) -> std::path::PathBuf {
        let stored = self
            .session_store()
            .await
            .get(session_id)
            .await
            .ok()
            .flatten()
            .and_then(|info| info.working_dir);
        crate::utils::path::session_workspace_dir(
            &self.agent_shared.data_dir,
            stored.map(Into::into),
        )
    }

    /// Get usage store from `agent_shared`
    pub async fn usage_store(&self) -> Arc<dyn UsageStore> {
        self.agent_shared
            .usage_store
            .clone()
            .expect("usage_store not configured")
    }

    /// Get goal store from `agent_shared`
    pub async fn goal_store(&self) -> Arc<dyn crate::goal::GoalStore> {
        self.agent_shared
            .goal_store
            .clone()
            .expect("goal_store not configured")
    }

    /// Get todo store from `agent_shared`
    pub async fn todo_store(&self) -> Arc<dyn crate::storage::TodoStore> {
        self.agent_shared
            .todo_storage
            .clone()
            .expect("todo_storage not configured")
    }

    /// Get the global notification bus.
    pub fn notification_bus(&self) -> Arc<crate::notification::NotificationBus> {
        self.notification_bus.clone()
    }

    /// Create a new kernel.
    ///
    /// # Errors
    /// Returns a config error when two `[[models]]` entries share the same `name`
    /// (note: `name` defaults to `"default"` when omitted, so multiple unnamed
    /// entries collide).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: &StorageSet,
        agent_config: AgentConfig,
        task_store: Option<Arc<crate::tools::task::TaskStore>>,
        compactor: Option<crate::compactor::Compactor>,
        skill_folders: Vec<std::path::PathBuf>,
        enable_cron: bool,
        channel_store: Option<Arc<dyn crate::channels::ChannelStore>>,
        models: Vec<crate::provider::ModelConfig>,
        tasks_config: crate::config::TasksConfig,
        gc_config: crate::config::GcConfig,
        update_session_title: bool,
        config_auto_approve: Level,
    ) -> Result<Arc<Self>> {
        let session_store = storage.session_store();
        let message_store = storage.message_store();
        let todo_storage = storage.todo_store();
        // Todo reminder interceptor is only wired up when the todo tool is enabled.
        let todo_interceptor = agent_config.enable_todo_tool.then(|| {
            Arc::new(crate::agent::TodoReminderInterceptor::new(
                todo_storage.clone(),
            ))
        });
        let checkpoint_store = storage.checkpoint_store();
        let data_dir = storage.data_dir().to_path_buf();
        let data_dir_for_conductor = data_dir.clone();
        let project_store = storage.project_store();
        let pinned_session_store = storage.pinned_session_store();
        let favorite_store = storage.favorite_store();
        let goal_store = storage.goal_store();

        // Build model registry (BTreeMap for ordering); reject duplicate names
        // instead of silently letting the last entry win.
        let mut models_map = std::collections::BTreeMap::new();
        for m in models {
            if let Some(prev) = models_map.insert(m.name.clone(), m) {
                return Err(KernelError::Config(format!(
                    "Duplicate model name '{}' in [[models]] (model_id '{}' would be \
                     shadowed). Give each model a unique `name` — note that `name` \
                     defaults to \"default\" when omitted.",
                    prev.name, prev.model_id
                )));
            }
        }
        let models_map: Arc<std::collections::BTreeMap<String, crate::provider::ModelConfig>> =
            Arc::new(models_map);

        if !models_map.contains_key(&agent_config.default_model) {
            tracing::warn!(
                "default_model '{}' not found in models; sessions without a valid \
                 model_key will fail to resolve a model",
                agent_config.default_model
            );
        }

        let cron_store = if enable_cron {
            Some(storage.cron_store())
        } else {
            None
        };
        // Shared with `KernelServer`, which fills in the running scheduler on
        // start so that agents (cron tool) can notify it of job changes.
        let cron_scheduler = Arc::new(std::sync::Mutex::new(None));

        let agent_shared = AgentShared::with_data_dir(
            Arc::clone(&models_map),
            agent_config.default_model.clone(),
            task_store,
            Some(todo_storage),
            compactor,
            Some(session_store),
            Some(message_store),
            Some(storage.usage_store()),
            None,
            skill_folders,
            None,
            Some(checkpoint_store),
            data_dir,
        )
        .with_goal_store(goal_store)
        .with_cron(cron_store.clone(), Arc::clone(&cron_scheduler))
        .with_config_auto_approve(config_auto_approve);

        let agent_shared = match todo_interceptor {
            Some(interceptor) => agent_shared.with_message_interceptor(interceptor),
            None => agent_shared,
        };

        let channel_manager =
            channel_store.map(|store| Arc::new(crate::channels::hub::ChannelHub::new(store)));

        let agent_shared = agent_shared.with_channel_manager(channel_manager.clone());
        let event_bus = crate::comms::EventBus::new();
        let agent_shared = agent_shared.with_event_bus(event_bus.clone());
        let agent_shared = Arc::new(agent_shared);

        let input_bus = InputBus::new();
        let rx = input_bus.subscribe_all();
        let base_prompt = agent_config.system_prompt.clone();
        let notification_bus = Arc::new(crate::notification::NotificationBus::new());
        agent_shared
            .background_tasks
            .set_notification_bus(notification_bus.clone());
        let conductor = Arc::new(Conductor::new(
            agent_shared.clone(),
            agent_config.clone(),
            rx,
            event_bus,
            input_bus.clone(),
            base_prompt,
            data_dir_for_conductor,
            notification_bus.clone(),
        ));
        let cron_store = if enable_cron {
            Some(storage.cron_store())
        } else {
            None
        };

        Ok(Arc::new(Self {
            agent_shared,
            input_bus,
            conductor,
            storage: storage.clone(),
            agent_config,
            models: models_map,
            tasks_config,
            gc_config,
            update_session_title,
            project_store,
            pinned_session_store,
            favorite_store,
            cron_store,
            cron_scheduler,
            restart_tx: Arc::new(std::sync::Mutex::new(None)),
            kv_cache: storage.kv_cache(),
            channel_manager,
            notification_bus,
            shutdown: tokio_util::sync::CancellationToken::new(),
        }))
    }

    pub fn start(&self) {
        let conductor = self.conductor.clone();
        let token = self.shutdown.clone();
        tokio::spawn(async move { conductor.run(token).await });
        self.start_auto_gc();
    }

    /// Garbage-collect expired session resources when `[gc] auto` is enabled.
    /// Runs once at startup (catching up for downtime and missed midnights)
    /// and then every day at local midnight; per-run failures are logged and
    /// never interrupt the loop.
    fn start_auto_gc(&self) {
        if !self.gc_config.auto {
            return;
        }
        let storage = self.storage.clone();
        let base_opts = crate::storage::GcOptions::from_config(&self.gc_config, false);
        let conductor = self.conductor.clone();
        let token = self.shutdown.child_token();
        tokio::spawn(async move {
            loop {
                // Refresh exclusions every round: sessions with a live agent
                // must keep their data even when long expired.
                let mut opts = base_opts.clone();
                opts.exclude_sessions = conductor.loaded_session_ids();
                match storage.gc().run(&opts).await {
                    Ok(report) => {
                        tracing::info!(
                            sessions = report.sessions.len(),
                            orphan_files = report.orphan_files_deleted,
                            assets = report.assets_deleted,
                            bytes_reclaimed = report.bytes_reclaimed,
                            errors = report.errors.len(),
                            "auto gc completed"
                        );
                    }
                    Err(e) => tracing::warn!("auto gc failed: {e}"),
                }
                tokio::select! {
                    biased;
                    () = token.cancelled() => break,
                    () = tokio::time::sleep(duration_until_next_midnight()) => {}
                }
            }
        });
    }

    /// Gracefully stop the kernel and all background tasks.
    pub fn stop(&self) {
        self.shutdown.cancel();
        self.input_bus.shutdown();
        if let Some(ref bus) = self.agent_shared.event_bus {
            bus.shutdown();
        }
    }

    // ── Project API ──────────────────────────────────────────────────────

    /// Create a new project.
    /// If a project already exists for the given directory, returns the existing one.
    pub async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        let abs = tokio::fs::canonicalize(&dir).await.unwrap_or(dir);
        let dir_str = abs
            .to_str()
            .ok_or_else(|| SessionError::Other("Invalid project directory path".to_string()))?;

        // Check for existing project by directory
        if let Some(existing) = self.project_store.get_by_dir(dir_str).await? {
            return Ok(existing);
        }

        let name = name.unwrap_or_else(|| {
            abs.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unnamed")
                .to_string()
        });
        let id = ProjectId::new();
        self.project_store.create(&id, &name, dir_str).await?;
        Ok(Project {
            id,
            name,
            dir: abs,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// List all projects
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        self.project_store.list().await
    }

    /// Get project by ID
    pub async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        self.project_store.get(id).await
    }

    /// Rename a project
    pub async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        self.project_store.update_name(id, &name).await
    }

    /// Rename a session (update title in storage, normalized: collapse whitespace, truncated to 20 chars).
    pub async fn rename_session(&self, id: &SessionId, title: String) -> Result<()> {
        let _guard = crate::utils::g_lock::g_lock(session_title_lock_key(id)).await;
        let title = normalize_session_title(&title);
        self.session_store().await.update_title(id, &title).await?;
        let noti = crate::notification::Notification::TitleUpdated {
            session_id: id.clone(),
            title: title.clone(),
        };
        let _ = self.notification_bus.send(noti);
        Ok(())
    }

    /// Pin a session to the top of the sidebar, optionally with an emoji.
    pub async fn pin_session(&self, id: &SessionId, emoji: Option<String>) -> Result<()> {
        self.pinned_session_store().pin(id, emoji.as_deref()).await
    }

    /// Unpin a session from the sidebar.
    pub async fn unpin_session(&self, id: &SessionId) -> Result<()> {
        self.pinned_session_store().unpin(id).await
    }

    /// Update the emoji for a pinned session.
    pub async fn set_pinned_session_emoji(
        &self,
        id: &SessionId,
        emoji: Option<String>,
    ) -> Result<()> {
        self.pinned_session_store()
            .update_emoji(id, emoji.as_deref())
            .await
    }

    /// List pinned sessions with their session metadata.
    pub async fn list_pinned_sessions(&self) -> Result<Vec<crate::storage::PinnedSessionDetail>> {
        self.pinned_session_store().list_with_details().await
    }

    // ── Favorites ────────────────────────────────────────────────────────

    /// Favorite an assistant answer (snapshots its content).
    pub async fn add_favorite(
        &self,
        input: crate::storage::AddFavoriteInput,
    ) -> Result<crate::storage::FavoriteAnswer> {
        self.favorite_store().add(input).await
    }

    /// Remove a favorite by id.
    pub async fn remove_favorite(&self, id: &str) -> Result<()> {
        self.favorite_store().remove(id).await
    }

    /// Remove a favorite by its source message.
    pub async fn remove_favorite_by_message(
        &self,
        session_id: &SessionId,
        message_id: &crate::types::MessageId,
    ) -> Result<()> {
        self.favorite_store()
            .remove_by_message(session_id, message_id)
            .await
    }

    /// List favorited answers, most recent first.
    pub async fn list_favorites(
        &self,
        query: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::storage::FavoriteAnswer>> {
        self.favorite_store()
            .list(query.as_deref(), limit, offset)
            .await
    }

    /// Update the note on a favorite.
    pub async fn update_favorite_note(&self, id: &str, note: Option<String>) -> Result<()> {
        self.favorite_store().update_note(id, note.as_deref()).await
    }

    /// Delete a project **and all its sessions** (including subagent children)
    /// with their resources: message history, todos, goals, file states,
    /// checkpoints and channel mappings. `token_usage` rows are kept.
    ///
    /// Running agents of affected sessions are cancelled (best-effort) before
    /// deletion. Returns a [`crate::storage::GcReport`] describing what was
    /// removed.
    pub async fn delete_project(&self, id: &ProjectId) -> Result<crate::storage::GcReport> {
        let session_ids = self.session_store().await.list_ids_by_project(id).await?;

        // Best-effort: cancel any running agent so it stops writing to
        // storage while we delete underneath it.
        for sid in &session_ids {
            self.cancel(sid);
        }

        let report = self.storage.gc().purge_sessions(&session_ids).await?;
        self.project_store.delete(id).await?;

        tracing::info!(
            project_id = %id.0,
            sessions = report.sessions.len(),
            files = report.files_deleted,
            checkpoints = report.checkpoint_dirs_deleted,
            bytes = report.bytes_reclaimed,
            "project deleted (cascade)"
        );
        Ok(report)
    }

    // ── Model API ────────────────────────────────────────────────────────

    /// List all available models (sorted by name)
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self
            .models
            .values()
            .map(|m| ModelInfo {
                name: m.name.clone(),
                model_id: m.model_id.clone(),
                provider: m.provider.to_string(),
                context_window: m.context_window,
            })
            .collect())
    }

    /// Get the current model name for a session (falls back to `default_model`)
    pub async fn get_session_model(&self, session_id: &SessionId) -> String {
        match self.session_store().await.get(session_id).await {
            Ok(Some(info)) => info
                .model_key
                .unwrap_or_else(|| self.agent_config.default_model.clone()),
            _ => self.agent_config.default_model.clone(),
        }
    }

    /// The configured default model key.
    pub fn default_model_key(&self) -> String {
        self.agent_config.default_model.clone()
    }

    /// Set the model for a session (persisted to database)
    pub async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()> {
        if !self.models.contains_key(key) {
            return Err(SessionError::Other(format!("Model '{key}' not found in config")).into());
        }
        let rows_affected = self
            .session_store()
            .await
            .update_model_key(session_id, key)
            .await?;
        if rows_affected == 0 {
            return Err(SessionError::NotFound {
                session_id: session_id.0.to_string(),
            }
            .into());
        }
        Ok(())
    }

    /// Clear the session's model override — it follows the configured
    /// default again (and picks up future default changes).
    pub async fn clear_session_model(&self, session_id: &SessionId) -> Result<()> {
        let rows_affected = self
            .session_store()
            .await
            .clear_model_key(session_id)
            .await?;
        if rows_affected == 0 {
            return Err(SessionError::NotFound {
                session_id: session_id.0.to_string(),
            }
            .into());
        }
        Ok(())
    }

    // ── Session API ──────────────────────────────────────────────────────

    /// Create a new session with the given input.
    #[tracing::instrument(skip(self, input))]
    pub async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        let project = match &input.project_id {
            Some(pid) => Some(
                self.project_store
                    .get(pid)
                    .await?
                    .ok_or_else(|| SessionError::Other(format!("Project {} not found", pid.0)))?,
            ),
            None => None,
        };

        // working_dir 缺省时继承项目目录（创建时定型存储），而不是运行时
        // 才回落 data_dir/workspace——定型后各处的 cwd 解析（spawn、
        // workspace 技能/模板、渠道附件投递）自然一致。
        let working_dir = if let Some(p) = input.working_dir {
            let canonical = tokio::fs::canonicalize(&p).await;
            Some(canonical.unwrap_or(p).to_string_lossy().to_string())
        } else {
            project
                .as_ref()
                .map(|p| p.dir.to_string_lossy().to_string())
        };

        let id = SessionId::new();

        // 未指定审批级别时回落到配置 auto_approve
        let auto_approve_level = input
            .auto_approve_level
            .unwrap_or(self.agent_shared.config_auto_approve);

        self.session_store()
            .await
            .create(crate::storage::NewSession {
                project_id: input.project_id.clone(),
                working_dir,
                auto_approve_level: Some(auto_approve_level.as_str().to_string()),
                model_key: input.model_key.clone(),
                ..crate::storage::NewSession::new(id.clone())
            })
            .await?;

        if let Some(ref pid) = input.project_id {
            let _ = self.project_store.touch(pid).await;
        }
        tracing::info!("created");
        Ok(id)
    }

    /// Restore a session from storage by its ID.
    pub async fn restore_session(&self, session_id: &SessionId) -> Result<SessionId> {
        let info = self
            .session_store()
            .await
            .get(session_id)
            .await?
            .ok_or_else(|| {
                KernelError::from(SessionError::NotFound {
                    session_id: session_id.0.to_string(),
                })
            })?;

        tracing::info!(session_id = %session_id.0, "Restoring from storage");
        Ok(info.id)
    }

    /// Fork a session: create new session with copied history from parent.
    /// Also copies message history, goal state, todo list, file states, and checkpoints.
    #[tracing::instrument(skip(self, auto_approve_level), fields(parent_id = %parent_id.0))]
    pub async fn fork_session(
        &self,
        parent_id: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        let parent_info = self
            .session_store()
            .await
            .get(parent_id)
            .await?
            .ok_or_else(|| {
                KernelError::from(SessionError::NotFound {
                    session_id: parent_id.0.to_string(),
                })
            })?;

        let new_id = self.session_store().await.fork(parent_id).await?;
        // Override the copied level with the requested one
        self.session_store()
            .await
            .update_auto_approve_level(&new_id, auto_approve_level.as_str())
            .await?;
        // Set title: "Forked: <parent_title>"
        let parent_title = parent_info.title.as_deref().unwrap_or("Untitled");
        let new_title = format!("Forked: {parent_title}");
        self.rename_session(&new_id, new_title).await?;
        tracing::info!("forked session {}", new_id.0);

        // Copy message history from parent to child
        let message_store = self.message_store().await;
        if let Ok(msgs) = message_store.get(&parent_id.0).await {
            if !msgs.is_empty() {
                if let Err(e) = message_store.replace(&new_id.0, &msgs).await {
                    tracing::warn!("failed to copy message history: {}", e);
                } else {
                    tracing::info!("copied {} messages", msgs.len());
                }
            }
        }

        // Copy goal state from parent to child
        let goal_store = self.goal_store().await;
        if let Ok(Some(goal)) = goal_store.load(&parent_id.0).await {
            if let Err(e) = goal_store.save(&new_id.0, &goal).await {
                tracing::warn!("failed to copy goal state: {}", e);
            } else {
                tracing::info!("copied goal state");
            }
        }

        // Copy todo list from parent to child
        let todo_store = self.todo_store().await;
        if let Ok(Some(todos)) = todo_store.load(&parent_id.0).await {
            if let Err(e) = todo_store.save(&new_id.0, &todos).await {
                tracing::warn!("failed to copy todo list: {}", e);
            } else {
                tracing::info!("copied todo list");
            }
        }

        // Copy file states from parent to child
        let data_dir = self.data_dir().await;
        let file_states_dir = data_dir.join("sessions").join("file_states");
        let parent_file_state =
            file_states_dir.join(format!("{}.jsonl", parent_id.0.replace(['/', '\\'], "_")));
        let child_file_state =
            file_states_dir.join(format!("{}.jsonl", new_id.0.replace(['/', '\\'], "_")));
        if parent_file_state.exists() {
            if let Err(e) = tokio::fs::copy(&parent_file_state, &child_file_state).await {
                tracing::warn!("failed to copy file state: {}", e);
            } else {
                tracing::info!("copied file state");
            }
        }

        // Copy checkpoints from parent to child
        let checkpoint_store = self.checkpoint_store().await;
        match checkpoint_store
            .copy_session_checkpoints(&parent_id.0, &new_id.0)
            .await
        {
            Ok(0) => tracing::debug!("no checkpoints to copy"),
            Ok(n) => tracing::info!("copied {} checkpoints", n),
            Err(e) => tracing::warn!("failed to copy checkpoints: {}", e),
        }

        tracing::info!("forked session {} initialized", new_id.0);
        Ok(new_id)
    }

    /// List sessions with cursor-based pagination.
    pub async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        scope: crate::storage::session::SessionListScope,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<crate::client::PaginatedSessions> {
        let (sessions, next_cursor) = self
            .session_store()
            .await
            .list(project_id, scope, before, limit)
            .await?;
        Ok(crate::client::PaginatedSessions {
            sessions,
            next_cursor,
        })
    }

    /// List running sessions from the authoritative in-memory agent registry,
    /// hydrated with persisted session metadata.
    pub async fn read_session_jsonl(
        &self,
        session_id: &SessionId,
        before_offset: Option<u64>,
        after_offset: Option<u64>,
    ) -> Result<crate::client::SessionJsonlChunk> {
        let safe_id = session_id.0.replace(['/', '\\'], "_");
        let path = self
            .agent_shared
            .data_dir
            .join("sessions")
            .join(format!("{safe_id}.jsonl"));
        tokio::task::spawn_blocking(move || {
            crate::utils::file_chunk::read_utf8_file_chunk(
                &path,
                before_offset,
                after_offset,
                SESSION_JSONL_CHUNK_BYTES,
                true,
            )
        })
        .await
        .map_err(|error| KernelError::io(format!("session JSONL reader failed: {error}")))?
        .map_err(|error| KernelError::io(format!("failed to read session JSONL: {error}")))
    }

    pub async fn list_running_sessions(&self) -> Result<Vec<crate::types::RunningSessionResponse>> {
        let shell_tasks_by_session = self
            .agent_shared
            .background_tasks
            .shell_tasks()
            .into_iter()
            .fold(
                std::collections::HashMap::<SessionId, Vec<crate::agent::BackgroundShellTask>>::new(
                ),
                |mut tasks, task| {
                    tasks.entry(task.session_id.clone()).or_default().push(task);
                    tasks
                },
            );
        let mut states: std::collections::HashMap<SessionId, AgentState> = self
            .conductor
            .running_sessions()
            .into_iter()
            .map(|snapshot| (snapshot.session_id, snapshot.state))
            .collect();
        let background_session_ids = self.agent_shared.background_tasks.active_session_ids();
        for session_id in background_session_ids {
            states.entry(session_id).or_insert(AgentState::Idle);
        }

        let store = self.session_store().await;
        let mut sessions = Vec::with_capacity(states.len());
        for (session_id, state) in states {
            let Some(info) = store.get(&session_id).await? else {
                // A newly spawned subagent can briefly precede its persisted row.
                continue;
            };
            let background_shells = shell_tasks_by_session
                .get(&info.id)
                .cloned()
                .unwrap_or_default();
            let background_task_count = [
                crate::agent::BackgroundTaskKind::Subagent,
                crate::agent::BackgroundTaskKind::Shell,
            ]
            .into_iter()
            .map(|kind| self.agent_shared.background_tasks.count(&info.id, kind))
            .sum();
            sessions.push(crate::types::RunningSessionResponse {
                id: info.id,
                parent_id: info.parent_id,
                title: info.title,
                project_id: info.project_id,
                phase: agent_state_phase(state).to_string(),
                background_task_count,
                background_shells,
            });
        }
        sessions.sort_by(|left, right| left.id.0.cmp(&right.id.0));
        Ok(sessions)
    }

    /// List direct subagent children with their current runtime phase.
    pub async fn list_subagents(
        &self,
        parent_session_id: &SessionId,
    ) -> Result<Vec<crate::types::SubagentResponse>> {
        let subagents = self
            .session_store()
            .await
            .list_subagents(parent_session_id)
            .await?;
        Ok(subagents
            .into_iter()
            .map(|info| {
                let session = self.session_response(info);
                let is_running = session.phase != "idle";
                crate::types::SubagentResponse {
                    id: session.id,
                    alias: session.title,
                    parent_session_id: session
                        .parent_id
                        .expect("list_subagents only returns sessions with a parent"),
                    phase: session.phase,
                    is_running,
                    model_key: session.model_key,
                    created_at: session.created_at,
                }
            })
            .collect())
    }

    /// Return the number of sessions currently live in memory.
    pub fn live_session_count(&self) -> usize {
        self.conductor.active_count()
    }

    /// Whether the session has a live agent task with an active (non-idle) run.
    pub fn is_session_running(&self, session_id: &SessionId) -> bool {
        self.conductor.is_running(session_id)
    }

    /// List skills available to a session, merging global skills with workspace skills.
    /// Workspace skills take precedence on name collision.
    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn list_session_skills(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<Arc<crate::skill::Skill>>> {
        let info = self
            .session_store()
            .await
            .get(session_id)
            .await?
            .ok_or_else(|| SessionError::NotFound {
                session_id: session_id.0.to_string(),
            })?;

        let cwd = Some(crate::utils::path::session_workspace_dir(
            &self.data_dir().await,
            info.working_dir.map(std::path::PathBuf::from),
        ));

        let workspace_skill_dir = match cwd.as_ref() {
            Some(dir) => crate::skill::workspace_skill_dir(dir).await,
            None => None,
        };

        let mut skills = self.agent_config.skills.clone();
        if let Some(dir) = workspace_skill_dir.as_ref() {
            skills = crate::skill::load_workspace_skills(dir, skills).await;
        }

        Ok(skills)
    }

    /// Resolve the cwd used to resolve workspace-layer agent templates of a
    /// session: session `working_dir` → `<data_dir>/workspace`.
    ///
    /// Deliberately mirrors the subagent spawn resolution
    /// (`tools/subagent.rs`, `kernel/conductor.rs`) — the panel must show
    /// what spawn sees. Sessions created with a project get the project
    /// dir stamped into `working_dir` at creation time, so the chain
    /// needs no project lookup (same for `list_session_skills`).
    /// `None` session means no workspace context.
    async fn session_asset_cwd(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<Option<std::path::PathBuf>> {
        let Some(sid) = session_id else {
            return Ok(None);
        };
        let info =
            self.session_store()
                .await
                .get(sid)
                .await?
                .ok_or_else(|| SessionError::NotFound {
                    session_id: sid.0.to_string(),
                })?;
        let cwd = crate::utils::path::session_workspace_dir(
            &self.data_dir().await,
            info.working_dir.map(std::path::PathBuf::from),
        );
        Ok(Some(cwd))
    }

    /// List effective agent templates (builtin → global → workspace merged),
    /// matching what the subagent tool resolves at spawn time.
    pub async fn list_agent_templates(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<Vec<crate::agent_tmpl::AgentTemplate>> {
        let cwd = self.session_asset_cwd(session_id).await?;
        let global = crate::agent_tmpl::global_dir(&self.data_dir().await);
        Ok(crate::agent_tmpl::list(&global, cwd.as_deref()).await)
    }

    async fn agent_template_root(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
    ) -> Result<std::path::PathBuf> {
        use crate::agent_tmpl::TemplateScope;
        match scope {
            TemplateScope::Global => Ok(crate::agent_tmpl::global_dir(&self.data_dir().await)),
            TemplateScope::Workspace => {
                let cwd = self.session_asset_cwd(session_id).await?.ok_or_else(|| {
                    SessionError::Other(
                        "workspace template scope requires a session context".to_string(),
                    )
                })?;
                Ok(cwd.join(crate::agent_tmpl::WORKSPACE_DIR))
            }
        }
    }

    /// Create or overwrite a custom template at the given scope.
    pub async fn save_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
        body: &str,
    ) -> Result<()> {
        let root = self.agent_template_root(session_id, scope).await?;
        crate::agent_tmpl::save(&root, name, body).await?;
        Ok(())
    }

    /// Delete a custom template layer; deleting an override reveals the
    /// lower layer again. Builtin templates are not on disk and thus
    /// unreachable.
    pub async fn delete_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
    ) -> Result<()> {
        let root = self.agent_template_root(session_id, scope).await?;
        crate::agent_tmpl::delete(&root, name).await?;
        Ok(())
    }

    /// Get full session info merged with runtime phase
    pub async fn get_session(&self, sid: &SessionId) -> Result<crate::types::SessionResponse> {
        let info = self.session_store().await.get(sid).await?;
        let info = info.ok_or_else(|| crate::types::SessionError::NotFound {
            session_id: sid.0.to_string(),
        })?;
        Ok(self.session_response(info))
    }

    fn session_response(
        &self,
        info: crate::storage::session::SessionInfo,
    ) -> crate::types::SessionResponse {
        let phase = agent_state_phase(
            self.conductor
                .get_state(&info.id)
                .unwrap_or(AgentState::Idle),
        );
        crate::types::SessionResponse {
            id: info.id,
            phase: phase.to_string(),
            title: info.title,
            parent_id: info.parent_id,
            project_id: info.project_id,
            working_dir: info.working_dir,
            message_count: info.message_count,
            created_at: info.created_at,
            updated_at: info.updated_at,
            auto_approve_level: info.auto_approve_level,
            model_key: info.model_key,
            template: info.template,
        }
    }

    /// Send a multi-modal message with content blocks
    #[tracing::instrument(skip(self, blocks), fields(session_id = %session_id.0))]
    pub async fn send_message(
        &self,
        session_id: &SessionId,
        blocks: Vec<crate::types::ContentBlock>,
    ) -> Result<()> {
        self.send_message_inner(session_id, blocks, true).await
    }

    pub(crate) async fn send_message_inner(
        &self,
        session_id: &SessionId,
        mut blocks: Vec<crate::types::ContentBlock>,
        update_title: bool,
    ) -> Result<()> {
        crate::utils::image::normalize_image_blocks(&mut blocks).await;
        let title_input = update_title
            .then(|| tasks::session_title::input_from_blocks(&blocks))
            .flatten();
        self.input_bus
            .publish(session_id.clone(), AgentInput::User { content: blocks })
            .map_err(|e| KernelError::io(format!("InputBus full: {e}")))?;

        if let Some(query) = title_input {
            self.update_session_title_after_message(session_id.clone(), query);
        }
        Ok(())
    }

    fn update_session_title_after_message(&self, session_id: SessionId, query: String) {
        let session_store = self
            .agent_shared
            .session_store
            .clone()
            .expect("session_store not configured");
        let notification_bus = Arc::clone(&self.notification_bus);
        let should_generate = tasks::session_title::should_generate(self.update_session_title);

        if should_generate {
            self.spawn_session_title_generation(session_id, query);
            return;
        }

        tokio::spawn(async move {
            let Some(_guard) =
                crate::utils::g_lock::g_try_lock(session_title_lock_key(&session_id))
            else {
                return;
            };
            let result = async {
                let current_title = session_store
                    .get(&session_id)
                    .await?
                    .and_then(|session| session.title);
                if current_title.is_some() {
                    return Result::<()>::Ok(());
                }
                let fallback = normalize_session_title(&query);
                if fallback.is_empty() {
                    return Ok(());
                }
                session_store.update_title(&session_id, &fallback).await?;
                let _ = notification_bus.send(crate::notification::Notification::TitleUpdated {
                    session_id: session_id.clone(),
                    title: fallback,
                });
                Ok(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(
                    session_id = %session_id.0,
                    %error,
                    "failed to set fallback session title"
                );
            }
        });
    }

    /// Subscribe to events for a session (to be called by TUI / GUI / remote client).
    /// Filters out `InternalEvent` — those are for kernel-internal consumers only.
    pub fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> crate::comms::EventBusSubscriber {
        self.agent_shared
            .event_bus
            .as_ref()
            .expect("event_bus must be configured")
            .subscribe_filtered(session_id.clone(), |envelope| {
                !matches!(envelope.event, crate::event::Event::Internal(_))
            })
    }

    /// Get the global event bus.
    pub fn event_bus(&self) -> Option<Arc<crate::comms::EventBus>> {
        self.agent_shared.event_bus.clone()
    }

    /// Send a continue command to trigger the agent from Idle to Streaming
    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn send_continue(&self, session_id: &SessionId) {
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Continue)
        {
            tracing::warn!("Failed to publish continue input: {}", e);
        }
    }

    #[tracing::instrument(skip(self, content), fields(session_id = %session_id.0))]
    pub async fn send_steer(
        &self,
        session_id: &SessionId,
        content: Vec<crate::types::ContentBlock>,
    ) {
        let mut content = mark_user_steer(content);
        crate::utils::image::normalize_image_blocks(&mut content).await;
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Steer(content))
        {
            tracing::warn!("Failed to publish steer input: {}", e);
        }
    }

    /// Pending mailbox contents (steer + queued user messages), FIFO —
    /// the management surface for frontends. Empty when no mailbox
    /// exists (nothing pending); transient control inputs are hidden.
    pub async fn mailbox_snapshot(&self, session_id: &SessionId) -> crate::comms::MailboxSnapshot {
        match self.conductor.mailbox(session_id) {
            Some(mb) => mb.snapshot().await,
            None => crate::comms::MailboxSnapshot::default(),
        }
    }

    /// Retract one pending mailbox item (best-effort: already consumed →
    /// false). Pending operations never touch session history.
    pub async fn remove_mailbox_item(&self, session_id: &SessionId, item_id: &str) -> bool {
        let Some(mb) = self.conductor.mailbox(session_id) else {
            return false;
        };
        let removed = mb.remove(&crate::types::MailboxItemId::from(item_id)).await;
        if removed {
            self.conductor.emit_mailbox_changed(session_id, &mb).await;
        } else {
            tracing::debug!(session_id = %session_id.0, item_id, "mailbox retract: item not pending");
        }
        removed
    }

    /// Promote a queued user message to a steer (atomic server-side move —
    /// the content never round-trips through the client). Returns false
    /// when the item is gone or not a queued user message.
    pub async fn steer_mailbox_item(&self, session_id: &SessionId, item_id: &str) -> bool {
        let Some(mb) = self.conductor.mailbox(session_id) else {
            return false;
        };
        let Some(AgentInput::User { content }) =
            mb.take(&crate::types::MailboxItemId::from(item_id)).await
        else {
            // Legit false paths: already consumed (double-click/race), or
            // a control input (Compact/Rewind) living in the normal queue.
            tracing::debug!(session_id = %session_id.0, item_id, "mailbox steer-promote: item not pending");
            return false;
        };
        mb.push_steer(mark_user_steer(content)).await;
        self.conductor.emit_mailbox_changed(session_id, &mb).await;
        true
    }

    /// Clear pending items by scope without cancelling the agent (unlike
    /// `cancel`, which clears both queues AND stops the run). Returns
    /// the number removed.
    pub async fn clear_mailbox(
        &self,
        session_id: &SessionId,
        scope: crate::comms::MailboxScope,
    ) -> usize {
        let Some(mb) = self.conductor.mailbox(session_id) else {
            return 0;
        };
        let removed = mb.clear_scope(scope).await;
        if removed > 0 {
            self.conductor.emit_mailbox_changed(session_id, &mb).await;
        }
        removed
    }

    /// Feed the session-title path with a user message's own text.
    /// Channel triggers merge context blocks (history, quoted message)
    /// ahead of the user's text before handing content to the agent,
    /// so the title input must come from the user message alone —
    /// extracting it from the merged content would title the session
    /// after someone else's chat history.
    pub(crate) fn note_title_input(&self, session_id: &SessionId, text: &str) {
        if let Some(query) = tasks::session_title::input_from_text(text) {
            self.update_session_title_after_message(session_id.clone(), query);
        }
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn cancel(&self, session_id: &SessionId) {
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Cancel)
        {
            tracing::warn!("Failed to publish cancel input: {}", e);
        }
    }

    /// Clear the session's agent context (messages, file state, todos, persisted history).
    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn clear_session(&self, session_id: &SessionId) -> Result<()> {
        self.input_bus
            .publish(session_id.clone(), AgentInput::Clear)
            .map_err(|e| {
                KernelError::Session(SessionError::Other(format!(
                    "Failed to publish clear input: {e}"
                )))
            })
    }

    pub fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        self.input_bus
            .publish(
                session_id.clone(),
                AgentInput::PermissionResponse {
                    req_id: req_id.to_string(),
                    approved,
                    remember,
                },
            )
            .map_err(|error| {
                KernelError::Session(SessionError::Other(format!(
                    "Failed to publish permission response: {error}"
                )))
            })?;
        self.publish_request_resolved(session_id, req_id);
        Ok(())
    }

    #[tracing::instrument(skip(self, response), fields(session_id = %session_id.0))]
    pub fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: AskUserResponse,
    ) -> Result<()> {
        self.input_bus
            .publish(
                session_id.clone(),
                AgentInput::AskUserResponse {
                    req_id: req_id.to_string(),
                    response,
                },
            )
            .map_err(|error| {
                KernelError::Session(SessionError::Other(format!(
                    "Failed to publish ask_user response: {error}"
                )))
            })?;
        self.publish_request_resolved(session_id, req_id);
        Ok(())
    }

    fn publish_request_resolved(&self, session_id: &SessionId, req_id: &str) {
        let _ = self.notification_bus.send(Notification::AgentActivity {
            session_id: session_id.clone(),
            event_id: format!("response:{req_id}"),
            activity: AgentActivity::RequestResolved {
                req_id: req_id.to_string(),
            },
        });
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        let rows = self
            .session_store()
            .await
            .update_auto_approve_level(session_id, level.as_str())
            .await?;
        if rows == 0 {
            tracing::warn!("set_permission_level: no rows updated — session may not exist in DB");
        } else {
            tracing::info!(
                "permission level persisted to DB as {:?} ({} row(s) affected)",
                level,
                rows,
            );
        }
        // Update in-memory permission state for the live agent (real-time)
        if self.conductor.set_permission_level(session_id, level) {
            tracing::info!(
                "permission level updated in-memory for live agent {}",
                session_id.0
            );
        }
        Ok(())
    }

    /// Request compaction for a session's message buffer
    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        self.input_bus
            .publish(session_id.clone(), AgentInput::Compact)
            .map_err(|e| KernelError::io(format!("InputBus full: {e}")))?;
        Ok(())
    }

    /// Rewind a session to a specific checkpoint
    #[tracing::instrument(skip(self, target), fields(session_id = %session_id.0))]
    pub async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: crate::types::MessageId,
        target: crate::checkpoint::RewindTarget,
    ) -> Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        self.input_bus
            .publish(
                session_id.clone(),
                AgentInput::Rewind {
                    message_id,
                    target,
                    result_tx: tx,
                },
            )
            .map_err(|e| KernelError::io(format!("InputBus full: {e}")))?;
        match rx.recv().await {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(KernelError::Checkpoint(e.to_string())),
            None => Err(KernelError::Checkpoint("Rewind channel closed".to_string())),
        }
    }

    #[tracing::instrument(skip(self, state), fields(session_id = %session_id.0))]
    pub async fn start_goal(
        &self,
        session_id: &SessionId,
        state: crate::goal::GoalState,
    ) -> Result<()> {
        let store = self.goal_store().await;
        store.save(&session_id.0, &state).await?;

        if let Err(e) = self.input_bus.publish(
            session_id.clone(),
            AgentInput::Steer(vec![crate::types::ContentBlock::Text {
                text: state.build_continue_prompt(),
            }]),
        ) {
            tracing::warn!("Failed to publish goal steer: {}", e);
        }

        if self.conductor.get_state(session_id) == Some(AgentState::Idle) {
            if let Err(e) = self
                .input_bus
                .publish(session_id.clone(), AgentInput::Continue)
            {
                tracing::warn!("Failed to publish goal continue: {}", e);
            }
        }

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalUpdated {
                        description: state.description.clone(),
                        status: state.status.as_str().to_string(),
                    }),
                ));
        }
        tracing::info!("goal mode started");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn pause_goal(&self, session_id: &SessionId) -> Result<()> {
        let store = self.goal_store().await;
        let mut state = store.load(&session_id.0).await?.ok_or_else(|| {
            crate::types::SessionError::Other("no active goal to pause".to_string())
        })?;
        state.status = crate::goal::GoalStatus::Paused;
        store.save(&session_id.0, &state).await?;

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalUpdated {
                        description: state.description.clone(),
                        status: state.status.as_str().to_string(),
                    }),
                ));
        }
        tracing::info!("goal paused");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn resume_goal(&self, session_id: &SessionId) -> Result<()> {
        let store = self.goal_store().await;
        let mut state = store
            .load(&session_id.0)
            .await?
            .ok_or_else(|| crate::types::SessionError::Other("no goal to resume".to_string()))?;
        state.status = crate::goal::GoalStatus::Active;
        store.save(&session_id.0, &state).await?;

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalUpdated {
                        description: state.description.clone(),
                        status: state.status.as_str().to_string(),
                    }),
                ));
        }
        tracing::info!("goal resumed");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn get_goal(&self, session_id: &SessionId) -> Result<Option<crate::goal::GoalState>> {
        self.goal_store().await.load(&session_id.0).await
    }

    #[tracing::instrument(skip(self, description), fields(session_id = %session_id.0))]
    pub async fn update_goal(
        &self,
        session_id: &SessionId,
        description: impl Into<String>,
    ) -> Result<()> {
        let description = description.into();
        let store = self.goal_store().await;
        let mut state = store
            .load(&session_id.0)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| crate::goal::GoalState::new(&description));

        state.description = description;
        state.status = crate::goal::GoalStatus::Active;
        store.save(&session_id.0, &state).await?;

        let prompt = state.objective_updated_prompt();
        let blocks = vec![crate::types::ContentBlock::Text { text: prompt }];
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Steer(blocks))
        {
            tracing::warn!("Failed to publish goal update steer: {}", e);
        }

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalUpdated {
                        description: state.description.clone(),
                        status: state.status.as_str().to_string(),
                    }),
                ));
        }
        tracing::info!("goal updated: {}", state.description);
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        self.goal_store().await.delete(&session_id.0).await?;

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalStopped),
                ));
        }
        tracing::info!("goal mode stopped");
        Ok(())
    }

    /// Delete a session from storage
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.session_store().await.delete(session_id).await
    }

    /// List messages for a session with a clean typed API (User / Assistant / Tool)
    pub async fn list_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::SessionMessage>> {
        let raw = self.message_store().await.get(&session_id.0).await?;
        Ok(crate::types::SessionMessage::from_storage(raw))
    }

    /// Get checkpoints for a session.
    pub async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        self.checkpoint_store()
            .await
            .get_session_checkpoints(&session_id.0)
            .await
    }

    /// Get todo JSON for a session.
    pub async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        match &self.agent_shared.todo_storage {
            Some(store) => store.load(&session_id.0).await,
            None => Ok(None),
        }
    }

    /// Get aggregated usage summary for the last N days
    pub async fn get_usage_summary(&self, days: i64) -> Result<UsageSummary> {
        let now = Utc::now();
        let start = now - chrono::Duration::days(days);
        self.usage_store().await.summarize(start, now, None).await
    }

    /// Get daily usage for the last N days
    pub async fn get_daily_usage(&self, days: i64) -> Result<Vec<DailyUsage>> {
        let now = Utc::now();
        let start = now - chrono::Duration::days(days);
        self.usage_store()
            .await
            .daily_summary(start, now, None)
            .await
    }

    /// Get usage aggregated by model for the last N days
    pub async fn get_model_usage(&self, days: i64) -> Result<Vec<ModelUsage>> {
        let start = Utc::now() - chrono::Duration::days(days);
        self.get_model_usage_since(start).await
    }

    /// Get usage aggregated by model since `start` (UTC)
    pub async fn get_model_usage_since(&self, start: DateTime<Utc>) -> Result<Vec<ModelUsage>> {
        self.usage_store()
            .await
            .by_model_summary(start, Utc::now(), None)
            .await
    }

    /// List raw usage records in reverse chronological order.
    pub async fn get_usage_records(
        &self,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<UsageRecord>> {
        self.usage_store()
            .await
            .list_records(before_id, limit)
            .await
    }

    // ── Cron Job API ──────────────────────────────────────────────────────
    //
    // All cron operations go through the Kernel so that clients (GUI, TUI,
    // CLI) can use the same `KernelApi` regardless of whether they are
    // talking to an in-process kernel or a remote daemon.  Never let the client
    // layer hold a `CronStore` directly — that would break remote mode.
    //
    // DESIGN PRINCIPLE: Every mutating cron operation (create / update / delete)
    // automatically notifies the scheduler to reload, so callers never need to
    // remember to do it manually.  This keeps both local (GUI in-process) and
    // remote (KernelServer) paths consistent.

    /// Notify the running scheduler (if any) that cron jobs changed.
    fn notify_cron_scheduler(&self) {
        let slot = self
            .cron_scheduler
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ref scheduler) = *slot {
            scheduler.reload();
        }
    }

    /// Create a new cron job.  Validates the schedule expression, computes the
    /// first `next_run_at`, persists, and notifies the scheduler.
    pub async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;

        // The RPC path has no caller session to follow, so a newly bound
        // session uses defaults.
        let session_store = self.session_store().await;
        let outcome = crate::cron::create_cron_job(
            store,
            Some(&session_store),
            None,
            input,
            self.agent_shared.config_auto_approve,
        )
        .await?;
        // 撞名返回既有 job 时没有任何变化，无需唤醒 scheduler。
        if outcome.created {
            self.notify_cron_scheduler();
        }
        Ok(outcome.job.id)
    }

    /// List cron jobs with optional status filter.
    pub async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;
        store.list(status, limit).await.map_err(|e| {
            crate::types::KernelError::storage(format!("Failed to list cron jobs: {e}"))
        })
    }

    /// Get a single cron job by ID.
    pub async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;
        store
            .get(id)
            .await
            .map_err(|e| crate::types::KernelError::storage(format!("Failed to get cron job: {e}")))
    }

    /// Update a cron job.  Validates the schedule if changed, recalculates
    /// `next_run_at`, persists.  Returns `true` if the job existed.
    pub async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        mut input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;

        if let Some(ref schedule_str) = input.schedule {
            input.next_run_at = Some(
                crate::cron::next_run_from_schedule(schedule_str)
                    .map_err(|e| crate::types::KernelError::storage(e.to_string()))?,
            );
        }

        // A replacement `SendMessage` action without a session gets a
        // dedicated new session bound, same as on create.
        let mut bound_session: Option<crate::types::SessionId> = None;
        if let Some(action) = input.action.take() {
            // Bail out on unknown ids before binding any session, and reuse
            // the job's name for the new session title.
            let Some(existing) = store.get(id).await.map_err(|e| {
                crate::types::KernelError::storage(format!("Failed to get cron job: {e}"))
            })?
            else {
                return Ok(false);
            };
            let session_store = self.session_store().await;
            let binds_new_session = matches!(
                action,
                crate::cron::CronAction::SendMessage {
                    session_id: None,
                    ..
                }
            );
            let action = crate::cron::ensure_action_session(
                action,
                &existing.name,
                &session_store,
                None,
                self.agent_shared.config_auto_approve,
            )
            .await?;
            if binds_new_session {
                bound_session = crate::cron::action_session_id(&action);
            }
            input.action = Some(action);
        }

        let updated = match store.update(id, &input).await {
            Ok(updated) => updated,
            Err(e) => {
                crate::cron::rollback_bound_session(&self.session_store().await, bound_session)
                    .await;
                return Err(crate::types::KernelError::storage(format!(
                    "Failed to update cron job: {e}"
                )));
            }
        };
        if !updated {
            // The job vanished between the get above and this update — the
            // freshly bound session would orphan; roll it back.
            crate::cron::rollback_bound_session(&self.session_store().await, bound_session).await;
            return Ok(false);
        }
        self.notify_cron_scheduler();
        Ok(true)
    }

    /// Delete a cron job.  Returns `true` if the job existed.
    pub async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;
        let deleted = store.delete(id).await.map_err(|e| {
            crate::types::KernelError::storage(format!("Failed to delete cron job: {e}"))
        })?;
        if deleted {
            self.notify_cron_scheduler();
        }
        Ok(deleted)
    }

    /// Trigger a cron job manually (execute immediately). Manual triggers
    /// are not recorded: they don't consume `run_count`/`max_runs` and
    /// don't touch `last_run_at`/`last_error`.
    pub async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;

        let job = store
            .get(id)
            .await
            .map_err(|e| {
                crate::types::KernelError::storage(format!("Failed to get cron job: {e}"))
            })?
            .ok_or_else(|| crate::types::KernelError::storage("Cron job not found"))?;

        let result = match tokio::time::timeout(
            std::time::Duration::from_secs(crate::cron::worker::EXECUTION_TIMEOUT_SECS),
            crate::cron::CronExecutor::execute_cron_action(self, &job.action),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(crate::cron::CronError::Timeout(
                crate::cron::worker::EXECUTION_TIMEOUT_SECS,
            )),
        };

        // 手动触发只跑一遍：SelfComplete（exit 42）约定只由调度路径兑现，
        // 这里丢弃 outcome、只透传成败。
        result
            .map(|_| ())
            .map_err(|e| crate::types::KernelError::storage(e.to_string()))
    }
}

#[async_trait::async_trait]
impl crate::cron::CronExecutor for Kernel {
    async fn execute_cron_action(
        &self,
        action: &crate::cron::CronAction,
    ) -> std::result::Result<crate::cron::CronActionOutcome, crate::cron::CronError> {
        use crate::cron::types::{render_template, CronAction, CronError};
        use crate::cron::CronActionOutcome;
        use crate::types::{ContentBlock, SessionId};

        match action {
            CronAction::SendMessage {
                session_id,
                content,
            } => {
                let session_id = session_id.as_deref().ok_or_else(|| {
                    CronError::Session(crate::types::KernelError::storage(
                        "cron job has no session bound",
                    ))
                })?;
                let sid = SessionId::from(session_id.to_string());
                let text = render_template(content);
                let blocks = vec![ContentBlock::Text { text }];
                self.send_message_inner(&sid, blocks, false)
                    .await
                    .map_err(CronError::Session)?;
                Ok(CronActionOutcome::Done)
            }
            CronAction::Shell {
                command,
                working_dir,
            } => {
                let out = crate::cron::run_shell_command(command, working_dir.as_deref()).await?;
                Ok(if out.self_complete {
                    CronActionOutcome::SelfComplete
                } else {
                    CronActionOutcome::Done
                })
            }
            CronAction::Internal { .. } => {
                Err(CronError::UnsupportedAction("Internal".to_string()))
            }
        }
    }
}

fn mark_user_steer(
    mut content: Vec<crate::types::ContentBlock>,
) -> Vec<crate::types::ContentBlock> {
    const PREFIX: &str = "[From User] ";

    if let Some(crate::types::ContentBlock::Text { text }) = content.first_mut() {
        text.insert_str(0, PREFIX);
    } else {
        content.insert(
            0,
            crate::types::ContentBlock::Text {
                text: PREFIX.to_string(),
            },
        );
    }

    content
}

fn agent_state_phase(state: AgentState) -> &'static str {
    match state {
        AgentState::Streaming => "streaming",
        AgentState::ExecutingTool => "executing_tool",
        AgentState::Compacting => "compacting",
        AgentState::Idle => "idle",
    }
}

fn session_title_lock_key(session_id: &SessionId) -> String {
    format!("session-title:{}", session_id.0)
}

/// Normalize session title: collapse whitespace, trim, truncate to 20 chars.
fn normalize_session_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::utils::strs::truncate_by_chars(&title, 20, "")
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;

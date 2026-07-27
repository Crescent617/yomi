//! Cron tool for managing scheduled jobs.
//!
//! A single tool with multiple actions (list/create/update/delete/trigger),
//! gated behind the `cron` tool flag (`[features] cron_tool`, default off).

use crate::agent::AgentInput;
use crate::comms::InputBus;
use crate::cron::{CronAction, CronJobId, CronJobStatus, CronScheduler, CronStore};
use crate::storage::SessionStore;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{ContentBlock, KernelError, Result, SessionId, ToolOutput};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub const CRON_TOOL_NAME: &str = "cron";

/// Same execution timeout as `CronWorker`.
const TRIGGER_TIMEOUT_SECS: u64 = crate::cron::worker::EXECUTION_TIMEOUT_SECS;

/// Max captured stdout returned by `trigger` for shell jobs.
const MAX_STDOUT: usize = 4096;

/// Unified cron job management tool.
pub struct CronTool {
    store: Arc<dyn CronStore>,
    /// Shared slot with the running scheduler (filled by the daemon). Mutations
    /// notify it so new/updated/deleted jobs are picked up immediately.
    scheduler: Arc<std::sync::Mutex<Option<Arc<CronScheduler>>>>,
    /// Needed to bind a dedicated session when `send_message` omits `session_id`.
    session_store: Option<Arc<dyn SessionStore>>,
    /// Needed to deliver `trigger` messages.
    input_bus: Option<Arc<InputBus>>,
}

impl CronTool {
    pub fn new(
        store: Arc<dyn CronStore>,
        scheduler: Arc<std::sync::Mutex<Option<Arc<CronScheduler>>>>,
        session_store: Option<Arc<dyn SessionStore>>,
        input_bus: Option<Arc<InputBus>>,
    ) -> Self {
        Self {
            store,
            scheduler,
            session_store,
            input_bus,
        }
    }

    /// Notify the running scheduler that jobs changed (no-op outside daemon mode).
    fn notify_scheduler(&self) {
        let slot = self.scheduler.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref scheduler) = *slot {
            scheduler.reload();
        }
    }

    async fn handle_list(&self, args: &Value) -> Result<ToolOutput> {
        let status = match args["status"].as_str() {
            Some(s) => Some(
                s.parse::<CronJobStatus>()
                    .map_err(|e: String| KernelError::tool(e))?,
            ),
            None => None,
        };
        let limit = args["limit"].as_u64().unwrap_or(50).clamp(1, 200) as usize;

        let jobs = self
            .store
            .list(status, limit)
            .await
            .map_err(|e| KernelError::tool(format!("failed to list cron jobs: {e}")))?;
        let out = serde_json::to_string(&jobs)?;
        Ok(ToolOutput::text(out))
    }

    async fn handle_create(&self, args: &Value, ctx: &ToolExecCtx<'_>) -> Result<ToolOutput> {
        let name = required_str(args, "name")?.to_string();
        let schedule = required_str(args, "schedule")?.to_string();
        let job_type = required_str(args, "type")?;

        let action = match job_type {
            "send_message" => CronAction::SendMessage {
                session_id: optional_str(args, "session_id"),
                content: required_str(args, "content")?.to_string(),
            },
            "shell" => CronAction::Shell {
                command: required_str(args, "command")?.to_string(),
                working_dir: optional_str(args, "working_dir"),
            },
            other => {
                return Err(KernelError::tool(format!(
                    "unknown type '{other}', expected send_message or shell"
                )))
            }
        };

        let input = crate::cron::CreateCronJobInput {
            name,
            schedule,
            action,
            max_runs: parse_max_runs(args)?,
            expires_at: parse_expires_at(args)?,
        };

        // When a new session needs to be bound, it follows the current
        // session's working_dir/project; model stays default.
        let follow = match &self.session_store {
            Some(store) => match store.get(&SessionId::from(ctx.session_id.clone())).await {
                Ok(info) => info,
                Err(e) => {
                    tracing::warn!(
                        "cron: failed to load session {} for follow: {e}",
                        ctx.session_id
                    );
                    None
                }
            },
            None => None,
        };

        let job = crate::cron::create_cron_job(
            &self.store,
            self.session_store.as_ref(),
            follow.as_ref(),
            input,
        )
        .await
        .map_err(|e| KernelError::tool(e.to_string()))?;
        self.notify_scheduler();

        let session_id = match &job.action {
            CronAction::SendMessage { session_id, .. } => session_id.clone(),
            _ => None,
        };
        Ok(ToolOutput::text(
            json!({
                "job_id": job.id.0,
                "session_id": session_id,
                "next_run_at": job.next_run_at.map(|t| t.to_rfc3339()),
            })
            .to_string(),
        ))
    }

    async fn handle_update(&self, args: &Value) -> Result<ToolOutput> {
        let id = parse_job_id(args)?;

        let mut input = crate::cron::UpdateCronJobInput::default();
        if let Some(name) = optional_str(args, "name") {
            input.name = Some(name);
        }
        if let Some(schedule_str) = optional_str(args, "schedule") {
            input.next_run_at = Some(
                crate::cron::next_run_from_schedule(&schedule_str)
                    .map_err(|e| KernelError::tool(e.to_string()))?,
            );
            input.schedule = Some(schedule_str);
        }
        if let Some(status) = args["status"].as_str() {
            input.status = Some(match status {
                "active" => CronJobStatus::Active,
                "paused" => CronJobStatus::Paused,
                _ => {
                    return Err(KernelError::tool(
                        "status for update must be active or paused".to_string(),
                    ))
                }
            });
        }
        if let Some(v) = args.get("max_runs") {
            if v.is_null() {
                input.max_runs = Some(crate::cron::UNLIMITED_MAX_RUNS);
            } else {
                input.max_runs = Some(parse_max_runs_value(v)?);
            }
        }
        if let Some(v) = args.get("expires_at") {
            if v.is_null() {
                input.expires_at = Some(crate::cron::NEVER_EXPIRES);
            } else {
                input.expires_at = Some(parse_expires_at(args)?.ok_or_else(|| {
                    KernelError::tool("expires_at must be an RFC3339 timestamp".to_string())
                })?);
            }
        }

        // Action edits: rebuild the existing action, preserving its type.
        let wants_action_edit = args.get("content").is_some()
            || args.get("command").is_some()
            || args.get("working_dir").is_some()
            || args.get("session_id").is_some();
        let mut bound_session: Option<SessionId> = None;
        if wants_action_edit {
            let job = self
                .store
                .get(&id)
                .await
                .map_err(|e| KernelError::tool(format!("failed to get cron job: {e}")))?
                .ok_or_else(|| KernelError::tool(format!("cron job '{}' not found", id.0)))?;
            // 与 job 类型不符的字段直接报错，避免静默丢弃
            let mismatched = match &job.action {
                CronAction::SendMessage { .. } => ["command", "working_dir"]
                    .iter()
                    .find(|k| args.get(**k).is_some()),
                CronAction::Shell { .. } => ["content", "session_id"]
                    .iter()
                    .find(|k| args.get(**k).is_some()),
                CronAction::Internal { .. } => None,
            };
            if let Some(key) = mismatched {
                return Err(KernelError::tool(format!(
                    "{key} does not apply to this job type"
                )));
            }
            let mut action = match job.action {
                CronAction::SendMessage {
                    session_id,
                    content,
                } => CronAction::SendMessage {
                    session_id: optional_str(args, "session_id").or(session_id),
                    content: args["content"].as_str().map_or(content, str::to_string),
                },
                CronAction::Shell {
                    command,
                    working_dir,
                } => CronAction::Shell {
                    command: args["command"].as_str().map_or(command, str::to_string),
                    working_dir: optional_str(args, "working_dir").or(working_dir),
                },
                other @ CronAction::Internal { .. } => other,
            };
            // A SendMessage left without a session would fail every fire
            // ("cron job has no session bound"); bind a dedicated session
            // now, same as the create path and the Kernel RPC update path.
            if matches!(
                action,
                CronAction::SendMessage {
                    session_id: None,
                    ..
                }
            ) {
                let Some(session_store) = &self.session_store else {
                    return Err(KernelError::tool(
                        "session store not available; pass session_id explicitly".to_string(),
                    ));
                };
                action = crate::cron::ensure_action_session(action, &job.name, session_store, None)
                    .await
                    .map_err(|e| KernelError::tool(e.to_string()))?;
                bound_session = crate::cron::action_session_id(&action);
            }
            input.action = Some(action);
        }

        let updated = match self.store.update(&id, &input).await {
            Ok(updated) => updated,
            Err(e) => {
                self.rollback_bound_session(bound_session).await;
                return Err(KernelError::tool(format!("failed to update cron job: {e}")));
            }
        };
        if !updated {
            // The job vanished between the get above and this update — the
            // freshly bound session would orphan; roll it back.
            self.rollback_bound_session(bound_session).await;
            return Err(KernelError::tool(format!("cron job '{}' not found", id.0)));
        }
        self.notify_scheduler();

        Ok(ToolOutput::text(json!({ "updated": true }).to_string()))
    }

    /// Best-effort rollback of a dedicated session bound during a failed
    /// update (the update erroring out, or the job having vanished).
    async fn rollback_bound_session(&self, session: Option<SessionId>) {
        if let Some(store) = &self.session_store {
            crate::cron::rollback_bound_session(store, session).await;
        }
    }

    async fn handle_delete(&self, args: &Value) -> Result<ToolOutput> {
        let id = parse_job_id(args)?;
        let deleted = self
            .store
            .delete(&id)
            .await
            .map_err(|e| KernelError::tool(format!("failed to delete cron job: {e}")))?;
        if deleted {
            self.notify_scheduler();
        }
        Ok(ToolOutput::text(json!({ "deleted": deleted }).to_string()))
    }

    async fn handle_trigger(&self, args: &Value) -> Result<ToolOutput> {
        let id = parse_job_id(args)?;
        let job = self
            .store
            .get(&id)
            .await
            .map_err(|e| KernelError::tool(format!("failed to get cron job: {e}")))?
            .ok_or_else(|| KernelError::tool(format!("cron job '{}' not found", id.0)))?;

        let result = self.execute_action(&job.action).await;

        // Manual triggers are not recorded: they don't consume
        // `run_count`/`max_runs` and don't touch `last_run_at`/`last_error`.
        match result {
            Ok(stdout) => {
                let mut out = json!({ "triggered": true });
                if !stdout.is_empty() {
                    out["stdout"] = json!(stdout);
                }
                Ok(ToolOutput::text(out.to_string()))
            }
            Err(e) => Err(KernelError::tool(format!("trigger failed: {e}"))),
        }
    }

    /// Execute a job action once (for `trigger`). Returns captured stdout.
    async fn execute_action(&self, action: &CronAction) -> Result<String> {
        match action {
            CronAction::SendMessage {
                session_id,
                content,
            } => {
                let session_id = session_id.as_deref().ok_or_else(|| {
                    KernelError::tool("cron job has no session bound".to_string())
                })?;
                let input_bus = self.input_bus.as_ref().ok_or_else(|| {
                    KernelError::tool("trigger is not available in this mode".to_string())
                })?;
                let text = crate::cron::types::render_template(content);
                input_bus
                    .publish(
                        SessionId::from(session_id.to_string()),
                        AgentInput::User {
                            content: vec![ContentBlock::Text { text }],
                        },
                    )
                    .map_err(|e| KernelError::tool(format!("failed to deliver message: {e}")))?;
                Ok(String::new())
            }
            CronAction::Shell {
                command,
                working_dir,
            } => execute_shell(command, working_dir.as_deref()).await,
            CronAction::Internal { endpoint, .. } => Err(KernelError::tool(format!(
                "unsupported action type (internal: {endpoint})"
            ))),
        }
    }
}

/// Run a shell command via the shared cron runner (same hardening as the
/// worker), with the worker's execution timeout. Returns captured stdout
/// (truncated to 4KB).
async fn execute_shell(command: &str, working_dir: Option<&str>) -> Result<String> {
    let stdout = tokio::time::timeout(
        Duration::from_secs(TRIGGER_TIMEOUT_SECS),
        crate::cron::run_shell_command(command, working_dir),
    )
    .await
    .map_err(|_| {
        KernelError::tool(format!(
            "shell command timed out after {TRIGGER_TIMEOUT_SECS} seconds"
        ))
    })?
    .map_err(|e| KernelError::tool(e.to_string()))?;

    let stdout = stdout.trim();
    Ok(crate::utils::strs::truncate_with_suffix(
        stdout,
        MAX_STDOUT,
        "... [truncated]",
    ))
}

/// Non-empty string arg, or a "required" tool error.
fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| KernelError::tool(format!("{key} is required")))
}

/// Non-empty string arg, or `None` when absent/blank.
fn optional_str(args: &Value, key: &str) -> Option<String> {
    args[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

fn parse_job_id(args: &Value) -> Result<CronJobId> {
    Ok(CronJobId::from(required_str(args, "id")?.to_string()))
}

fn parse_max_runs(args: &Value) -> Result<Option<u32>> {
    match args.get("max_runs") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(parse_max_runs_value(v)?)),
    }
}

fn parse_max_runs_value(v: &Value) -> Result<u32> {
    // 0 is the "no limit" sentinel.
    let n = v
        .as_u64()
        .filter(|n| u32::try_from(*n).is_ok())
        .ok_or_else(|| KernelError::tool("max_runs must be a non-negative integer".to_string()))?;
    Ok(n as u32)
}

fn parse_expires_at(args: &Value) -> Result<Option<DateTime<Utc>>> {
    match args.get("expires_at").and_then(Value::as_str) {
        None => Ok(None),
        Some(s) => {
            let dt = DateTime::parse_from_rfc3339(s)
                .map_err(|e| KernelError::tool(format!("invalid expires_at (RFC3339): {e}")))?;
            Ok(Some(dt.with_timezone(&Utc)))
        }
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        CRON_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        r"Manage cron jobs: scheduled tasks that send a message to a session (waking its agent) or run a shell command on a cron schedule.
Actions: list, create, update, delete, trigger (run once immediately, for testing).
Schedule is a cron expression with 5 fields ('0 9 * * 1-5' = weekdays 09:00) or 6 fields with leading seconds, interpreted in the machine's LOCAL timezone.
For send_message jobs: pass session_id to target an existing session (e.g. the current conversation); omit it to create a dedicated new session that every run reuses (the new session inherits the current working directory and project; model stays default).
Use update with status active/paused to resume/pause a job; pass null (or 0 for max_runs, the zero timestamp for expires_at) to clear those limits. Job type cannot be changed after creation."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "create", "update", "delete", "trigger"],
                    "description": "list: show jobs; create: add a job; update: partial update (pause/resume via status); delete: remove; trigger: run once immediately"
                },
                "id": {
                    "type": "string",
                    "description": "Job id. Required for update/delete/trigger"
                },
                "status": {
                    "type": "string",
                    "enum": ["active", "paused", "completed", "failed"],
                    "description": "For list: filter by status. For update: only active/paused are allowed"
                },
                "limit": {
                    "type": "integer",
                    "description": "For list: max jobs to return (default 50)"
                },
                "name": {
                    "type": "string",
                    "description": "Job name. Required for create"
                },
                "schedule": {
                    "type": "string",
                    "description": "Cron expression in local timezone, e.g. '0 9 * * 1-5' (weekdays 09:00) or 6-field '0 0 9 * * 1-5'. Required for create"
                },
                "type": {
                    "type": "string",
                    "enum": ["send_message", "shell"],
                    "description": "Job type. Required for create; cannot be changed later"
                },
                "content": {
                    "type": "string",
                    "description": "Message text for send_message. Supports {{timestamp}}, {{date}}, {{time}} template variables. Required for create with type=send_message"
                },
                "session_id": {
                    "type": "string",
                    "description": "Target session for send_message. Omit on create to create a dedicated new session (following the current working directory and project) reused by every run; pass the current session id to deliver messages to this conversation"
                },
                "command": {
                    "type": "string",
                    "description": "Shell command for type=shell. Required for create with type=shell"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for type=shell (default: current dir)"
                },
                "max_runs": {
                    "type": "integer",
                    "description": "Stop after N runs (0 or omitted: unlimited; pass null or 0 on update to clear a limit)"
                },
                "expires_at": {
                    "type": "string",
                    "description": "RFC3339 expiry timestamp (default: never expires; pass null or the zero timestamp on update to clear)"
                }
            }
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| KernelError::tool("action is required"))?;

        match action {
            "list" => self.handle_list(&args).await,
            "create" => self.handle_create(&args, &ctx).await,
            "update" => self.handle_update(&args).await,
            "delete" => self.handle_delete(&args).await,
            "trigger" => self.handle_trigger(&args).await,
            _ => Err(KernelError::tool(format!("unknown action: {action}"))),
        }
    }
}

#[cfg(test)]
#[path = "cron_test.rs"]
mod tests;

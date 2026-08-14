//! Session management for CLI

use crate::{storage::AppStorage, utils::DEBUG_MODE};
use anyhow::Result;
use kernel::{
    client::KernelApi,
    event::Command,
    kernel::CreateSessionInput,
    permission::Level,
    tools::AskUserResponse,
    types::{ContentBlock, SessionId},
};
use std::path::Path;
use std::sync::Arc;
use tui::run_tui;

/// Send a message to the daemon with automatic retry and session restore.
/// On `session_not_found` (daemon restart), attempts `restore_session` once.
/// Exponential backoff capped at 2s. Returns `Err` after `max_retries` failures.
pub(crate) async fn send_with_retry(
    kernel: &dyn KernelApi,
    session_id: &SessionId,
    blocks: Vec<ContentBlock>,
    max_retries: u32,
) -> Result<()> {
    let mut retries = 0;
    let mut restored = false;
    loop {
        match kernel.send_message(session_id, blocks.clone()).await {
            Ok(()) => return Ok(()),
            Err(ref e) if !restored && e.is_session_not_found() => {
                tracing::info!(
                    "Session {} missing on daemon, attempting restore...",
                    session_id.0
                );
                match kernel.restore_session(session_id).await {
                    Ok(_) => {
                        tracing::info!("Session restored successfully");
                        restored = true;
                    }
                    Err(restore_err) => {
                        return Err(anyhow::anyhow!(
                            "Failed to restore session {}: {}",
                            session_id.0,
                            restore_err
                        ));
                    }
                }
            }
            Err(e) => {
                retries += 1;
                if retries > max_retries {
                    return Err(anyhow::anyhow!(
                        "send_message failed {} times for session {}: {}",
                        max_retries,
                        session_id.0,
                        e
                    ));
                }
                tracing::warn!("send_message failed (retry {}): {}", retries, e);
                let delay = std::cmp::min(100 * (1_u64 << retries), 2000);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
        }
    }
}

/// Context needed to run a session
#[derive(Clone)]
pub struct SessionContext {
    pub working_dir: std::path::PathBuf,
}

/// Result of running a session
pub struct SessionResult {
    pub new_history_entries: Vec<String>,
    pub should_create_new_session: bool,
    pub switch_to_session: Option<String>,
}

/// Session argument parsed from command line
#[derive(Debug, Clone)]
pub enum SessionArg {
    /// No --session flag, create new session
    New,
    /// --session without value, resume last session
    Last,
    /// --session <id>, resume specific session
    Specific(String),
    /// --fork without value, fork last session
    ForkLast,
    /// --fork <id>, fork specific session
    ForkSpecific(String),
}

/// Resolve session from command line arguments.
/// `auto_approve_level` is passed directly to the kernel,
/// which holds the agent configuration internally.
pub async fn resolve_session(
    session_arg: &SessionArg,
    is_launch: bool,
    kernel: &dyn KernelApi,
    app_storage: &AppStorage,
    working_dir: &Path,
    auto_approve_level: Level,
    model_key: Option<String>,
) -> Result<SessionId> {
    // All branches create at most one session, so this FnOnce closure can
    // move `model_key` into the input without cloning.
    let new_session_input = move || CreateSessionInput {
        project_id: None,
        working_dir: Some(working_dir.to_path_buf()),
        auto_approve_level: Some(auto_approve_level),
        tool_blocklist: vec![],
        model_key,
    };

    // When not launching (e.g., creating new session mid-run), ignore --resume/--fork args
    if !is_launch {
        return Ok(kernel.create_session(new_session_input()).await?);
    }

    match session_arg {
        // --session <id>: restore specific session
        SessionArg::Specific(id) => {
            let session_id = SessionId::from(id.clone());
            eprintln!("Restoring session: {}", session_id.0);

            match kernel.restore_session(&session_id).await {
                Ok(_) => Ok(session_id),
                Err(e) => {
                    eprintln!("Failed to restore session: {e}");
                    eprintln!("Starting new session instead");
                    Ok(kernel.create_session(new_session_input()).await?)
                }
            }
        }
        // --session (no value): resume last session for this directory
        SessionArg::Last => match app_storage.load_session(working_dir).await? {
            Some(entry) => {
                let session_id = SessionId::from(entry.session_id);
                eprintln!("Restoring previous session: {}", session_id.0);

                match kernel.restore_session(&session_id).await {
                    Ok(_) => Ok(session_id),
                    Err(e) => {
                        eprintln!("Failed to restore session: {e}");
                        eprintln!("Starting new session instead");
                        Ok(kernel.create_session(new_session_input()).await?)
                    }
                }
            }
            None => {
                eprintln!("No previous session found, starting new session");
                Ok(kernel.create_session(new_session_input()).await?)
            }
        },
        // No --session: create new session
        SessionArg::New => Ok(kernel.create_session(new_session_input()).await?),
        // --fork (no value): fork last session for this directory
        SessionArg::ForkLast => match app_storage.load_session(working_dir).await? {
            Some(entry) => {
                let source_id = SessionId::from(entry.session_id);
                eprintln!("Forking last session: {}", source_id.0);
                Ok(kernel.fork_session(&source_id, auto_approve_level).await?)
            }
            None => {
                eprintln!("No previous session found to fork, starting new session");
                Ok(kernel.create_session(new_session_input()).await?)
            }
        },
        // --fork <id>: fork specific session
        SessionArg::ForkSpecific(id) => {
            let source_id = SessionId::from(id.clone());
            eprintln!("Forking session: {}", source_id.0);
            Ok(kernel.fork_session(&source_id, auto_approve_level).await?)
        }
    }
}

/// Run a single session lifecycle
pub async fn run_session_loop(
    kernel: Arc<dyn KernelApi>,
    session_id: SessionId,
    ctx: SessionContext,
    app_storage: Arc<AppStorage>,
    input_history: Vec<String>,
    is_launch: bool,
    initial_message: Option<String>,
) -> Result<SessionResult> {
    const MAX_RETRIES: u32 = 10;

    // Print startup info only in debug mode (DEBUG=1)
    if *DEBUG_MODE {
        if is_launch {
            println!("yomi session started: {}", session_id.0);
            println!("Working directory: {}", ctx.working_dir.display());
        } else {
            println!("yomi new session started: {}", session_id.0);
        }
        println!("Starting TUI...\n");
    }

    // Create channels
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<Vec<ContentBlock>>(100);
    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::channel::<Command>(10);

    // Spawn input forwarding task.
    // Keeps retrying transient errors (connection lost, daemon restarting).
    // On `session_not_found` (daemon was restarted and lost in-memory state)
    // we automatically call `restore_session` so the TUI can continue
    // seamlessly.
    let coord_for_input = kernel.clone();
    let session_id_for_input = session_id.clone();
    let app_storage_for_save = app_storage.clone();
    let working_dir_for_save = ctx.working_dir.clone();
    let mut input_handle = tokio::spawn(async move {
        let mut has_saved = false;
        while let Some(blocks) = input_rx.recv().await {
            if !has_saved {
                app_storage_for_save
                    .update_last_session(&working_dir_for_save, &session_id_for_input.0)
                    .await
                    .ok();
                has_saved = true;
            }
            if let Err(e) = send_with_retry(
                &*coord_for_input,
                &session_id_for_input,
                blocks,
                MAX_RETRIES,
            )
            .await
            {
                tracing::error!(
                    "Input forwarding stopped for session {}: {}",
                    session_id_for_input.0,
                    e
                );
                return;
            }
        }
    });

    // Spawn control command handling task
    let coord_for_ctrl = kernel.clone();
    let session_id_for_ctrl = session_id.clone();
    let mut ctrl_handle = tokio::spawn(async move {
        while let Some(cmd) = ctrl_rx.recv().await {
            match cmd {
                Command::Cancel => {
                    if let Err(e) = coord_for_ctrl.cancel(&session_id_for_ctrl).await {
                        tracing::error!("Failed to cancel request: {}", e);
                    }
                }
                Command::Response {
                    req_id,
                    approved,
                    remember,
                } => {
                    if let Err(e) = coord_for_ctrl
                        .send_permission_response(&session_id_for_ctrl, &req_id, approved, remember)
                        .await
                    {
                        tracing::error!("Failed to send permission response: {}", e);
                    }
                }
                Command::SetLevel(level) => {
                    if let Err(e) = coord_for_ctrl
                        .set_permission_level(&session_id_for_ctrl, level)
                        .await
                    {
                        tracing::error!("Failed to set permission level: {}", e);
                    }
                }
                Command::Compact => {
                    if let Err(e) = coord_for_ctrl.compact_session(&session_id_for_ctrl).await {
                        tracing::error!("Failed to compact session: {}", e);
                    }
                }
                Command::StartGoal(config) => {
                    if let Err(e) = coord_for_ctrl
                        .start_goal(&session_id_for_ctrl, config)
                        .await
                    {
                        tracing::error!("Failed to start goal: {}", e);
                    }
                }
                Command::StopGoal => {
                    if let Err(e) = coord_for_ctrl.stop_goal(&session_id_for_ctrl).await {
                        tracing::error!("Failed to stop goal: {}", e);
                    }
                }
                Command::Rewind { message_id, target } => {
                    if let Err(e) = coord_for_ctrl
                        .rewind_session(&session_id_for_ctrl, message_id, target)
                        .await
                    {
                        tracing::error!("Failed to rewind session: {}", e);
                    }
                }
                Command::AskUserResponse { req_id, answers } => {
                    let response = AskUserResponse {
                        answers: answers.into_iter().collect(),
                    };
                    if let Err(e) = coord_for_ctrl
                        .send_ask_user_response(&session_id_for_ctrl, &req_id, response)
                        .await
                    {
                        tracing::error!("Failed to send ask_user response: {}", e);
                    }
                }
                Command::PauseGoal => {
                    if let Err(e) = coord_for_ctrl.pause_goal(&session_id_for_ctrl).await {
                        tracing::error!("Failed to pause goal: {}", e);
                    }
                }
                Command::ResumeGoal => {
                    if let Err(e) = coord_for_ctrl.resume_goal(&session_id_for_ctrl).await {
                        tracing::error!("Failed to resume goal: {}", e);
                    }
                }
                Command::EditGoal { description } => {
                    if let Err(e) = coord_for_ctrl
                        .update_goal(&session_id_for_ctrl, description)
                        .await
                    {
                        tracing::error!("Failed to edit goal: {}", e);
                    }
                }
                Command::Steer { content } => {
                    if let Err(e) = coord_for_ctrl
                        .send_steer(&session_id_for_ctrl, content)
                        .await
                    {
                        tracing::error!("Failed to send steer: {}", e);
                    }
                }
                Command::Continue => {
                    if let Err(e) = coord_for_ctrl.send_continue(&session_id_for_ctrl).await {
                        tracing::error!("Failed to send continue: {}", e);
                    }
                }
                Command::GetGoal => {
                    // GetGoal is a query; no-op for CLI since it returns a value
                    tracing::debug!("GetGoal command received in CLI session — no action");
                }
            }
        }
    });

    // Subscribe to session events (broadcast channel - TUI can lag but won't block)
    let event_rx = kernel.subscribe_session_events(&session_id, None).await?;

    let tui_result = run_tui(
        event_rx,
        input_tx.clone(),
        ctrl_tx,
        kernel.clone(),
        ctx.working_dir.to_string_lossy().to_string(),
        input_history,
        initial_message,
        session_id.0.to_string(),
    )
    .await?;

    // Close input channel so the forwarding task exits
    drop(input_tx);
    // When the daemon connection is already known to be dead, draining
    // tasks and RPCs below would only stall until their timeouts — use a
    // short drain window and skip the remote session check entirely.
    let kernel_reachable = kernel.is_connected().await;
    let drain_timeout = if kernel_reachable {
        std::time::Duration::from_secs(5)
    } else {
        std::time::Duration::from_millis(500)
    };

    // Wait for the input/control tasks to drain. In daemon mode their
    // kernel calls are async over IPC, so allow up to 5s; on timeout abort
    // them rather than letting them keep retrying against a dead daemon
    // in the background after shutdown.
    let (input_drained, ctrl_drained) = tokio::join!(
        tokio::time::timeout(drain_timeout, &mut input_handle),
        tokio::time::timeout(drain_timeout, &mut ctrl_handle),
    );
    if input_drained.is_err() {
        tracing::warn!("Input forwarding task did not exit in time, aborting");
        input_handle.abort();
    }
    if ctrl_drained.is_err() {
        tracing::warn!("Control task did not exit in time, aborting");
        ctrl_handle.abort();
    }

    // Only record session if the session has actual messages in storage.
    // If we can't reach storage (e.g. daemon disconnected), conservatively
    // keep the session rather than risk deleting data.
    let messages = if kernel_reachable {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            kernel.list_messages(&session_id),
        )
        .await
        {
            Ok(result) => Some(result),
            Err(_) => {
                tracing::warn!("list_messages timed out during shutdown, keeping session");
                None
            }
        }
    } else {
        tracing::info!("Daemon connection is down, keeping session without remote check");
        None
    };
    match messages {
        Some(Ok(msgs)) if msgs.is_empty() => {
            if let Err(e) = kernel.delete_session(&session_id).await {
                tracing::warn!("Failed to delete empty session: {}", e);
            }
            println!("Goodbye~");
        }
        Some(Ok(_)) => {
            app_storage
                .save_session(&ctx.working_dir, &session_id.0)
                .await?;
            println!("Goodbye~ You can resume this session later with:");
            println!("yomi --resume {}", session_id.0);
        }
        other => {
            if let Some(Err(e)) = &other {
                tracing::warn!("Failed to check session messages, keeping session: {}", e);
            }
            app_storage
                .save_session(&ctx.working_dir, &session_id.0)
                .await?;
            println!("Goodbye~ You can resume this session later with:");
            println!("yomi --resume {}", session_id.0);
        }
    }

    Ok(SessionResult {
        new_history_entries: tui_result.input_history,
        should_create_new_session: tui_result.should_create_new_session,
        switch_to_session: tui_result.switch_to_session,
    })
}

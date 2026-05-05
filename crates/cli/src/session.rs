//! Session management for CLI

use crate::{storage::AppStorage, utils::DEBUG_MODE};
use anyhow::Result;
use kernel::{
    event::ControlCommand,
    types::{ContentBlock, SessionId},
    Coordinator, SessionConfig,
};
use std::path::Path;
use std::sync::Arc;
use tui::{run_tui, OnInputHook};

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

/// Resolve session from command line arguments
pub async fn resolve_session(
    session_arg: &SessionArg,
    is_launch: bool,
    coordinator: &Coordinator,
    app_storage: &AppStorage,
    working_dir: &Path,
    mk_config: impl Fn() -> SessionConfig,
) -> Result<SessionId> {
    // When not launching (e.g., creating new session mid-run), ignore --resume/--fork args
    if !is_launch {
        return Ok(coordinator.create_session(mk_config()).await?);
    }

    match session_arg {
        // --session <id>: restore specific session
        SessionArg::Specific(id) => {
            let session_id = SessionId(id.clone());
            println!("Restoring session: {}", session_id.0);

            match coordinator.restore_session(&session_id, mk_config()).await {
                Ok(_) => Ok(session_id),
                Err(e) => {
                    println!("Failed to restore session: {e}");
                    println!("Starting new session instead");
                    Ok(coordinator.create_session(mk_config()).await?)
                }
            }
        }
        // --session (no value): resume last session for this directory
        SessionArg::Last => match app_storage.load_session(working_dir).await? {
            Some(entry) => {
                let session_id = SessionId(entry.session_id);
                println!("Restoring previous session: {}", session_id.0);

                match coordinator.restore_session(&session_id, mk_config()).await {
                    Ok(_) => Ok(session_id),
                    Err(e) => {
                        println!("Failed to restore session: {e}");
                        println!("Starting new session instead");
                        Ok(coordinator.create_session(mk_config()).await?)
                    }
                }
            }
            None => {
                println!("No previous session found, starting new session");
                Ok(coordinator.create_session(mk_config()).await?)
            }
        },
        // No --session: create new session
        SessionArg::New => Ok(coordinator.create_session(mk_config()).await?),
        // --fork (no value): fork last session for this directory
        SessionArg::ForkLast => match app_storage.load_session(working_dir).await? {
            Some(entry) => {
                let source_id = SessionId(entry.session_id);
                println!("Forking last session: {}", source_id.0);
                Ok(coordinator.fork_session(&source_id, mk_config()).await?)
            }
            None => {
                println!("No previous session found to fork, starting new session");
                Ok(coordinator.create_session(mk_config()).await?)
            }
        },
        // --fork <id>: fork specific session
        SessionArg::ForkSpecific(id) => {
            let source_id = SessionId(id.clone());
            println!("Forking session: {}", source_id.0);
            Ok(coordinator.fork_session(&source_id, mk_config()).await?)
        }
    }
}

/// Run a single session lifecycle
#[allow(clippy::too_many_arguments)]
pub async fn run_session_loop(
    coordinator: Arc<Coordinator>,
    session_id: SessionId,
    ctx: SessionContext,
    app_storage: Arc<AppStorage>,
    input_history: Vec<String>,
    session_messages: Vec<kernel::types::Message>,
    is_launch: bool,
    initial_message: Option<String>,
) -> Result<SessionResult> {
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
    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::channel::<ControlCommand>(10);

    // Spawn input forwarding task
    let coord_for_input = coordinator.clone();
    let session_id_for_input = session_id.clone();
    tokio::spawn(async move {
        while let Some(blocks) = input_rx.recv().await {
            if let Err(e) = coord_for_input
                .send_message(&session_id_for_input, blocks)
                .await
            {
                tracing::error!("Failed to send message: {}", e);
            }
        }
    });

    // Spawn control command handling task
    let coord_for_ctrl = coordinator.clone();
    let session_id_for_ctrl = session_id.clone();
    tokio::spawn(async move {
        while let Some(cmd) = ctrl_rx.recv().await {
            match cmd {
                ControlCommand::Cancel => {
                    if let Err(e) = coord_for_ctrl.cancel(&session_id_for_ctrl).await {
                        tracing::error!("Failed to cancel request: {}", e);
                    }
                }
                ControlCommand::Response {
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
                ControlCommand::SetLevel(level) => {
                    if let Err(e) = coord_for_ctrl
                        .set_permission_level(&session_id_for_ctrl, level)
                        .await
                    {
                        tracing::error!("Failed to set permission level: {}", e);
                    }
                }
                ControlCommand::Compact => {
                    if let Err(e) = coord_for_ctrl.compact_session(&session_id_for_ctrl).await {
                        tracing::error!("Failed to compact session: {}", e);
                    }
                }
            }
        }
    });

    // Subscribe to session events (broadcast channel - TUI can lag but won't block)
    let event_rx = coordinator
        .subscribe_session_events(&session_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("Failed to get event receiver for session"))?;

    // On input: update "last session" for current directory
    let on_input_hook: OnInputHook = Box::new({
        let storage = app_storage.clone();
        let dir = ctx.working_dir.clone();
        move |sid: &str| {
            let s = storage.clone();
            let d = dir.clone();
            let id = sid.to_string();
            tokio::spawn(async move {
                s.update_last_session(&d, &id).await.ok();
            });
        }
    });

    let tui_result = run_tui(
        event_rx,
        input_tx,
        ctrl_tx,
        coordinator.session_store().clone(),
        ctx.working_dir.to_string_lossy().to_string(),
        input_history,
        session_messages,
        initial_message,
        session_id.0.clone(),
        Some(on_input_hook),
    )
    .await?;

    // Only record session if the session has actual messages in storage
    let session_messages = coordinator
        .get_session_messages(&session_id)
        .await
        .unwrap_or_default();
    let has_conversation = !session_messages.is_empty();
    if has_conversation {
        // Save last session for this directory
        app_storage
            .save_session(&ctx.working_dir, &session_id.0)
            .await?;
        println!("Goodbye~ You can resume this session later with:");
        println!("yomi --resume {}", session_id.0);
    } else {
        // Delete empty session (no conversation)
        if let Err(e) = coordinator.delete_session(&session_id).await {
            tracing::warn!("Failed to delete empty session: {}", e);
        }
        println!("Goodbye~");
    }

    Ok(SessionResult {
        new_history_entries: tui_result.input_history,
        should_create_new_session: tui_result.should_create_new_session,
        switch_to_session: tui_result.switch_to_session,
    })
}

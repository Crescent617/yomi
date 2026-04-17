//! Session management for CLI

use anyhow::Result;
use kernel::{
    event::ControlCommand,
    types::{ContentBlock, SessionId},
    Coordinator, SessionConfig,
};
use std::path::Path;
use std::sync::Arc;
use tui::run_tui;

use crate::storage::AppStorage;

/// Context needed to run a session
#[derive(Clone)]
pub struct SessionContext {
    pub working_dir: std::path::PathBuf,
    pub skill_names: Vec<String>,
    pub auto_approve: kernel::permissions::Level,
    pub context_window: u32,
}

/// Result of running a session
pub struct SessionResult {
    pub new_history_entries: Vec<String>,
    pub should_create_new_session: bool,
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
}

/// Resolve session from command line arguments
pub async fn resolve_session(
    session_arg: &SessionArg,
    is_first_session: bool,
    coordinator: &Coordinator,
    app_storage: &AppStorage,
    working_dir: &Path,
    mk_config: impl Fn() -> SessionConfig,
) -> Result<SessionId> {
    if !is_first_session {
        return coordinator.create_session(mk_config()).await;
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
                    coordinator.create_session(mk_config()).await
                }
            }
        }
        // --session (no value): resume last session for this directory
        SessionArg::Last => match app_storage.get_last_session(working_dir).await? {
            Some(id) => {
                let session_id = SessionId(id);
                println!("Restoring previous session: {}", session_id.0);

                match coordinator.restore_session(&session_id, mk_config()).await {
                    Ok(_) => Ok(session_id),
                    Err(e) => {
                        println!("Failed to restore session: {e}");
                        println!("Starting new session instead");
                        coordinator.create_session(mk_config()).await
                    }
                }
            }
            None => {
                println!("No previous session found, starting new session");
                coordinator.create_session(mk_config()).await
            }
        },
        // No --session: create new session
        SessionArg::New => coordinator.create_session(mk_config()).await,
    }
}

/// Run a single session lifecycle
pub async fn run_session_loop(
    coordinator: Arc<Coordinator>,
    session_id: SessionId,
    ctx: SessionContext,
    app_storage: Arc<AppStorage>,
    input_history: Vec<String>,
    session_messages: Vec<kernel::types::Message>,
    is_first_session: bool,
) -> Result<SessionResult> {
    // Print startup info
    if is_first_session {
        println!("yomi session started: {}", session_id.0);
        println!("Working directory: {}", ctx.working_dir.display());
    } else {
        println!("yomi new session started: {}", session_id.0);
    }
    println!("Starting TUI...\n");

    // Create channels
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<Vec<ContentBlock>>(100);
    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::channel::<ControlCommand>(10);

    // Spawn input forwarding task
    let coord_for_input = coordinator.clone();
    let session_id_for_input = session_id.clone();
    tokio::spawn(async move {
        while let Some(blocks) = input_rx.recv().await {
            if let Err(e) = coord_for_input
                .send_blocks(&session_id_for_input, blocks)
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

    // Get event receiver and run TUI
    let event_rx = coordinator
        .take_session_event_receiver(&session_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("Failed to get event receiver for session"))?;

    let tui_result = run_tui(
        event_rx,
        input_tx,
        ctrl_tx,
        ctx.working_dir.to_string_lossy().to_string(),
        ctx.skill_names.clone(),
        input_history,
        session_messages,
        ctx.auto_approve,
        ctx.context_window,
    )
    .await?;

    // Record session for future --session
    app_storage
        .record_session(&ctx.working_dir, &session_id.0)
        .await?;

    Ok(SessionResult {
        new_history_entries: tui_result.input_history,
        should_create_new_session: tui_result.should_create_new_session,
    })
}

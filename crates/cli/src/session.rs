//! Session management for CLI

use anyhow::Result;
use kernel::{
    event::ControlCommand,
    tools::file_state::FileStateStore,
    types::{ContentBlock, SessionId},
    Coordinator, SessionConfig,
};
use std::path::Path;
use std::sync::Arc;
use tui::run_tui;

use crate::{storage::AppStorage, utils::DEBUG_MODE};

/// Context needed to run a session
#[derive(Clone)]
pub struct SessionContext {
    pub working_dir: std::path::PathBuf,
    pub skill_names: Vec<String>,
    pub auto_approve: kernel::permissions::Level,
    pub context_window: u32,
    pub data_dir: std::path::PathBuf,
    pub model_name: String,
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
    // Create empty file state store for new sessions
    let empty_file_state = Arc::new(FileStateStore::new());

    if !is_first_session {
        return coordinator
            .create_session(mk_config(), empty_file_state)
            .await;
    }

    match session_arg {
        // --session <id>: restore specific session
        SessionArg::Specific(id) => {
            let session_id = SessionId(id.clone());
            println!("Restoring session: {}", session_id.0);

            // Load file state from session entry if available
            let file_state = match app_storage.load_session(working_dir).await? {
                Some(entry) => match entry.file_state {
                    Some(snapshot) => {
                        tracing::info!("Loaded file state with {} entries", snapshot.entries.len());
                        Arc::new(FileStateStore::from_snapshot(snapshot))
                    }
                    None => Arc::new(FileStateStore::new()),
                },
                None => Arc::new(FileStateStore::new()),
            };

            match coordinator
                .restore_session(&session_id, mk_config(), file_state)
                .await
            {
                Ok(_) => Ok(session_id),
                Err(e) => {
                    println!("Failed to restore session: {e}");
                    println!("Starting new session instead");
                    coordinator
                        .create_session(mk_config(), empty_file_state)
                        .await
                }
            }
        }
        // --session (no value): resume last session for this directory
        SessionArg::Last => match app_storage.load_session(working_dir).await? {
            Some(entry) => {
                let session_id = SessionId(entry.session_id);
                println!("Restoring previous session: {}", session_id.0);

                let file_state = match entry.file_state {
                    Some(snapshot) => {
                        tracing::info!("Loaded file state with {} entries", snapshot.entries.len());
                        Arc::new(FileStateStore::from_snapshot(snapshot))
                    }
                    None => Arc::new(FileStateStore::new()),
                };

                match coordinator
                    .restore_session(&session_id, mk_config(), file_state)
                    .await
                {
                    Ok(_) => Ok(session_id),
                    Err(e) => {
                        println!("Failed to restore session: {e}");
                        println!("Starting new session instead");
                        coordinator
                            .create_session(mk_config(), empty_file_state)
                            .await
                    }
                }
            }
            None => {
                println!("No previous session found, starting new session");
                coordinator
                    .create_session(mk_config(), empty_file_state)
                    .await
            }
        },
        // No --session: create new session
        SessionArg::New => {
            coordinator
                .create_session(mk_config(), empty_file_state)
                .await
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
    is_first_session: bool,
    initial_message: Option<String>,
) -> Result<SessionResult> {
    // Print startup info only in debug mode (DEBUG=1)
    if *DEBUG_MODE {
        if is_first_session {
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
        coordinator.storage().clone(),
        ctx.working_dir.to_string_lossy().to_string(),
        ctx.skill_names.clone(),
        input_history,
        session_messages,
        ctx.auto_approve,
        ctx.context_window,
        initial_message,
        ctx.data_dir.clone(),
        session_id.0.clone(),
        ctx.model_name.clone(),
    )
    .await?;

    // Only record session if the session has actual messages in storage
    let session_messages = coordinator
        .get_session_messages(&session_id)
        .await
        .unwrap_or_default();
    let has_conversation = !session_messages.is_empty();
    if has_conversation {
        // Get file state snapshot and save with session
        let file_state = coordinator
            .get_file_state_snapshot(&session_id)
            .await
            .filter(|s| !s.is_empty());
        if let Some(file_state) = &file_state {
            tracing::info!("Saved file state with {} entries", file_state.entries.len());
        }
        app_storage
            .save_session(&ctx.working_dir, &session_id.0, file_state.as_ref())
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

//! Checkpoint management commands

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use kernel::checkpoint::RewindTarget;
use kernel::storage::StorageSet;

/// Resolve session ID from CLI argument or find the most recent session in current directory.
async fn resolve_session_id(storage: &StorageSet, session_id: Option<String>) -> Result<String> {
    match session_id {
        Some(id) => Ok(id),
        None => {
            // Try to find the most recent session in current directory
            let sessions = storage
                .session_store()
                .list(kernel::storage::ListArgs::default())
                .await?;
            let cwd = std::env::current_dir()?;
            let cwd_str = cwd.to_string_lossy();

            sessions
                .into_iter()
                .find(|s| {
                    s.working_dir
                        .as_ref()
                        .is_some_and(|d| d.starts_with(cwd_str.as_ref()))
                })
                .map(|s| s.id.0)
                .context("No session found in current directory. Use --session to specify one.")
        }
    }
}

/// List checkpoints for a session
pub async fn list(global: &GlobalArgs, session_id: Option<String>) -> Result<()> {
    let data_dir = crate::utils::data_dir(global)?;
    let storage = StorageSet::open(&data_dir).await?;

    let session_id = resolve_session_id(&storage, session_id).await?;

    let checkpoints = storage
        .checkpoint_store()
        .get_session_checkpoints(&session_id)
        .await?;

    if checkpoints.is_empty() {
        println!("No checkpoints found for session {session_id}");
        return Ok(());
    }

    println!("Checkpoints for session {session_id}:");
    println!("{:<36} {:<10} Files Changed", "Message ID", "Seq");
    println!("{}", "-".repeat(80));

    for cp in checkpoints {
        println!(
            "{:<36} {:<10} {}",
            cp.message_id, cp.sequence, cp.files_changed
        );
    }

    Ok(())
}

/// Show details of a specific checkpoint
pub async fn show(
    global: &GlobalArgs,
    session_id: Option<String>,
    message_id: String,
) -> Result<()> {
    let data_dir = crate::utils::data_dir(global)?;
    let storage = StorageSet::open(&data_dir).await?;

    let session_id = resolve_session_id(&storage, session_id).await?;

    // Get all checkpoints for session and filter locally (avoids global scan)
    let checkpoints = storage
        .checkpoint_store()
        .get_session_checkpoints(&session_id)
        .await?;

    let checkpoint = checkpoints.into_iter().find(|c| c.message_id == message_id);

    match checkpoint {
        Some(cp) => {
            println!("Checkpoint Details:");
            println!("  Message ID:    {}", cp.message_id);
            println!("  Session ID:    {}", cp.session_id);
            println!("  Sequence:      {}", cp.sequence);
            println!("  Files Changed: {}", cp.files_changed);
        }
        None => {
            println!("Checkpoint not found: {message_id}");
        }
    }

    Ok(())
}

/// Rewind to a checkpoint
pub async fn rewind(
    _global: &GlobalArgs,
    message_id: String,
    _target: RewindTarget,
    #[allow(unused_variables)] _dry_run: bool,
) -> Result<()> {
    // In a real implementation, this would need to communicate with the running TUI/session
    // For now, we just show what would happen
    println!("To rewind to checkpoint {message_id}, use the /rewind command in TUI.");
    println!("Or restart the session with: yomi --rewind {message_id}");

    Ok(())
}

/// Clean up orphaned backups
pub async fn cleanup(_global: &GlobalArgs, dry_run: bool) -> Result<()> {
    if dry_run {
        println!("Dry run mode - would clean up orphaned backups");
    } else {
        println!("Cleanup functionality has been simplified in V2.");
        println!("Orphaned backups are now cleaned up automatically with checkpoints.");
    }
    Ok(())
}

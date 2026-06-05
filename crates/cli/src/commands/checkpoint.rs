//! Checkpoint management commands

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use comfy_table::{ContentArrangement, Table};
use kernel::checkpoint::RewindTarget;
use kernel::storage::StorageSet;

/// Resolve session ID from CLI argument or find the most recent session in current directory.
async fn resolve_session_id(storage: &StorageSet, session_id: Option<String>) -> Result<String> {
    match session_id {
        Some(id) => Ok(id),
        None => {
            // Try to find the most recent session in current directory
            let (sessions, _) = storage.session_store().list(None, None, 50).await?;
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
    let storage = crate::utils::open_storage(global).await?;

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

    // Build table (no borders)
    let mut table = Table::new();
    table
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["SEQ", "SUMMARY", "FILES", "MESSAGE ID"]);
    table.load_preset(comfy_table::presets::NOTHING);
    // Remove left padding from first column only
    if let Some(col) = table.column_mut(0) {
        col.set_padding((0, 1));
    }

    for cp in &checkpoints {
        let summary = if cp.summary.is_empty() {
            "(no summary)".to_string()
        } else if cp.summary.chars().count() > 40 {
            format!("{}...", cp.summary.chars().take(40).collect::<String>())
        } else {
            cp.summary.clone()
        };

        table.add_row(vec![
            cp.sequence.to_string(),
            summary,
            cp.files_changed.to_string(),
            cp.message_id.clone(),
        ]);
    }

    println!("{table}");

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

use crate::args::GlobalArgs;
use anyhow::Result;
use tokio::fs;

/// Cleanup old session data
pub async fn run(global: GlobalArgs, days: i64, yes: bool) -> Result<()> {
    let data_dir = crate::utils::data_dir(&global)?;
    let storage = crate::utils::open_storage(&global).await?;

    if !yes {
        // Dry-run: only query and show what would be deleted
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
        let (old_sessions, _) = storage
            .session_store()
            .list(None, Some(cutoff), 10000)
            .await?;

        if old_sessions.is_empty() {
            println!("No sessions older than {days} days found.");
        } else {
            println!(
                "Found {} session(s) older than {} days.",
                old_sessions.len(),
                days
            );
            println!("This is a dry-run. Use --yes to actually delete.");
        }
        return Ok(());
    }

    // Perform cleanup (cleanup returns the IDs it deleted)
    let deleted_ids = storage.session_store().cleanup(days).await?;

    if deleted_ids.is_empty() {
        println!("No sessions older than {days} days found.");
        return Ok(());
    }

    // Delete associated data files (messages, todos, file_states)
    let mut files_removed = 0;
    for id in &deleted_ids {
        // Message file: sessions/{id}.jsonl
        let msg_file = data_dir.join("sessions").join(format!("{}.jsonl", id.0));
        if msg_file.exists() {
            if let Err(e) = fs::remove_file(&msg_file).await {
                eprintln!("Warning: failed to remove {}: {}", msg_file.display(), e);
            } else {
                files_removed += 1;
            }
        }

        // Todo file: sessions/todos/{id}.json
        let todo_file = data_dir
            .join("sessions")
            .join("todos")
            .join(format!("{}.json", id.0));
        if todo_file.exists() {
            if let Err(e) = fs::remove_file(&todo_file).await {
                eprintln!("Warning: failed to remove {}: {}", todo_file.display(), e);
            } else {
                files_removed += 1;
            }
        }

        // File state file: sessions/file_states/{id}.jsonl
        let file_state_file = data_dir
            .join("sessions")
            .join("file_states")
            .join(format!("{}.jsonl", id.0));
        if file_state_file.exists() {
            if let Err(e) = fs::remove_file(&file_state_file).await {
                eprintln!(
                    "Warning: failed to remove {}: {}",
                    file_state_file.display(),
                    e
                );
            } else {
                files_removed += 1;
            }
        }
    }

    println!(
        "Deleted {} session(s) and {} associated files.",
        deleted_ids.len(),
        files_removed
    );

    Ok(())
}

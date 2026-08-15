use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use comfy_table::{ContentArrangement, Table};

pub mod cancel;
pub mod cat;
pub mod send;

#[cfg(test)]
#[path = "list_test.rs"]
mod list_test;

/// Resolve a session ID from CLI arg or the current directory's last session.
pub async fn resolve_session_id(global: &GlobalArgs, session: Option<String>) -> Result<String> {
    match session {
        Some(id) => Ok(id),
        None => {
            let data_dir = crate::utils::data_dir(global)?;
            let app_storage = crate::storage::AppStorage::new(&data_dir)?;
            let working_dir = std::env::current_dir()?;
            let entry = app_storage
                .load_session(&working_dir)
                .await?
                .context("No session found for current directory. Use --session <id> or run from a directory with an active session.")?;
            Ok(entry.session_id)
        }
    }
}

/// Filter sessions to a working directory (`-d/--dir` given), capped at
/// 50 rows; `None` passes the full list through (the default).
fn filter_by_dir(
    sessions: Vec<kernel::storage::session::SessionInfo>,
    dir: Option<&std::path::Path>,
) -> Vec<kernel::storage::session::SessionInfo> {
    let Some(dir) = dir else {
        return sessions;
    };
    let dir_str = dir.to_string_lossy();
    sessions
        .into_iter()
        .filter(|s| s.working_dir.as_ref().is_some_and(|wd| wd == &dir_str))
        .take(50)
        .collect()
}

pub async fn list(global: &GlobalArgs, all: bool) -> Result<()> {
    // `-a/--all` is a deprecated no-op: listing now defaults to all
    // sessions; only an explicit -d/--dir filters by working directory.
    let _ = all;
    let storage = crate::utils::open_storage(global).await?;

    // List sessions: all by default, filtered only with an explicit -d/--dir.
    let (sessions, _) = storage
        .session_store()
        .list(
            None,
            kernel::storage::session::SessionListScope::All,
            None,
            200,
        )
        .await?;

    let filter_dir = match &global.dir {
        Some(dir) => Some(dir.canonicalize()?),
        None => None,
    };
    let sessions = filter_by_dir(sessions, filter_dir.as_deref());

    if sessions.is_empty() {
        match &filter_dir {
            Some(dir) => println!("No sessions found for directory: {}", dir.display()),
            None => println!("No sessions found."),
        }
        return Ok(());
    }

    // Build table (no borders)
    let mut table = Table::new();
    table
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["SESSION ID", "AGE", "WORKING DIR", "PREVIEW"]);
    table.load_preset(comfy_table::presets::NOTHING);
    // Remove left padding from first column only
    if let Some(col) = table.column_mut(0) {
        col.set_padding((0, 1));
    }

    for session in &sessions {
        let age_str = session.format_age();
        let preview = session.title.as_ref().map_or_else(
            || "(no user message)".to_string(),
            |t| {
                if t.chars().count() > 50 {
                    format!("{}...", t.chars().take(50).collect::<String>())
                } else {
                    t.clone()
                }
            },
        );
        let working_dir = session
            .working_dir
            .clone()
            .unwrap_or_else(|| "(unknown)".to_string());

        table.add_row(vec![
            &session.id.0.to_string(),
            &age_str,
            &working_dir,
            &preview,
        ]);
    }

    println!("{table}");

    Ok(())
}

use crate::args::GlobalArgs;
use anyhow::Result;
use comfy_table::{ContentArrangement, Table};
use kernel::{ListArgs, StorageSet};

pub mod cleanup;

#[allow(clippy::needless_pass_by_value)]
pub async fn list(global: GlobalArgs, all: bool) -> Result<()> {
    let storage = StorageSet::open(&crate::utils::data_dir(&global)?).await?;

    // Get current working directory
    let current_dir = std::env::current_dir()?;
    let current_dir_str = current_dir.to_string_lossy().to_string();

    // List sessions: by default only current working dir, with --all list all
    // Limit default to 50 to prevent overwhelming output
    let args = ListArgs {
        working_dir: if all {
            None
        } else {
            Some(current_dir_str.clone())
        },
        limit: Some(50),
        ..Default::default()
    };
    let sessions = storage.session_store().list(args).await?;

    if !all && sessions.is_empty() {
        println!("No sessions found for current directory: {current_dir_str}");
        println!("Use --all to list all sessions.");
        return Ok(());
    }

    if sessions.is_empty() {
        println!("No sessions found.");
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

        table.add_row(vec![&session.id.0, &age_str, &working_dir, &preview]);
    }

    println!("{table}");

    Ok(())
}

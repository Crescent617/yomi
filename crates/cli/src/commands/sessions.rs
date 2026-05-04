use crate::args::GlobalArgs;
use anyhow::Result;
use kernel::StorageSet;

#[allow(clippy::needless_pass_by_value)]
pub async fn list(global: GlobalArgs, all: bool) -> Result<()> {
    let storage = StorageSet::open(&crate::utils::data_dir(&global)?).await?;

    // Get current working directory
    let current_dir = std::env::current_dir()?;
    let current_dir_str = current_dir.to_string_lossy().to_string();

    // List sessions: by default only current working dir, with --all list all
    let sessions = if all {
        storage.session_store().list().await?
    } else {
        let filtered = storage
            .session_store()
            .list_by_working_dir(&current_dir_str)
            .await?;
        if filtered.is_empty() {
            println!("No sessions found for current directory: {current_dir_str}");
            println!("Use --all to list all sessions.");
            return Ok(());
        }
        filtered
    };

    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    let mut rows = Vec::new();
    for session in sessions.iter().take(50) {
        let age_str = session.format_age();

        // Use title field for last user message preview
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
        rows.push((session.id.0.clone(), age_str, working_dir, preview));
    }

    let id_width = rows.iter().map(|r| r.0.len()).max().unwrap_or(10).max(10);
    let age_width = rows.iter().map(|r| r.1.len()).max().unwrap_or(5).max(5);
    let wd_width = rows.iter().map(|r| r.2.len()).max().unwrap_or(10).max(10);

    println!(
        "{:<id_width$}  {:<age_width$}  {:<wd_width$}  PREVIEW",
        "SESSION ID",
        "AGE",
        "WORKING DIR",
        id_width = id_width,
        age_width = age_width,
        wd_width = wd_width
    );

    for (id, age, wd, preview) in rows {
        println!("{id:<id_width$}  {age:<age_width$}  {wd:<wd_width$}  {preview}");
    }

    Ok(())
}

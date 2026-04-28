use crate::args::GlobalArgs;
use crate::utils::load_config;
use anyhow::Result;
use kernel::{storage::FsStorage, storage::Storage, types::Role, types::SessionId};
use sqlx::sqlite::SqlitePoolOptions;
use std::path::Path;
use std::sync::Arc;

/// Create `SQLite` pool with proper connection string and PRAGMAs
async fn create_db_pool(db_path: &Path) -> Result<sqlx::SqlitePool> {
    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Create empty file if it doesn't exist (sqlx requirement)
    if !db_path.exists() {
        tokio::fs::File::create(db_path).await?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", db_path.display()))
        .await?;

    // Set busy timeout (5 seconds)
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&pool)
        .await?;

    Ok(pool)
}

#[allow(clippy::needless_pass_by_value)]
pub async fn list(global: GlobalArgs, all: bool) -> Result<()> {
    let config = load_config(global.config.as_ref())?;
    let data_dir = config.data_dir;

    let db_path = data_dir.join("yomi.db");
    let pool = create_db_pool(&db_path).await?;
    let storage = Arc::new(FsStorage::new(data_dir.join("sessions"), pool).await?);

    // Get current working directory
    let current_dir = std::env::current_dir()?;
    let current_dir_str = current_dir.to_string_lossy().to_string();

    // List sessions: by default only current working dir, with --all list all
    let sessions = if all {
        storage.list_sessions().await?
    } else {
        let filtered = storage
            .list_sessions_by_working_dir(&current_dir_str)
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
        let age = chrono::Utc::now() - session.updated_at;
        let age_str = if age.num_days() > 0 {
            format!("{}d ago", age.num_days())
        } else if age.num_hours() > 0 {
            format!("{}h ago", age.num_hours())
        } else if age.num_minutes() > 0 {
            format!("{}m ago", age.num_minutes())
        } else {
            "just now".to_string()
        };

        let preview =
            if let Ok(messages) = storage.get_messages(&SessionId(session.id.clone())).await {
                messages.iter().rev().find(|m| m.role == Role::User).map_or(
                    "(no user message)".to_string(),
                    |m| {
                        let text = m.text_content().replace('\n', " ").trim_start().to_string();
                        if text.chars().count() > 50 {
                            format!("{}...", text.chars().take(50).collect::<String>())
                        } else {
                            text
                        }
                    },
                )
            } else {
                "(error loading messages)".to_string()
            };

        let working_dir = session
            .working_dir
            .clone()
            .unwrap_or_else(|| "(unknown)".to_string());
        rows.push((session.id.clone(), age_str, working_dir, preview));
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

use crate::args::GlobalArgs;
use crate::utils::load_config;
use anyhow::Result;
use kernel::{storage::FsStorage, storage::Storage, types::Role, types::SessionId};
use std::sync::Arc;

#[allow(clippy::needless_pass_by_value)]
pub async fn list(global: GlobalArgs) -> Result<()> {
    let config = load_config(global.config.as_ref())?;
    let data_dir = config.data_dir;

    let storage = Arc::new(FsStorage::new(data_dir.join("sessions"))?);
    let sessions = storage.list_sessions().await?;

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

        rows.push((session.id.clone(), age_str, preview));
    }

    let id_width = rows.iter().map(|r| r.0.len()).max().unwrap_or(10).max(10);
    let age_width = rows.iter().map(|r| r.1.len()).max().unwrap_or(5).max(5);

    println!(
        "{:<id_width$}  {:<age_width$}  PREVIEW",
        "SESSION ID",
        "AGE",
        id_width = id_width,
        age_width = age_width
    );

    for (id, age, preview) in rows {
        println!("{id:<id_width$}  {age:<age_width$}  {preview}");
    }

    Ok(())
}

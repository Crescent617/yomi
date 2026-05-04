use crate::args::GlobalArgs;
use crate::storage::open_storage;
use anyhow::Result;
use chrono::{Duration, Utc};
use kernel::storage::Storage;

/// Show token usage for a time range (default: last 7 days)
pub async fn show(global: GlobalArgs, days: i64) -> Result<()> {
    let storage = open_storage(global).await?;

    let end = Utc::now();
    let start = end - Duration::days(days);

    let summary = storage.get_token_usage_summary(start, end).await?;

    // Format time range
    let start_str = start.format("%Y-%m-%d");
    let end_str = end.format("%Y-%m-%d");

    println!("Token Usage ({start_str} to {end_str})");
    println!();

    if summary.request_count == 0 {
        println!("No token usage recorded for this period.");
        return Ok(());
    }

    println!("📊 Summary:");
    println!("  Requests:     {}", summary.request_count);
    println!("  Prompt:       {} tokens", summary.total_prompt);
    println!("  Completion:   {} tokens", summary.total_completion);
    println!("  Cached:       {} tokens", summary.total_cached);
    println!("  ─────────────────────────");
    println!("  Total:        {} tokens", summary.total_tokens());

    Ok(())
}

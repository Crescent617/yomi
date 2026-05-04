use crate::args::GlobalArgs;
use anyhow::Result;
use chrono::{Duration, Local, TimeZone};
use kernel::StorageSet;

/// Show token usage for a time range (default: last 7 days)
pub async fn show(global: GlobalArgs, days: i64) -> Result<()> {
    let storage = StorageSet::open(&crate::utils::data_dir(&global)?).await?;

    // Calculate date range by whole days in local timezone
    let now_local = Local::now();
    let today_local = now_local.date_naive();
    let today_start_local = today_local.and_hms_opt(0, 0, 0).unwrap();
    let tomorrow_start_local = today_start_local + Duration::days(1);

    // Calculate range in local time, then convert to UTC for database query
    let start_local = today_start_local - Duration::days(days);
    let start_utc = Local.from_local_datetime(&start_local).unwrap().to_utc();
    let end_utc = Local
        .from_local_datetime(&tomorrow_start_local)
        .unwrap()
        .to_utc();

    let summary = storage.usage_store().summarize(start_utc, end_utc).await?;

    // Format time range using local dates
    let start_str = start_local.format("%Y-%m-%d");
    let end_str = today_start_local.format("%Y-%m-%d");

    println!("Token Usage ({start_str} to {end_str})");
    println!();

    if summary.request_count == 0 {
        println!("No token usage recorded for this period.");
        return Ok(());
    }

    println!("📊 Summary:");
    println!("  Requests:     {}", summary.request_count);
    println!("  Prompt:       {} tokens", summary.prompt_tokens);
    println!("  Completion:   {} tokens", summary.completion_tokens);
    println!("  Cached:       {} tokens", summary.cached_tokens);
    println!("  ─────────────────────────");
    println!("  Total:        {} tokens", summary.total_tokens());

    Ok(())
}

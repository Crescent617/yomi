use crate::args::GlobalArgs;
use anyhow::Result;
use chrono::{Duration, Local, TimeZone};
use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, ContentArrangement, Table};
use kernel::StorageSet;

/// Format a number with compact notation (K, M, B)
#[allow(clippy::cast_precision_loss)] // Precision loss acceptable for display
fn fmt_compact(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Format cache rate as percentage
#[allow(clippy::cast_precision_loss)] // Precision loss acceptable for display
fn fmt_cache_rate(cached: u64, prompt: u64) -> String {
    if prompt == 0 {
        "-".to_string()
    } else {
        format!("{:.0}%", (cached as f64 / prompt as f64) * 100.0)
    }
}

/// Parse date string and return day name (Mon, Tue, etc.)
fn day_name(date_str: &str) -> String {
    chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .map(|d| d.format("%a").to_string())
        .unwrap_or_default()
}

/// Show token usage for a time range (default: last 7 days)
pub async fn show(global: GlobalArgs, days: i64) -> Result<()> {
    let storage = StorageSet::open(&crate::utils::data_dir(&global)?).await?;

    // Calculate date range by whole days in local timezone
    let now_local = Local::now();
    let today_local = now_local.date_naive();
    let today_start_local = today_local.and_hms_opt(0, 0, 0).unwrap();
    let tomorrow_start_local = today_start_local + Duration::days(1);

    // Calculate range in local time, then convert to UTC for database query
    let start_local = today_start_local - Duration::days(days - 1);
    let start_utc = Local.from_local_datetime(&start_local).unwrap().to_utc();
    let end_utc = Local
        .from_local_datetime(&tomorrow_start_local)
        .unwrap()
        .to_utc();

    let daily = storage
        .usage_store()
        .daily_summary(start_utc, end_utc)
        .await?;

    if daily.is_empty() {
        println!("No data");
        return Ok(());
    }

    // Calculate totals
    let total_prompt: u64 = daily.iter().map(|d| d.prompt_tokens).sum();
    let total_completion: u64 = daily.iter().map(|d| d.completion_tokens).sum();
    let total_cached: u64 = daily.iter().map(|d| d.cached_tokens).sum();
    let total_tokens: u64 = daily.iter().map(|d| d.total_tokens()).sum();
    let total_requests: u64 = daily.iter().map(|d| d.request_count).sum();

    // Build table
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            "Date",
            "Day",
            "Prompt",
            "Completion",
            "Cached",
            "Cache%",
            "Total",
            "Req",
        ]);

    for day in &daily {
        table.add_row(vec![
            day.date[5..].to_string(), // MM-DD
            day_name(&day.date),
            fmt_compact(day.prompt_tokens),
            fmt_compact(day.completion_tokens),
            fmt_compact(day.cached_tokens),
            fmt_cache_rate(day.cached_tokens, day.prompt_tokens),
            fmt_compact(day.total_tokens()),
            day.request_count.to_string(),
        ]);
    }

    // Add separator and total row
    table.add_row(vec!["", "", "", "", "", "", "", ""]);
    table.add_row(vec![
        "Total".to_string(),
        String::new(),
        fmt_compact(total_prompt),
        fmt_compact(total_completion),
        fmt_compact(total_cached),
        fmt_cache_rate(total_cached, total_prompt),
        fmt_compact(total_tokens),
        total_requests.to_string(),
    ]);

    println!("{table}");

    Ok(())
}

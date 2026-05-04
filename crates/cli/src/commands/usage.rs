use crate::args::GlobalArgs;
use anyhow::Result;
use chrono::{Duration, Local, TimeZone};
use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Attribute, Cell, Color, ContentArrangement,
    Table,
};
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

/// Get color based on value's percentile in the distribution
#[allow(clippy::cast_precision_loss)] // Precision loss acceptable for percentile calculation
fn color_by_percentile(value: u64, sorted_values: &[u64]) -> Color {
    if sorted_values.len() <= 1 {
        return Color::Reset;
    }

    // Find the rank of this value in sorted array
    let rank = sorted_values.partition_point(|&v| v < value);
    let percentile = rank as f64 / sorted_values.len() as f64;

    match percentile {
        p if p < 0.33 => Color::Green,
        p if p < 0.67 => Color::Yellow,
        _ => Color::Red,
    }
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

    // Build sorted arrays for percentile calculation
    let mut prompt_values: Vec<u64> = daily.iter().map(|d| d.prompt_tokens).collect();
    let mut completion_values: Vec<u64> = daily.iter().map(|d| d.completion_tokens).collect();
    let mut cached_values: Vec<u64> = daily.iter().map(|d| d.cached_tokens).collect();
    let mut total_values: Vec<u64> = daily.iter().map(|d| d.total_tokens()).collect();

    prompt_values.sort_unstable();
    completion_values.sort_unstable();
    cached_values.sort_unstable();
    total_values.sort_unstable();

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
        let cache_rate = fmt_cache_rate(day.cached_tokens, day.prompt_tokens);
        let cache_color = if cache_rate.starts_with('0') {
            Color::DarkGrey
        } else {
            Color::Reset
        };
        table.add_row(vec![
            Cell::new(&day.date[5..]), // MM-DD
            Cell::new(day_name(&day.date)),
            Cell::new(fmt_compact(day.prompt_tokens))
                .fg(color_by_percentile(day.prompt_tokens, &prompt_values)),
            Cell::new(fmt_compact(day.completion_tokens)).fg(color_by_percentile(
                day.completion_tokens,
                &completion_values,
            )),
            Cell::new(fmt_compact(day.cached_tokens))
                .fg(color_by_percentile(day.cached_tokens, &cached_values)),
            Cell::new(&cache_rate).fg(cache_color),
            Cell::new(fmt_compact(day.total_tokens()))
                .fg(color_by_percentile(day.total_tokens(), &total_values)),
            Cell::new(day.request_count),
        ]);
    }

    // Add total row (bold)
    table.add_row(vec![
        Cell::new("Total").add_attribute(Attribute::Bold),
        Cell::new(""),
        Cell::new(fmt_compact(total_prompt)).add_attribute(Attribute::Bold),
        Cell::new(fmt_compact(total_completion)).add_attribute(Attribute::Bold),
        Cell::new(fmt_compact(total_cached)).add_attribute(Attribute::Bold),
        Cell::new(fmt_cache_rate(total_cached, total_prompt)).add_attribute(Attribute::Bold),
        Cell::new(fmt_compact(total_tokens)).add_attribute(Attribute::Bold),
        Cell::new(total_requests).add_attribute(Attribute::Bold),
    ]);

    println!("{table}");

    Ok(())
}

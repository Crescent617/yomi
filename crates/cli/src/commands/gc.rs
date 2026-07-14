//! `yomi gc` - garbage collect expired session resources
//!
//! Removes all resources associated with sessions whose `updated_at` is older
//! than the cutoff: sqlite rows (sessions, channel mappings), message history,
//! todos, goals, file states, checkpoint directories and unreferenced assets. Also sweeps
//! orphan files left behind by past bugs or interrupted writes.
//!
//! The `token_usage` table is never touched.

use crate::args::GlobalArgs;
use anyhow::Result;
use clap::Parser;
use kernel::storage::{GcOptions, GcReport};

#[derive(Parser)]
#[allow(clippy::struct_excessive_bools)]
pub struct GcArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Collect sessions older than this many days (minimum 1)
    #[arg(long, default_value = "90", value_parser = clap::value_parser!(i64).range(1..))]
    pub days: i64,

    /// Actually delete data (dry-run by default)
    #[arg(short, long)]
    pub yes: bool,

    /// Also collect pinned sessions
    #[arg(long)]
    pub include_pinned: bool,

    /// Skip the orphan file sweep
    #[arg(long)]
    pub no_orphans: bool,

    /// Run VACUUM on the sqlite database after deletion
    #[arg(long)]
    pub vacuum: bool,

    /// Output the report as JSON
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: GcArgs) -> Result<()> {
    let storage = crate::utils::open_storage(&args.global).await?;

    let opts = GcOptions {
        days: args.days,
        keep_pinned: !args.include_pinned,
        sweep_orphans: !args.no_orphans,
        vacuum: args.vacuum,
        dry_run: !args.yes,
    };

    let report = storage.gc().run(&opts).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    print_report(&report, args.days);
    Ok(())
}

fn print_report(report: &GcReport, days: i64) {
    let mode = if report.dry_run { " (dry-run)" } else { "" };
    println!("yomi gc{mode} — sessions older than {days} days\n");

    if report.sessions.is_empty()
        && report.orphan_files_deleted == 0
        && report.assets_deleted == 0
        && report.errors.is_empty()
    {
        println!("  Nothing to collect.");
        return;
    }

    if report.subagent_sessions > 0 {
        println!(
            "  sessions      {:>6}  (including {} subagents)",
            report.sessions.len(),
            report.subagent_sessions
        );
    } else {
        println!("  sessions      {:>6}", report.sessions.len());
    }
    println!("  data files    {:>6}", report.files_deleted);
    println!("  checkpoints   {:>6} dirs", report.checkpoint_dirs_deleted);
    if report.channel_mappings_deleted > 0 {
        println!(
            "  channel maps  {:>6} rows",
            report.channel_mappings_deleted
        );
    }
    println!("  orphan files  {:>6}", report.orphan_files_deleted);
    println!("  assets        {:>6}", report.assets_deleted);
    let reclaim_label = if report.dry_run {
        "est. reclaim"
    } else {
        "reclaimed   "
    };
    println!(
        "  {reclaim_label}  {:>6}",
        format_bytes(report.bytes_reclaimed)
    );

    for err in &report.errors {
        eprintln!("  warning: {err}");
    }

    if report.dry_run {
        println!("\nRun again with --yes to delete.");
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(195_500_000), "186.4 MB");
        assert_eq!(format_bytes(2_147_483_648), "2.0 GB");
    }

    #[test]
    fn test_gc_args_defaults_to_dry_run() {
        let args = GcArgs::try_parse_from(["gc"]).unwrap();
        assert_eq!(args.days, 90);
        assert!(!args.yes);
        assert!(!args.include_pinned);
        assert!(!args.no_orphans);
        assert!(!args.vacuum);
        assert!(!args.json);
    }

    #[test]
    fn test_gc_args_days_minimum() {
        assert!(GcArgs::try_parse_from(["gc", "--days", "0"]).is_err());
        assert!(GcArgs::try_parse_from(["gc", "--days", "-5"]).is_err());
        assert!(GcArgs::try_parse_from(["gc", "--days", "1"]).is_ok());
    }
}

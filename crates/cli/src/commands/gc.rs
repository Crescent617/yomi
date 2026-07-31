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

    /// Collect sessions older than this many days (minimum 1; default: config `[gc]` `retention_days`)
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
    pub days: Option<i64>,

    /// Actually delete data (dry-run by default)
    #[arg(short, long)]
    pub yes: bool,

    /// Skip pinned sessions (default: config `[gc]` `keep_pinned`)
    #[arg(long, num_args = 0..=1, default_missing_value = "true", conflicts_with = "include_pinned")]
    pub keep_pinned: Option<bool>,

    /// Also collect pinned sessions (alias for `--keep-pinned false`)
    #[arg(long)]
    pub include_pinned: bool,

    /// Sweep orphan files whose session no longer exists (default: config `[gc]` `sweep_orphans`)
    #[arg(long, num_args = 0..=1, default_missing_value = "true", conflicts_with = "no_orphans")]
    pub sweep_orphans: Option<bool>,

    /// Skip the orphan file sweep (alias for `--sweep-orphans false`)
    #[arg(long)]
    pub no_orphans: bool,

    /// Run VACUUM on the sqlite database after deletion (default: config `[gc]` vacuum)
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub vacuum: Option<bool>,

    /// Output the report as JSON
    #[arg(long)]
    pub json: bool,
}

/// Merge CLI flags over the configured gc policy. Unset flags fall back to
/// `config.gc`; `dry_run` is controlled only by `--yes`.
fn resolve_options(args: &GcArgs, gc: &kernel::config::GcConfig) -> GcOptions {
    let mut opts = GcOptions::from_config(gc, !args.yes);
    if let Some(days) = args.days {
        opts.retention_days = days;
    }
    if let Some(keep_pinned) = args.keep_pinned {
        opts.keep_pinned = keep_pinned;
    }
    if args.include_pinned {
        opts.keep_pinned = false;
    }
    if let Some(sweep_orphans) = args.sweep_orphans {
        opts.sweep_orphans = sweep_orphans;
    }
    if args.no_orphans {
        opts.sweep_orphans = false;
    }
    if let Some(vacuum) = args.vacuum {
        opts.vacuum = vacuum;
    }
    opts
}

pub async fn run(args: GcArgs) -> Result<()> {
    let config = crate::utils::load_config(args.global.config.as_ref())?;
    let storage = kernel::StorageSet::open_with_config(&config.data_dir, &config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open storage: {e}"))?;

    let opts = resolve_options(&args, &config.gc);
    let report = storage.gc().run(&opts).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    print_report(&report, opts.retention_days);
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
        assert_eq!(args.days, None);
        assert!(!args.yes);
        assert_eq!(args.keep_pinned, None);
        assert!(!args.include_pinned);
        assert_eq!(args.sweep_orphans, None);
        assert!(!args.no_orphans);
        assert_eq!(args.vacuum, None);
        assert!(!args.json);
    }

    #[test]
    fn test_resolve_options_falls_back_to_config() {
        let args = GcArgs::try_parse_from(["gc"]).unwrap();
        let gc = kernel::config::GcConfig {
            retention_days: 30,
            keep_pinned: false,
            sweep_orphans: false,
            vacuum: true,
            auto: true,
        };
        let opts = resolve_options(&args, &gc);
        assert_eq!(opts.retention_days, 30);
        assert!(!opts.keep_pinned);
        assert!(!opts.sweep_orphans);
        assert!(opts.vacuum);
        assert!(opts.dry_run);
    }

    #[test]
    fn test_resolve_options_flags_override_config() {
        let args = GcArgs::try_parse_from([
            "gc",
            "--yes",
            "--days",
            "7",
            "--keep-pinned",
            "true",
            "--sweep-orphans",
            "false",
            "--vacuum",
        ])
        .unwrap();
        let gc = kernel::config::GcConfig {
            retention_days: 30,
            keep_pinned: false,
            vacuum: false,
            ..Default::default()
        };
        let opts = resolve_options(&args, &gc);
        assert_eq!(opts.retention_days, 7);
        assert!(opts.keep_pinned);
        assert!(!opts.sweep_orphans);
        assert!(opts.vacuum);
        assert!(!opts.dry_run);
    }

    #[test]
    fn test_resolve_options_compat_aliases() {
        let args = GcArgs::try_parse_from(["gc", "--include-pinned", "--no-orphans"]).unwrap();
        let opts = resolve_options(&args, &kernel::config::GcConfig::default());
        assert!(!opts.keep_pinned);
        assert!(!opts.sweep_orphans);
    }

    #[test]
    fn test_gc_args_conflicting_flags_rejected() {
        assert!(
            GcArgs::try_parse_from(["gc", "--keep-pinned", "true", "--include-pinned"]).is_err()
        );
        assert!(GcArgs::try_parse_from(["gc", "--sweep-orphans", "--no-orphans"]).is_err());
    }

    #[test]
    fn test_gc_args_days_minimum() {
        assert!(GcArgs::try_parse_from(["gc", "--days", "0"]).is_err());
        assert!(GcArgs::try_parse_from(["gc", "--days", "-5"]).is_err());
        assert!(GcArgs::try_parse_from(["gc", "--days", "1"]).is_ok());
    }
}

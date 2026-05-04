use anyhow::Result;
use clap::{Parser, Subcommand};

mod args;
mod commands;
mod misc;
mod session;
mod storage;
mod utils;

use args::GlobalArgs;
use commands::tui;

#[derive(Parser)]
#[command(name = "yomi")]
#[command(about = "AI coding assistant CLI")]
struct Args {
    #[command(flatten)]
    tui: tui::TuiArgs,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start TUI session (default when no subcommand provided)
    Tui(tui::TuiArgs),
    /// Manage sessions
    Session(SessionArgs),
    /// Manage skills
    Skill(SkillArgs),
    /// Manage configuration
    Config(ConfigArgs),
    /// Show token usage
    Usage(UsageArgs),
    /// Show version
    Version,
}

#[derive(Parser)]
struct SessionArgs {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: SessionsCommands,
}

#[derive(Subcommand)]
enum SessionsCommands {
    /// List sessions (default: current directory only)
    List {
        /// List all sessions, not just current directory
        #[arg(short, long)]
        all: bool,
    },
}

#[derive(Parser)]
struct SkillArgs {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: SkillsCommands,
}

#[derive(Subcommand)]
enum SkillsCommands {
    /// List all available skills
    List,
}

#[derive(Parser)]
struct ConfigArgs {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: ConfigCommands,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show current configuration
    Show,
    /// Get a configuration value
    Get {
        /// The configuration key to get (e.g., provider, `model.api_key`)
        key: String,
    },
    /// Set a configuration value
    Set {
        /// The configuration key to set (e.g., provider, `model.api_key`)
        key: String,
        /// The value to set
        value: String,
    },
}

#[derive(Parser)]
struct UsageArgs {
    #[command(flatten)]
    global: GlobalArgs,

    /// Number of days to look back
    #[arg(short = 'n', long, default_value = "7")]
    days: i64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Some(Commands::Tui(tui_args)) => tui::run(tui_args).await,
        Some(Commands::Session(args)) => run_session(args).await,
        Some(Commands::Skill(args)) => run_skill(args).await,
        Some(Commands::Config(args)) => run_config(args).await,
        Some(Commands::Usage(args)) => run_usage(args).await,
        Some(Commands::Version) => {
            println!("v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => tui::run(args.tui).await,
    }
}

async fn run_session(args: SessionArgs) -> Result<()> {
    match args.command {
        SessionsCommands::List { all } => commands::sessions::list(args.global, all).await,
    }
}

async fn run_skill(args: SkillArgs) -> Result<()> {
    match args.command {
        SkillsCommands::List => commands::skills::list(args.global).await,
    }
}

async fn run_config(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommands::Show => commands::config::show(args.global),
        ConfigCommands::Get { key } => commands::config::get(args.global, &key),
        ConfigCommands::Set { key, value } => commands::config::set(args.global, &key, value),
    }
}

async fn run_usage(args: UsageArgs) -> Result<()> {
    commands::usage::show(args.global, args.days).await
}

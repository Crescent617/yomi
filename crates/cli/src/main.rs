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
    Sessions(SessionsArgs),
    /// Manage skills
    Skills(SkillsArgs),
    /// Manage configuration
    Config(ConfigArgs),
    /// Show version
    Version,
}

#[derive(Parser)]
struct SessionsArgs {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: SessionsCommands,
}

#[derive(Subcommand)]
enum SessionsCommands {
    /// List all sessions
    List,
}

#[derive(Parser)]
struct SkillsArgs {
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Some(Commands::Tui(tui_args)) => tui::run(tui_args).await,
        Some(Commands::Sessions(args)) => run_sessions(args).await,
        Some(Commands::Skills(args)) => run_skills(args).await,
        Some(Commands::Config(args)) => run_config(args).await,
        Some(Commands::Version) => {
            println!("v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => tui::run(args.tui).await,
    }
}

async fn run_sessions(args: SessionsArgs) -> Result<()> {
    match args.command {
        SessionsCommands::List => commands::sessions::list(args.global).await,
    }
}

async fn run_skills(args: SkillsArgs) -> Result<()> {
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

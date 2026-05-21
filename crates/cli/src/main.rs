use anyhow::Result;
use clap::{Parser, Subcommand};

mod args;
mod commands;
mod daemon;
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
    /// Manage daemon (internal use)
    #[command(subcommand)]
    Daemon(commands::daemon::DaemonCommands),
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
    /// Cancel an active session (stops the agent loop)
    Cancel {
        /// Session ID to cancel (defaults to current directory's last session)
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Shutdown a running session (remove from daemon memory)
    Stop {
        /// Session ID to stop (defaults to current directory's last session)
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Cleanup old sessions and their data
    Cleanup {
        /// Delete sessions older than this many days
        #[arg(long, default_value = "180")]
        days: i64,
        /// Actually delete data (dry-run by default)
        #[arg(short, long)]
        yes: bool,
    },
    /// Manage checkpoints for a session
    Checkpoint(SessionCheckpointArgs),
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
struct SessionCheckpointArgs {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: SessionCheckpointCommands,
}

#[derive(Subcommand)]
enum SessionCheckpointCommands {
    /// List checkpoints for a session
    List {
        /// Session ID (defaults to current directory's session)
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Rewind to a checkpoint (shows what would happen, use TUI for actual rewind)
    Rewind {
        /// Message ID of the checkpoint
        message_id: String,
        /// Only restore conversation (not files)
        #[arg(long, group = "target")]
        conversation: bool,
        /// Only restore files (not conversation)
        #[arg(long, group = "target")]
        files: bool,
        /// Dry run - show what would happen
        #[arg(long)]
        dry_run: bool,
    },
    /// Clean up orphaned backup files
    Cleanup {
        /// Actually delete files (dry-run by default)
        #[arg(short, long)]
        yes: bool,
    },
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

    /// Filter by model name (e.g. claude-3-5-sonnet, gpt-4o)
    #[arg(long)]
    model: Option<String>,

    /// Filter by provider name (e.g. anthropic, openai)
    #[arg(long)]
    provider: Option<String>,
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
        Some(Commands::Daemon(cmd)) => commands::daemon::run(cmd).await,
        None => tui::run(args.tui).await,
    }
}

async fn run_session(args: SessionArgs) -> Result<()> {
    match args.command {
        SessionsCommands::List { all } => commands::session::list(&args.global, all).await,
        SessionsCommands::Cancel { session } => {
            commands::session::cancel::run(&args.global, session).await
        }
        SessionsCommands::Stop { session } => {
            commands::session::stop::run(&args.global, session).await
        }
        SessionsCommands::Cleanup { days, yes } => {
            commands::session::cleanup::run(args.global, days, yes).await
        }
        SessionsCommands::Checkpoint(cp_args) => run_session_checkpoint(cp_args).await,
    }
}

async fn run_session_checkpoint(args: SessionCheckpointArgs) -> Result<()> {
    use kernel::checkpoint::RewindTarget;

    match args.command {
        SessionCheckpointCommands::List { session } => {
            commands::checkpoint::list(&args.global, session).await
        }
        SessionCheckpointCommands::Rewind {
            message_id,
            conversation,
            files,
            dry_run,
        } => {
            let target = if conversation {
                RewindTarget::Conversation
            } else if files {
                RewindTarget::Files
            } else {
                RewindTarget::Both
            };
            commands::checkpoint::rewind(&args.global, message_id, target, dry_run).await
        }
        SessionCheckpointCommands::Cleanup { yes } => {
            commands::checkpoint::cleanup(&args.global, !yes).await
        }
    }
}

async fn run_skill(args: SkillArgs) -> Result<()> {
    match args.command {
        SkillsCommands::List => commands::skill::list(&args.global).await,
    }
}

async fn run_config(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommands::Show => commands::config::show(&args.global),
        ConfigCommands::Get { key } => commands::config::get(&args.global, &key),
        ConfigCommands::Set { key, value } => commands::config::set(&args.global, &key, value),
    }
}

async fn run_usage(args: UsageArgs) -> Result<()> {
    let filter = if args.model.is_none() && args.provider.is_none() {
        None
    } else {
        Some(kernel::storage::usage::UsageFilter {
            model: args.model,
            provider: args.provider,
            usage_type: None,
        })
    };
    commands::usage::show(args.global, args.days, filter).await
}

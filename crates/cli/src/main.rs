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
    /// Run a single prompt headlessly and print the result
    Run(commands::run::RunArgs),
    /// Manage sessions
    Session(SessionArgs),
    /// Garbage collect expired session data (dry-run by default)
    Gc(commands::gc::GcArgs),
    /// Manage skills
    Skill(SkillArgs),
    /// Manage configuration
    Config(ConfigArgs),
    /// Show token usage
    Usage(UsageArgs),
    /// Stream session events from the daemon as NDJSON
    Events(EventsArgs),
    /// Manage cron jobs
    Cron(CronArgs),
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
    /// Send a message to a session (requires the daemon; queues if the agent is busy)
    Send {
        /// Message text (reads from stdin when omitted)
        message: Vec<String>,
        /// Session ID (defaults to current directory's last session)
        #[arg(short, long)]
        session: Option<String>,
        /// Inject into the current run (takes effect between turns) instead of queueing
        #[arg(long)]
        steer: bool,
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

#[derive(Parser)]
struct EventsArgs {
    #[command(flatten)]
    global: GlobalArgs,

    /// Session ID to subscribe to (defaults to current directory's last session)
    #[arg(short, long, conflicts_with = "all")]
    session: Option<String>,

    /// Subscribe to events from all sessions (real-time only, no replay)
    #[arg(long)]
    all: bool,

    /// Resume after this event ID (single-session mode only)
    #[arg(long)]
    after_event_id: Option<String>,
}

#[derive(Parser)]
struct CronArgs {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: CronCommands,
}

#[derive(Subcommand)]
enum CronCommands {
    /// List cron jobs
    List {
        /// Filter by status: active, paused, completed, failed
        #[arg(long)]
        status: Option<String>,
        /// Max jobs to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Show one cron job as JSON
    Get {
        /// Cron job ID
        job_id: String,
    },
    /// Create a cron job
    Create {
        /// Job name
        #[arg(long)]
        name: String,
        /// Cron expression (5 or 6 fields, interpreted in local time)
        #[arg(long)]
        schedule: String,
        /// Action: send a message to a session (the agent responds)
        #[arg(long, conflicts_with = "command", required_unless_present = "command")]
        message: Option<String>,
        /// Action: run a shell command (exit 42 marks the job completed)
        #[arg(long)]
        command: Option<String>,
        /// Target session for --message (default: a dedicated session)
        #[arg(long, requires = "message")]
        session: Option<String>,
        /// Working directory for --command
        #[arg(long, requires = "command")]
        work_dir: Option<String>,
        /// Stop after N runs (default: unlimited)
        #[arg(long)]
        max_runs: Option<u32>,
        /// Expire at this time, RFC 3339 (default: never)
        #[arg(long)]
        expires_at: Option<String>,
    },
    /// Update a cron job (only given fields change)
    Update {
        /// Cron job ID
        job_id: String,
        /// New name
        #[arg(long)]
        name: Option<String>,
        /// New cron expression
        #[arg(long)]
        schedule: Option<String>,
        /// Replace the action with a session message
        #[arg(long, conflicts_with = "command")]
        message: Option<String>,
        /// Replace the action with a shell command (exit 42 marks the job completed)
        #[arg(long)]
        command: Option<String>,
        /// Target session for --message (omit for a dedicated session)
        #[arg(long, requires = "message")]
        session: Option<String>,
        /// Working directory for --command
        #[arg(long, requires = "command")]
        work_dir: Option<String>,
        /// Stop after N runs (0 = back to unlimited)
        #[arg(long)]
        max_runs: Option<u32>,
        /// Expire at this time, RFC 3339 ("never" = back to no expiry)
        #[arg(long)]
        expires_at: Option<String>,
    },
    /// Pause a cron job
    Pause {
        /// Cron job ID
        job_id: String,
    },
    /// Resume a paused cron job
    Resume {
        /// Cron job ID
        job_id: String,
    },
    /// Delete a cron job
    Delete {
        /// Cron job ID
        job_id: String,
    },
    /// Run a cron job once, immediately
    Trigger {
        /// Cron job ID
        job_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install rustls crypto provider before any TLS operations.
    // This is required by rustls 0.23+ when multiple crypto providers
    // are available in the dependency tree.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let args = Args::parse();

    match args.command {
        Some(Commands::Tui(tui_args)) => tui::run(tui_args).await,
        Some(Commands::Run(args)) => commands::run::run(args).await,
        Some(Commands::Session(args)) => run_session(args).await,
        Some(Commands::Gc(args)) => commands::gc::run(args).await,
        Some(Commands::Skill(args)) => run_skill(args).await,
        Some(Commands::Config(args)) => run_config(args).await,
        Some(Commands::Usage(args)) => run_usage(args).await,
        Some(Commands::Events(args)) => {
            commands::events::run(&args.global, args.session, args.all, args.after_event_id).await
        }
        Some(Commands::Cron(args)) => run_cron(args).await,
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
        SessionsCommands::Send {
            message,
            session,
            steer,
        } => commands::session::send::run(&args.global, message, session, steer).await,
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

async fn run_cron(args: CronArgs) -> Result<()> {
    use kernel::cron::CronJobStatus;

    match args.command {
        CronCommands::List { status, limit } => {
            commands::cron::list(&args.global, status, limit).await
        }
        CronCommands::Get { job_id } => commands::cron::get(&args.global, job_id).await,
        CronCommands::Create {
            name,
            schedule,
            message,
            command,
            session,
            work_dir,
            max_runs,
            expires_at,
        } => {
            commands::cron::create(
                &args.global,
                name,
                schedule,
                message,
                command,
                session,
                work_dir,
                max_runs,
                expires_at,
            )
            .await
        }
        CronCommands::Update {
            job_id,
            name,
            schedule,
            message,
            command,
            session,
            work_dir,
            max_runs,
            expires_at,
        } => {
            commands::cron::update(
                &args.global,
                job_id,
                name,
                schedule,
                message,
                command,
                session,
                work_dir,
                max_runs,
                expires_at,
            )
            .await
        }
        CronCommands::Pause { job_id } => {
            commands::cron::set_status(&args.global, job_id, CronJobStatus::Paused).await
        }
        CronCommands::Resume { job_id } => {
            commands::cron::set_status(&args.global, job_id, CronJobStatus::Active).await
        }
        CronCommands::Delete { job_id } => commands::cron::delete(&args.global, job_id).await,
        CronCommands::Trigger { job_id } => commands::cron::trigger(&args.global, job_id).await,
    }
}

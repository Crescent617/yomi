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
#[command(about = "AI coding assistant CLI", version)]
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
    /// Send a raw wire-protocol request to the daemon (debug/tooling)
    Rpc(RpcArgs),
    /// Manage cron jobs
    Cron(CronArgs),
    /// Drive platform channels (open a thread with a fresh session)
    Channel(commands::channel::ChannelArgs),
    /// Health-check the daemon, channels, cron, storage and config
    Doctor(GlobalArgs),
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
    /// List sessions (all by default; -d/--dir filters to a directory)
    List {
        /// Deprecated no-op: listing now defaults to all sessions
        #[arg(short, long)]
        all: bool,
    },
    /// Cancel an active session (stops the agent loop)
    Cancel {
        /// Session ID to cancel (defaults to current directory's last session)
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
    /// Read a session's message log (friendly transcript by default)
    Cat {
        /// Session ID (defaults to current directory's last session)
        #[arg(short, long)]
        session: Option<String>,
        /// Dump the raw JSONL file (large inline base64 payloads elided)
        #[arg(long)]
        raw: bool,
        /// Include tool calls (name/args/result) in the transcript
        #[arg(long)]
        tools: bool,
        /// Include thinking blocks (excluded by default; --raw always shows everything)
        #[arg(long)]
        verbose: bool,
        /// Show the message at this JSONL line number (from `session search`)
        #[arg(long, conflicts_with = "raw")]
        line: Option<usize>,
        /// Also show this many lines before/after --line
        #[arg(long, requires = "line", default_value = "2")]
        context: usize,
    },
    /// List pending mailbox items (steer + queued messages)
    Mailbox {
        /// Session ID (defaults to current directory's last session)
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Full-text search across session histories
    Search(commands::session::search::SearchArgs),
    /// Retract one pending mailbox item (already-consumed ids fail safely)
    MailboxRemove {
        /// Mailbox item id (mbx_...)
        item_id: String,
        /// Session ID (defaults to current directory's last session)
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Clear pending mailbox items without cancelling the run
    MailboxClear {
        /// Session ID (defaults to current directory's last session)
        #[arg(short, long)]
        session: Option<String>,
        /// Clear only the steer queue
        #[arg(long, conflicts_with = "queue")]
        steer: bool,
        /// Clear only the normal queue
        #[arg(long, conflicts_with = "steer")]
        queue: bool,
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

/// Send a raw wire-protocol request to the daemon (debug/tooling)
#[derive(Parser)]
#[command(disable_help_flag = true)]
pub struct RpcArgs {
    #[command(flatten)]
    global: GlobalArgs,

    /// Wire method name in `snake_case` (`--help` lists all methods,
    /// `<METHOD> --help` shows one method's parameters), or a full method
    /// JSON object, e.g. `{"get_session": {"session_id": "sess_…"}}`
    method: Option<String>,

    /// Method parameters as a JSON object (reads from stdin when omitted and piped)
    params: Option<String>,

    /// Print compact single-line JSON instead of pretty-printed
    #[arg(long)]
    compact: bool,

    /// Print help: alone lists all wire methods; after METHOD shows its parameters
    #[arg(short, long)]
    help: bool,
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
        /// Target session for --message (default: a fresh session per run)
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
        /// Sensor gate: run this shell command before each scheduled trigger;
        /// exit 0 lets the run proceed (message jobs also get its stdout),
        /// non-zero skips the run silently
        #[arg(long)]
        precheck: Option<String>,
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
        /// Target session for --message (omit for a fresh session per run)
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
        /// Sensor gate shell command (empty string clears the gate)
        #[arg(long)]
        precheck: Option<String>,
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
        Some(Commands::Rpc(args)) => commands::rpc::run(args).await,
        Some(Commands::Cron(args)) => run_cron(args).await,
        Some(Commands::Channel(args)) => commands::channel::run(args).await,
        Some(Commands::Doctor(global)) => commands::doctor::run(&global).await,
        Some(Commands::Version) => {
            println!("v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(Commands::Daemon(cmd)) => commands::daemon::run(cmd, &args.tui.global).await,
        None => tui::run(args.tui).await,
    }
}

async fn run_session(args: SessionArgs) -> Result<()> {
    match args.command {
        SessionsCommands::List { all } => commands::session::list(&args.global, all).await,
        SessionsCommands::Cancel { session } => {
            commands::session::cancel::run(&args.global, session).await
        }
        SessionsCommands::Send {
            message,
            session,
            steer,
        } => commands::session::send::run(&args.global, message, session, steer).await,
        SessionsCommands::Cat {
            session,
            raw,
            tools,
            verbose,
            line,
            context,
        } => {
            commands::session::cat::run(&args.global, session, raw, tools, verbose, line, context)
                .await
        }
        SessionsCommands::Mailbox { session } => {
            commands::session::mailbox::list(&args.global, session).await
        }
        SessionsCommands::Search(search_args) => {
            commands::session::search::run_cli(&search_args).await
        }
        SessionsCommands::MailboxRemove { item_id, session } => {
            commands::session::mailbox::remove(&args.global, session, item_id).await
        }
        SessionsCommands::MailboxClear {
            session,
            steer,
            queue,
        } => commands::session::mailbox::clear(&args.global, session, steer, queue).await,
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
            precheck,
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
                precheck,
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
            precheck,
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
                precheck,
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

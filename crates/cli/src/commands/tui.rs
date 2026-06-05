use crate::{
    args::GlobalArgs,
    daemon,
    session::{resolve_session, run_session_loop, SessionArg, SessionContext},
    storage::AppStorage,
    utils::DEBUG_MODE,
};
use anyhow::{Context, Result};
use kernel::{
    client::{CoordinatorApi, RemoteCoordinator},
    config::{Config, ModelProvider},
    permissions::Level,
    utils::strs,
    AnthropicProvider, OpenAIProvider,
};
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Maximum stdin size to prevent OOM (400KB)
const MAX_STDIN_SIZE: u64 = 400 * 1024;

#[derive(Default, clap::Parser)]
pub struct TuiArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Initial prompt to send on startup (non-interactive mode if provided)
    #[arg(short, long, value_name = "PROMPT")]
    pub prompt: Option<String>,

    /// Skip all confirmations (YOLO mode)
    #[arg(short, long)]
    pub yolo: bool,

    /// Resume a session: --resume (last session) or --resume <id> (specific)
    ///
    /// Uses `Option<Option<String>>` to distinguish three cases:
    /// - `None`: --resume not provided (create new session)
    /// - `Some(None)`: --resume provided without value (resume last session)
    /// - `Some(Some(id))`: --resume <id> provided (resume specific session)
    #[arg(short, long, value_name = "SESSION_ID")]
    #[allow(clippy::option_option)]
    pub resume: Option<Option<String>>,

    /// Fork a session: --fork (last session) or --fork <id> (specific)
    ///
    /// Creates a new session with copied history from the source session.
    /// Uses `Option<Option<String>>` to distinguish three cases:
    /// - `None`: --fork not provided
    /// - `Some(None)`: --fork provided without value (fork last session)
    /// - `Some(Some(id))`: --fork <id> provided (fork specific session)
    #[arg(short, long, value_name = "SESSION_ID")]
    #[allow(clippy::option_option)]
    pub fork: Option<Option<String>>,

    /// Run in-process without daemon (local coordinator)
    #[arg(long, visible_alias = "fg")]
    pub no_daemon: bool,
}

impl TuiArgs {
    /// Build initial message from prompt (-p) and stdin content.
    ///
    /// Reads stdin if piped (non-TTY), with 400KB size limit.
    /// Combines prompt and stdin content:
    /// - prompt + stdin: "{prompt}\n\n```\n{stdin}\n```"
    /// - prompt only: "{prompt}"
    /// - stdin only: "{stdin}"
    /// - neither: None
    pub async fn build_initial_message(&self) -> Result<Option<String>> {
        let prompt = self.prompt.clone();

        // Quick check: if TTY, no stdin to read
        if io::stdin().is_terminal() {
            return Ok(prompt);
        }

        // Read stdin in blocking thread to avoid blocking async runtime
        let stdin_result = tokio::task::spawn_blocking(move || {
            // Pre-allocate up to 8KB initially to avoid over-allocation for small input
            let mut buffer = String::with_capacity((MAX_STDIN_SIZE as usize).min(8192));
            let mut stdin = io::stdin().take(MAX_STDIN_SIZE);

            match stdin.read_to_string(&mut buffer) {
                Ok(0) => Ok(None),
                Ok(n) => {
                    // Only warn if we actually hit the limit (have more data waiting)
                    // This avoids false positives when stdin is exactly MAX_STDIN_SIZE
                    if n >= MAX_STDIN_SIZE as usize {
                        // Try to read one more byte to confirm truncation
                        let mut extra = [0u8; 1];
                        if io::stdin().read(&mut extra).is_ok_and(|n| n > 0) {
                            tracing::warn!("Stdin truncated at {}KB limit", MAX_STDIN_SIZE / 1024);
                        }
                    }
                    let trimmed = buffer.trim_end().to_string();
                    Ok(if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    })
                }
                Err(e) => Err(e),
            }
        })
        .await;

        // Handle JoinError (panic in blocking task) vs IO error
        let stdin_content = match stdin_result {
            Ok(Ok(content)) => content,
            Ok(Err(e)) => {
                tracing::warn!("Failed to read stdin: {}", e);
                None
            }
            Err(e) => {
                tracing::warn!("Stdin reading task panicked: {}", e);
                None
            }
        };

        Ok(match (prompt, stdin_content) {
            (Some(p), Some(s)) => Some(format!("{p}\n\n```\n{s}\n```")),
            (Some(p), None) => Some(p),
            (None, Some(s)) => Some(s),
            (None, None) => None,
        })
    }
}

pub async fn run(args: TuiArgs) -> Result<()> {
    let working_dir = crate::utils::resolve_working_dir(&args.global)?;

    let mut config = crate::utils::load_config(args.global.config.as_ref(), &working_dir)?;

    // Load feature gates from environment
    let feature_gates = tui::FeatureGates::from_env();

    if args.yolo {
        config.auto_approve = Level::Dangerous;
        tracing::warn!("YOLO mode enabled - all confirmations skipped!");
    }

    tokio::fs::create_dir_all(&config.data_dir).await?;

    let app_storage = Arc::new(AppStorage::new(config.data_dir.clone())?);
    let _log_guard = init_logging(&config)?;

    let coordinator: Arc<dyn CoordinatorApi> = if args.no_daemon {
        create_local_coordinator(&config, &working_dir).await?
    } else {
        daemon::spawn_daemon().await?;
        Arc::new(RemoteCoordinator::new(daemon::socket_addr()))
    };

    print_startup_info(&config);

    // Initialize global config for TUI
    tui::init_config(config.clone(), feature_gates);
    tui::init_daemon_mode(!args.no_daemon);

    let session_ctx = SessionContext {
        working_dir: working_dir.clone(),
    };

    let mut is_launch = true; // First session in this process, should respect --resume/--fork args
    let mut input_history = app_storage
        .load_input_history(&working_dir)
        .await
        .unwrap_or_default();

    let mut session_arg = if let Some(ref fork) = args.fork {
        // --fork takes precedence
        match fork {
            None => SessionArg::ForkLast,
            Some(id) => SessionArg::ForkSpecific(id.clone()),
        }
    } else {
        match args.resume {
            Some(None) => SessionArg::Last,
            Some(Some(ref id)) => SessionArg::Specific(id.clone()),
            None => SessionArg::New,
        }
    };

    // Build initial message once; consumed by .take() so it is only sent
    // on the very first session, never on switch or /new.
    let mut initial_message = args.build_initial_message().await?;

    loop {
        let session_id = resolve_session(
            &session_arg,
            is_launch,
            coordinator.as_ref(),
            &app_storage,
            &working_dir,
            config.auto_approve,
        )
        .await?;

        let result = run_session_loop(
            coordinator.clone(),
            session_id,
            session_ctx.clone(),
            app_storage.clone(),
            input_history.clone(),
            is_launch,
            initial_message.take(),
            config.auto_approve,
        )
        .await?;

        // Save all new history entries at once (merge + dedup + trim in one shot)
        if let Err(e) = app_storage
            .save_input_history(&working_dir, &result.new_history_entries)
            .await
        {
            tracing::warn!("Failed to save input history: {}", e);
        }
        input_history.extend(result.new_history_entries);
        let limit = tui::INPUT_HISTORY_LIMIT;
        if input_history.len() > limit {
            input_history = input_history.split_off(input_history.len() - limit / 2);
        }

        // Handle session switching (/sessions command)
        if let Some(switch_to_id) = result.switch_to_session {
            session_arg = SessionArg::Specific(switch_to_id);
            is_launch = true; // Treat as launch to trigger restore flow
            continue;
        }

        if result.should_create_new_session {
            is_launch = false; // Subsequent session, ignore --resume/--fork args
            session_arg = SessionArg::New;
            continue;
        }

        break;
    }

    Ok(())
}

async fn create_local_coordinator(
    config: &Config,
    working_dir: &Path,
) -> Result<Arc<kernel::Coordinator>> {
    let storage = kernel::StorageSet::open_with_config(&config.data_dir, config).await?;
    let provider = create_provider(config)?;
    let task_store = Arc::new(kernel::TaskStore::new(&config.data_dir).await?);
    let skill_folders = resolve_skill_folders(config, working_dir);

    let agent_config = tokio::task::spawn_blocking({
        let config = config.clone();
        let working_dir = working_dir.to_path_buf();
        move || kernel::server::build_agent_config(&config, &working_dir)
    })
    .await?;

    Ok(kernel::Coordinator::new(
        &storage,
        provider,
        agent_config,
        Some(task_store),
        Some(config.agent.compactor.clone()),
        skill_folders,
        config.features.hooks.then(|| {
            kernel::hooks::build_registry(&config.hooks, config.features.allow_command_hooks)
        }),
    ))
}

/// Resolve skill folders against working directory.
/// Relative paths are joined with `working_dir`, absolute paths are kept as-is.
/// Resolve skill folders against working directory.
/// Relative paths are joined with `working_dir`, absolute paths are kept as-is.
pub fn resolve_skill_folders(config: &Config, working_dir: &Path) -> Vec<PathBuf> {
    config
        .skill_folders()
        .iter()
        .map(PathBuf::from)
        .map(|p| {
            if p.is_relative() {
                working_dir.join(p)
            } else {
                p
            }
        })
        .collect()
}

pub(crate) fn create_provider(config: &Config) -> Result<Arc<dyn kernel::Provider>> {
    if !config.has_api_key() {
        eprintln!("Error: API key not configured.");
        std::process::exit(1);
    }

    let provider: Arc<dyn kernel::Provider> = match config.agent.model.provider {
        ModelProvider::OpenAI => Arc::new(OpenAIProvider::new()?),
        ModelProvider::Anthropic => Arc::new(AnthropicProvider::new()?),
    };

    Ok(provider)
}

fn print_startup_info(config: &Config) {
    if *DEBUG_MODE {
        println!("Provider: {}", config.agent.model.provider);
        println!("Model: {}", config.agent.model.model_id);
        println!("Endpoint: {}", config.agent.model.endpoint);
        let api_key = config.api_key();
        let key_preview = if api_key.len() > 8 {
            strs::truncate_with_suffix(api_key, 11, "...")
        } else {
            "not set".to_string()
        };
        println!("API Key: {key_preview}\n");
    }
}

pub(crate) fn init_logging(
    config: &Config,
) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let log_dir = config.log_dir();

    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create log directory: {}", log_dir.display()))?;

    let log_path = log_dir.join("app.log");
    let file_appender = tracing_rolling_file::RollingFileAppenderBase::builder()
        .filename(log_path.to_string_lossy().to_string())
        .condition_max_file_size(10 * 1024 * 1024)
        .max_filecount(5)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to create rolling file appender: {e}"))?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .context("Failed to create env filter")?;

    // Use try_init to avoid panic if already initialized (e.g., in tests)
    if tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true),
        )
        .try_init()
        .is_ok()
    {
        tracing::info!("Logging initialized. Log directory: {}", log_dir.display());
        Ok(Some(guard))
    } else {
        drop(guard);
        Ok(None)
    }
}

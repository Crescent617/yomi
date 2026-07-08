use crate::{
    args::GlobalArgs,
    daemon,
    session::{resolve_session, run_session_loop, SessionArg, SessionContext},
    storage::AppStorage,
    utils::DEBUG_MODE,
};
use anyhow::Result;
use kernel::{
    client::{KernelApi, RemoteKernel},
    config::Config,
    permission::Level,
    utils::strs,
};
use std::io::{self, IsTerminal, Read};
use std::sync::Arc;

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

    /// Run with background daemon (external kernel)
    #[arg(long, visible_alias = "bg")]
    pub daemon: bool,
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

    let mut config = crate::utils::load_config(args.global.config.as_ref())?;

    // Load feature gates from environment
    let feature_gates = tui::FeatureGates::from_env();

    if args.yolo {
        config.auto_approve = Level::Dangerous;
        tracing::warn!("YOLO mode enabled - all confirmations skipped!");
    }

    tokio::fs::create_dir_all(&config.data_dir).await?;

    let app_storage = Arc::new(AppStorage::new(config.data_dir.clone())?);
    let _log_guard = kernel::utils::logging::init_logging(&config, "tui", false)?;

    let kernel: Arc<dyn KernelApi> = if args.daemon {
        daemon::spawn_daemon().await?;
        Arc::new(RemoteKernel::new(daemon::socket_addr()))
    } else {
        create_local_kernel(&config).await?
    };

    print_startup_info(&config);

    // Initialize global config for TUI
    tui::init_config(config.clone(), feature_gates);
    tui::init_daemon_mode(args.daemon);

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
            kernel.as_ref(),
            &app_storage,
            &working_dir,
            config.auto_approve,
        )
        .await?;

        let result = run_session_loop(
            kernel.clone(),
            session_id,
            session_ctx.clone(),
            app_storage.clone(),
            input_history.clone(),
            is_launch,
            initial_message.take(),
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

    kernel.stop();
    Ok(())
}

async fn create_local_kernel(config: &Config) -> Result<Arc<kernel::Kernel>> {
    let kernel = kernel::build_kernel(config, false)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to build kernel: {e}"))?;
    kernel.start();
    Ok(kernel)
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

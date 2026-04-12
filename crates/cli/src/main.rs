use anyhow::{Context, Result};
use clap::Parser;
use kernel::{
    agent::AgentConfig,
    config::{env_names, Config, ModelProvider},
    expand_tilde,
    skill::SkillLoader,
    storage::{FsStorage, Storage},
    tools::{enable_yolo_mode, file_state::FileStateStore, ToolRegistry},
    types::SessionId,
    utils::strs,
    ReadTool, TaskStore,
};
use kernel::{AnthropicProvider, EditTool, OpenAIProvider};
use kernel::{Coordinator, SessionConfig};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use tui::run_tui;

mod storage;
use storage::AppStorage;

#[derive(Parser)]
#[command(name = "yomi")]
#[command(about = "AI coding assistant CLI")]
struct Args {
    /// Working directory
    #[arg(short, long)]
    directory: Option<PathBuf>,

    /// Config file path
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Skip all confirmations (YOLO mode)
    #[arg(long)]
    yolo: bool,

    /// Resume the last session for this working directory
    #[arg(short, long)]
    resume: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.yolo {
        enable_yolo_mode();
        tracing::warn!("YOLO mode enabled - all confirmations skipped!");
    }

    let working_dir = args
        .directory
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let working_dir = working_dir.canonicalize()?;

    // Load configuration with priority: env vars > config file > defaults
    let config = if let Some(config_path) = args.config {
        Config::from_file(&config_path)?
    } else {
        // Try default config locations, fallback to env-only config
        let default_paths = [
            expand_tilde("~/.yomi/config.toml"),
            expand_tilde("~/.config/yomi/config.toml"),
            working_dir.join("yomi.toml"),
        ];
        let mut loaded = None;
        for path in &default_paths {
            if path.exists() {
                tracing::info!("Loading config from: {}", path.display());
                loaded = Some(Config::from_file(path)?);
                break;
            }
        }
        loaded.unwrap_or_else(Config::from_env)
    };

    // Create data directory
    tokio::fs::create_dir_all(&config.data_dir).await?;

    // Initialize AppStorage for session index and input history
    let app_storage = Arc::new(AppStorage::new(config.data_dir.clone())?);

    // Initialize logging with file output and env filter
    init_logging(&config)?;

    // Load skills from configured folders
    // Default to ./.claude/skills if no folders configured via env
    let skill_folders = if config.skill_folders.is_empty() {
        &vec!["~/.yomi/skills".into(), "~/.claude/skills".into()]
    } else {
        &config.skill_folders
    };

    tracing::debug!("Loading skills from folders: {:?}", skill_folders);

    let skills: Vec<Arc<kernel::skill::Skill>> = {
        let loader = SkillLoader::new(skill_folders.iter().map(expand_tilde).collect());
        loader.load_all().unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load skills: {e}");
            Vec::new()
        })
    };

    // Log loaded skills
    if !skills.is_empty() {
        tracing::info!("Loaded {} skill(s)", skills.len());
        for skill in &skills {
            tracing::info!("  - {} (from {})", skill.name, skill.source_path.display());
        }
    }

    // Validate API key
    if !config.has_api_key() {
        eprintln!("Error: API key not configured.");
        std::process::exit(1);
    }

    // Create storage
    let storage = Arc::new(FsStorage::new(config.data_dir.join("sessions"))?);

    // Create provider based on configuration
    let provider: Arc<dyn kernel::Provider> = match config.provider {
        ModelProvider::OpenAI => Arc::new(OpenAIProvider::new()?),
        ModelProvider::Anthropic => Arc::new(AnthropicProvider::new()?),
    };

    // Create task store with shared session ID
    let task_store = Arc::new(TaskStore::new(&config.data_dir).await?);
    let current_session_id = Arc::new(std::sync::Mutex::new(String::new()));

    // Create file state store for tracking reads
    let file_state_store = Arc::new(FileStateStore::new());

    // Create tool registry
    let tool_registry = ToolRegistry::new();

    // Register Edit tool with file state store
    tool_registry.register(Arc::new(
        EditTool::new(&working_dir).with_file_state_store(file_state_store.clone()),
    ));

    // Register Read tool with file state store
    tool_registry.register(Arc::new(
        ReadTool::new(&working_dir).with_file_state_store(file_state_store.clone()),
    ));

    // Register task tools
    let session_id_for_tasks = current_session_id.clone();
    tool_registry.register_task_tools(task_store, move || {
        session_id_for_tasks.lock().unwrap().clone()
    });

    let coordinator = Arc::new(Coordinator::new(
        storage.clone(),
        provider,
        tool_registry,
        config.model.clone(),
    ));

    // Prepare banner data (before skills is moved)
    let working_dir_str = working_dir.to_string_lossy().to_string();
    let skill_names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();

    // Build agent config
    let agent_config = AgentConfig {
        model: config.model.clone(),
        skills,
        ..Default::default()
    };

    // Extract context_window before agent_config is moved
    let context_window = agent_config.compactor.context_window;

    // Create or restore session
    let session_id = if args.resume {
        // Try to restore last session
        match app_storage.get_last_session(&working_dir).await? {
            Some(session_id_str) => {
                let session_id = SessionId(session_id_str);
                tracing::info!("Restoring session: {}", session_id.0);
                println!("Restoring previous session: {}", session_id.0);

                let session_config = SessionConfig {
                    agent: agent_config.clone(),
                    project_path: working_dir.clone(),
                };

                match coordinator
                    .restore_session(&session_id, session_config)
                    .await
                {
                    Ok(_) => session_id,
                    Err(e) => {
                        println!("Failed to restore session: {e}");
                        println!("Starting new session instead");
                        let session_config = SessionConfig {
                            agent: agent_config,
                            project_path: working_dir.clone(),
                        };
                        coordinator.create_session(session_config).await?
                    }
                }
            }
            None => {
                println!("No previous session found, starting new session");
                let session_config = SessionConfig {
                    agent: agent_config,
                    project_path: working_dir.clone(),
                };
                coordinator.create_session(session_config).await?
            }
        }
    } else {
        // Create new session
        let session_config = SessionConfig {
            agent: agent_config,
            project_path: working_dir.clone(),
        };
        coordinator.create_session(session_config).await?
    };

    // Update current session ID for task tools
    *current_session_id.lock().unwrap() = session_id.0.clone();

    // Record this session for future --continue
    app_storage
        .record_session(&working_dir, &session_id.0)
        .await?;

    println!("yomi session started: {}", session_id.0);
    println!("Working directory: {}", working_dir.display());
    println!("Provider: {}", config.provider);
    println!("Model: {}", config.model.model_id);
    println!("Endpoint: {}", config.model.endpoint);
    let api_key = config.api_key();
    let key_preview = if api_key.len() > 8 {
        strs::truncate_with_suffix(api_key, 11, "...")
    } else {
        "not set".to_string()
    };
    println!("API Key: {key_preview}");
    println!("Starting TUI...\n");

    // Load session messages for displaying in chat view
    let session_messages = storage.get_messages(&session_id).await.unwrap_or_default();

    // Create channel for input forwarding
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(100);
    // Create channel for cancel requests
    let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::channel::<()>(10);

    // Spawn task to forward input to coordinator
    let coord_for_input = coordinator.clone();
    let session_id_for_input = session_id.clone();
    tokio::spawn(async move {
        while let Some(content) = input_rx.recv().await {
            if let Err(e) = coord_for_input
                .send_message(&session_id_for_input, content)
                .await
            {
                tracing::error!("Failed to send message: {}", e);
            }
        }
    });

    // Spawn task to handle cancel requests
    let coord_for_cancel = coordinator.clone();
    let session_id_for_cancel = session_id.clone();
    tokio::spawn(async move {
        while cancel_rx.recv().await == Some(()) {
            if let Err(e) = coord_for_cancel.cancel(&session_id_for_cancel).await {
                tracing::error!("Failed to cancel request: {}", e);
            }
        }
    });

    // Get event receiver from coordinator for the session
    let event_rx = coordinator
        .take_session_event_receiver(&session_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("Failed to get event receiver for session"))?;

    // Load input history for this working directory
    let input_history = app_storage
        .load_input_history(&working_dir)
        .await
        .unwrap_or_default();

    // Run TUI with banner data, input history and session messages
    let new_history_entries = run_tui(
        event_rx,
        input_tx,
        cancel_tx,
        working_dir_str,
        skill_names,
        input_history,
        session_messages,
        context_window,
    )
    .await?;

    // Save new history entries
    for entry in &new_history_entries {
        app_storage.add_input_entry(&working_dir, entry).await?;
    }

    println!("Goodbye!");
    Ok(())
}

/// Initialize logging with console and file output
///
/// Environment variables:
/// - `RUST_LOG`: Set log level (e.g., "debug", "info", "warn", "error")
/// - `YOMI_LOG_DIR`: Log directory (default: "~/.yomi/logs")
fn init_logging(config: &Config) -> Result<()> {
    // Get log directory from env or default to ~/.yomi/logs
    let log_dir = std::env::var(env_names::LOG_DIR)
        .map_or_else(|_| config.data_dir.join("logs"), PathBuf::from);

    // Ensure log directory exists
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create log directory: {}", log_dir.display()))?;

    // Create rolling file appender - single file yomi.log with rotation
    // Max 10MB per file, keep 5 backups (yomi.log.1, yomi.log.2, etc.)
    let log_path = log_dir.join("yomi.log");
    let file_appender = tracing_rolling_file::RollingFileAppenderBase::builder()
        .filename(log_path.to_string_lossy().to_string())
        .condition_max_file_size(10 * 1024 * 1024) // 10MB
        .max_filecount(5)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to create rolling file appender: {e}"))?;

    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Leak the guard to keep it alive for the program duration
    Box::leak(Box::new(_guard));

    // Build env filter - try RUST_LOG first, then default to info
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .context("Failed to create env filter")?;

    // Initialize subscriber with file layer only (TUI uses stdout for display)
    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true),
        )
        .init();

    tracing::info!("Logging initialized. Log directory: {}", log_dir.display());

    Ok(())
}

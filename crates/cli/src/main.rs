use anyhow::{Context, Result};
use clap::Parser;
use kernel::{
    agent::AgentConfig,
    config::{env_names, Config, ModelProvider},
    event::PermissionCommand,
    expand_tilde,
    misc::plugin::PluginLoader,
    permissions::Level,
    skill::SkillLoader,
    storage::{FsStorage, Storage},
    types::SessionId,
    utils::strs,
};
use kernel::{AnthropicProvider, OpenAIProvider, TaskStore};
use kernel::{Coordinator, SessionConfig};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use tui::{run_tui, TuiResult};

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
    #[arg(short, long)]
    yolo: bool,

    /// Auto-approve level for tool permissions (safe | caution | dangerous)
    #[arg(long, value_name = "LEVEL")]
    auto_approve: Option<String>,

    /// Resume the last session for this working directory
    #[arg(short, long)]
    resume: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let working_dir = args
        .directory
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let working_dir = working_dir.canonicalize()?;

    // Load configuration with priority: env vars > config file > defaults
    let mut config = if let Some(config_path) = args.config {
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
                tracing::debug!("Loading config from: {}", path.display());
                loaded = Some(Config::from_file(path)?);
                break;
            }
        }
        loaded.unwrap_or_else(Config::from_env)
    };

    // Apply CLI overrides to config (after loading, before using)
    if let Some(level_str) = args.auto_approve {
        if let Ok(level) = Level::from_str(&level_str) {
            config.auto_approve = level;
            tracing::info!("Auto-approve level set to: {}", level);
        } else {
            tracing::warn!("Invalid auto-approve level: {}", level_str);
        }
    }

    if args.yolo {
        config.auto_approve = Level::Dangerous;
        tracing::warn!("YOLO mode enabled - all confirmations skipped!");
    }

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

    let mut skills: Vec<Arc<kernel::skill::Skill>> = {
        let loader = SkillLoader::new(skill_folders.iter().map(expand_tilde).collect());
        loader.load_all().unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load skills: {e}");
            Vec::new()
        })
    };

    // Load plugins and their skills (if enabled)
    if config.load_claude_plugins {
        let plugin_dirs = if config.plugin_dirs.is_empty() {
            vec![expand_tilde("~/.claude/plugins/cache")]
        } else {
            config.plugin_dirs.clone()
        };

        tracing::debug!("Loading plugins from directories: {:?}", plugin_dirs);

        let plugins = {
            let loader = PluginLoader::new(plugin_dirs);
            loader.load_all().unwrap_or_else(|e| {
                tracing::warn!("Failed to load plugins: {e}");
                Vec::new()
            })
        };

        // Log loaded plugins and load their skills
        if !plugins.is_empty() {
            tracing::debug!("Loaded {} plugin(s)", plugins.len());
            for plugin in &plugins {
                tracing::debug!("  - {} (from {})", plugin.name, plugin.path.display());
                match SkillLoader::load_from_plugin(plugin) {
                    Ok(plugin_skills) => {
                        for skill in plugin_skills {
                            tracing::debug!("    - skill: {}", skill.name);
                            skills.push(skill);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load skills from plugin {}: {e}", plugin.name);
                    }
                }
            }
        }
    } else {
        tracing::info!("Claude plugins loading is disabled");
    }

    // Deduplicate skills by name (regular skills take precedence over plugin skills)
    // We keep the first occurrence since regular skills are loaded before plugin skills
    let mut seen_names = std::collections::HashSet::new();
    skills.retain(|skill| {
        if seen_names.contains(&skill.name) {
            tracing::debug!(
                "Duplicate skill name '{}' found, keeping first instance.",
                skill.name
            );
            false
        } else {
            seen_names.insert(skill.name.clone());
            true
        }
    });

    // Log loaded skills
    if !skills.is_empty() {
        tracing::info!("Loaded {} skill(s)", skills.len());
        for skill in &skills {
            tracing::debug!("  - {} (from {})", skill.name, skill.source_path.display());
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

    // Create task store for all agents
    let task_store = Arc::new(TaskStore::new(&config.data_dir).await?);

    // Load project memory (CLAUDE.md/AGENTS.md)
    let project_memory = kernel::project_memory::load(&working_dir).await?;

    let coordinator = Arc::new(Coordinator::new(
        storage.clone(),
        provider,
        config.model.clone(),
        Some(task_store),
        project_memory,
        None, // Compactor is per-agent via agent_config
    ));

    // Prepare banner data (before skills is moved)
    let working_dir_str = working_dir.to_string_lossy().to_string();
    let skill_names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();

    // Build agent config (cloneable for session creation)
    let mk_agent_config = || AgentConfig {
        model: config.model.clone(),
        skills: skills.clone(),
        ..Default::default()
    };

    // Extract context_window from default config
    let context_window = mk_agent_config().compactor.context_window;

    // Helper to create session config
    let mk_config = || SessionConfig {
        agent: mk_agent_config(),
        project_path: working_dir.clone(),
        auto_approve_level: config.auto_approve,
    };

    // Main loop: create session, run TUI, optionally create new session
    let mut is_first_session = true;
    let mut input_history = app_storage
        .load_input_history(&working_dir)
        .await
        .unwrap_or_default();

    loop {
        // Create or restore session
        let session_id = if is_first_session && args.resume {
            match app_storage.get_last_session(&working_dir).await? {
                Some(id) => {
                    let session_id = SessionId(id);
                    println!("Restoring previous session: {}", session_id.0);

                    match coordinator
                        .restore_session(&session_id, mk_config())
                        .await
                    {
                        Ok(_) => session_id,
                        Err(e) => {
                            println!("Failed to restore session: {e}");
                            println!("Starting new session instead");
                            coordinator.create_session(mk_config()).await?
                        }
                    }
                }
                None => {
                    println!("No previous session found, starting new session");
                    coordinator.create_session(mk_config()).await?
                }
            }
        } else {
            coordinator.create_session(mk_config()).await?
        };

        // Record this session for future --continue
        app_storage
            .record_session(&working_dir, &session_id.0)
            .await?;

        if is_first_session {
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
        } else {
            println!("yomi new session started: {}", session_id.0);
        }
        println!("Starting TUI...\n");

        // Load session messages for displaying in chat view
        let session_messages = storage.get_messages(&session_id).await.unwrap_or_default();

        // Create channel for input forwarding
        let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(100);
        // Create channel for cancel requests
        let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::channel::<()>(10);
        // Create channel for permission responses
        let (permission_tx, mut permission_rx) = tokio::sync::mpsc::channel::<PermissionCommand>(10);

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

        // Spawn task to handle permission commands
        let coord_for_permission = coordinator.clone();
        let session_id_for_permission = session_id.clone();
        tokio::spawn(async move {
            while let Some(cmd) = permission_rx.recv().await {
                match cmd {
                    PermissionCommand::Response {
                        req_id,
                        approved,
                        remember,
                    } => {
                        tracing::debug!(
                            "CLI received permission response: req_id={} approved={} remember={}",
                            req_id,
                            approved,
                            remember
                        );
                        if let Err(e) = coord_for_permission
                            .send_permission_response(
                                &session_id_for_permission,
                                &req_id,
                                approved,
                                remember,
                            )
                            .await
                        {
                            tracing::error!("Failed to send permission response: {}", e);
                        }
                    }
                    PermissionCommand::SetLevel(level) => {
                        tracing::debug!("CLI received SetLevel command: {:?}", level);
                        if let Err(e) = coord_for_permission
                            .set_permission_level(&session_id_for_permission, level)
                            .await
                        {
                            tracing::error!("Failed to set permission level: {}", e);
                        }
                    }
                }
            }
        });

        // Get event receiver from coordinator for the session
        let event_rx = coordinator
            .take_session_event_receiver(&session_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to get event receiver for session"))?;

        // Run TUI with banner data, input history and session messages
        let tui_result = run_tui(
            event_rx,
            input_tx,
            cancel_tx,
            permission_tx,
            working_dir_str.clone(),
            skill_names.clone(),
            input_history.clone(),
            session_messages,
            config.auto_approve,
            context_window,
        )
        .await?;

        // Save new history entries
        input_history.extend(tui_result.input_history.clone());
        for entry in &tui_result.input_history {
            app_storage.add_input_entry(&working_dir, entry).await?;
        }

        // Check if we should create a new session
        if tui_result.should_create_new_session {
            is_first_session = false;
            continue;
        }

        // Otherwise, exit the loop
        break;
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

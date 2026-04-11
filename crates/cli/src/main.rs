use anyhow::{Context, Result};
use clap::Parser;
use kernel::{
    agent::AgentConfig,
    config::{env_names, Config, ModelProvider},
    expand_tilde,
    skill::SkillLoader,
    storage::FsStorage,
    tools::{enable_yolo_mode, ToolRegistry},
    utils::strs,
    ReadTool,
};
use kernel::{AnthropicProvider, EditTool, OpenAIProvider};
use kernel::{Coordinator, SessionConfig};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use tui::run_tui;

#[derive(Parser)]
#[command(name = "yomi")]
#[command(about = "AI coding assistant CLI")]
struct Args {
    #[arg(short, long)]
    directory: Option<PathBuf>,

    /// Provider to use (openai, anthropic)
    #[arg(short, long)]
    provider: Option<String>,

    /// Model ID (e.g., gpt-4, claude-3-5-sonnet-20241022)
    #[arg(short, long)]
    model: Option<String>,

    /// API endpoint URL
    #[arg(long)]
    endpoint: Option<String>,

    /// API key (or set env var)
    #[arg(long)]
    api_key: Option<String>,

    /// Skip all confirmations (YOLO mode)
    #[arg(long)]
    yolo: bool,

    /// Config file path (not yet implemented)
    #[arg(short, long)]
    config: Option<PathBuf>,
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

    // Load configuration from environment variables
    let mut config = Config::from_env();

    // CLI arguments override environment variables
    if let Some(provider_str) = args.provider {
        if let Ok(provider) = provider_str.parse::<ModelProvider>() {
            config.provider = provider;
            // Reload provider-specific settings after changing provider
            config = reload_for_provider(config);
        }
    }

    if let Some(model) = args.model {
        config.model.model_id = model;
    }

    if let Some(endpoint) = args.endpoint {
        config.model.endpoint = endpoint;
    }

    if let Some(api_key) = args.api_key {
        config.model.api_key = api_key;
    }

    // Create data directory
    tokio::fs::create_dir_all(&config.data_dir).await?;

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

    // Create tool registry
    let tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(EditTool::new(&working_dir)));
    tool_registry.register(Arc::new(ReadTool::new(&working_dir)));

    let coordinator = Arc::new(Coordinator::new(
        storage,
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

    let session_config = SessionConfig {
        agent: agent_config,
        project_path: working_dir.clone(),
    };
    let session_id = coordinator.create_session(session_config).await?;

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

    // Run TUI with banner data
    run_tui(event_rx, input_tx, cancel_tx, working_dir_str, skill_names).await?;

    println!("Goodbye!");
    Ok(())
}

/// Reload provider-specific settings after provider change
fn reload_for_provider(mut config: Config) -> Config {
    let provider = config.provider;

    // Re-check provider-specific API key: YOMI_{PROVIDER}_API_KEY > {PROVIDER}_API_KEY
    config.model.api_key = std::env::var(provider.standard_api_key_env())
        .ok()
        .unwrap_or_else(|| config.model.api_key.clone());

    // Re-check provider-specific model: YOMI_{PROVIDER}_MODEL > {PROVIDER}_MODEL
    config.model.model_id = std::env::var(provider.standard_model_env())
        .ok()
        .unwrap_or_else(|| {
            if config.model.model_id.is_empty() {
                // Set default model for new provider
                match provider {
                    ModelProvider::OpenAI => "gpt-4".to_string(),
                    ModelProvider::Anthropic => "claude-3-5-sonnet-20241022".to_string(),
                }
            } else {
                config.model.model_id.clone()
            }
        });

    // Re-check provider-specific endpoint: YOMI_{PROVIDER}_ENDPOINT > {PROVIDER}_ENDPOINT
    config.model.endpoint = std::env::var(provider.standard_api_base_env())
        .ok()
        .unwrap_or_else(|| config.model.endpoint.clone());

    config
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

use anyhow::Result;
use clap::Parser;
use nekoclaw_adapters::{BashTool, FileTool, OpenAIProvider, SqliteStorage};
use nekoclaw_app::{Coordinator, SessionConfig};
use nekoclaw_core::{
    agent::AgentConfig,
    bus::EventBus,
    storage::StorageConfig,
    tool::{ToolRegistry, ToolSandbox, enable_yolo_mode},
};
use nekoclaw_tui::App;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "nekoclaw")]
#[command(about = "AI coding assistant CLI")]
struct Args {
    #[arg(short, long)]
    directory: Option<PathBuf>,
    #[arg(short, long)]
    model: Option<String>,
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long)]
    sandbox: bool,
    #[arg(long)]
    yolo: bool,
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    if args.yolo {
        enable_yolo_mode();
        tracing::warn!("YOLO mode enabled - all confirmations skipped!");
    }

    let working_dir = args.directory
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let working_dir = working_dir.canonicalize()?;

    let mut agent_config = AgentConfig::default();
    if let Some(model) = args.model {
        agent_config.model.model_id = model;
    }
    if let Some(endpoint) = args.endpoint {
        agent_config.model.endpoint = endpoint;
    }
    if let Some(api_key) = args.api_key {
        agent_config.model.api_key = api_key;
    } else if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        agent_config.model.api_key = key;
    }

    let data_dir = directories::ProjectDirs::from("ai", "nekoclaw", "nekoclaw")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("~/.nekoclaw"));
    tokio::fs::create_dir_all(&data_dir).await?;

    let storage_config = StorageConfig {
        url: data_dir.join("sessions.db").to_string_lossy().to_string(),
        compaction_threshold: 100,
    };
    let storage = Arc::new(SqliteStorage::new(&storage_config).await?);
    let provider = Arc::new(OpenAIProvider::new()?);

    let mut tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(BashTool::new(&working_dir)));
    tool_registry.register(Arc::new(FileTool::new(&working_dir)));

    let sandbox = if args.sandbox {
        ToolSandbox::new().enable()
    } else {
        ToolSandbox::default()
    };

    let event_bus = EventBus::new(1000);

    let coordinator = Coordinator::new(
        event_bus.clone(),
        storage,
        provider,
        tool_registry,
        sandbox,
    );

    let session_config = SessionConfig {
        agent: agent_config,
        project_path: working_dir.clone(),
    };
    let session_id = coordinator.create_session(session_config).await?;

    println!("Nekoclaw session started: {}", session_id.0);
    println!("Working directory: {}", working_dir.display());
    println!("Starting TUI...\n");

    // Create channel for input forwarding
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(100);

    // Spawn task to forward input to coordinator
    let coord_for_input = Arc::new(coordinator);
    let session_id_for_input = session_id.clone();
    tokio::spawn(async move {
        while let Some(content) = input_rx.recv().await {
            if let Err(e) = coord_for_input.send_message(&session_id_for_input, content).await {
                tracing::error!("Failed to send message: {}", e);
            }
        }
    });

    // Run TUI
    let mut app = App::new(&event_bus, input_tx);
    app.run().await?;

    println!("Goodbye!");
    Ok(())
}

use crate::{
    args::GlobalArgs,
    misc::claude_settings::ClaudeSettings,
    session::{resolve_session, run_session_loop, SessionArg, SessionContext},
    storage::AppStorage,
    utils::resolve_skill_folders,
};
use anyhow::{Context, Result};
use kernel::{
    agent::AgentConfig,
    compactor,
    config::{env_names, Config, ModelProvider},
    expand_tilde,
    misc::plugin::PluginLoader,
    permissions::Level,
    skill::SkillLoader,
    storage::{FsStorage, Storage},
    utils::strs,
    AnthropicProvider, Coordinator, OpenAIProvider, SessionConfig, TaskStore,
};
use sqlx::sqlite::SqlitePoolOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Create `SQLite` pool with proper connection string and PRAGMAs
async fn create_db_pool(db_path: &Path) -> anyhow::Result<sqlx::SqlitePool> {
    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Create empty file if it doesn't exist (sqlx requirement)
    if !db_path.exists() {
        tokio::fs::File::create(db_path).await?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", db_path.display()))
        .await?;

    // Set busy timeout (5 seconds)
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&pool)
        .await?;

    Ok(pool)
}

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
}

pub async fn run(args: TuiArgs) -> Result<()> {
    let working_dir = args
        .global
        .dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let working_dir = working_dir.canonicalize()?;

    let mut config = crate::utils::load_config(args.global.config.as_ref())?;

    if args.yolo {
        config.auto_approve = Level::Dangerous;
        tracing::warn!("YOLO mode enabled - all confirmations skipped!");
    }

    tokio::fs::create_dir_all(&config.data_dir).await?;

    let app_storage = Arc::new(AppStorage::new(config.data_dir.clone())?);
    init_logging(&config)?;

    let skills = load_skills(&config).await;

    // Create SQLite pool for session metadata
    let db_path = config.data_dir.join("yomi.db");
    let pool = create_db_pool(&db_path).await?;
    let storage = Arc::new(FsStorage::new(config.data_dir.join("sessions"), pool).await?);
    let provider = create_provider(&config)?;
    let task_store = Arc::new(TaskStore::new(&config.data_dir).await?);
    let project_memory = kernel::project_memory::load(&working_dir).await?;

    let coordinator = Arc::new(Coordinator::new(
        storage.clone(),
        provider,
        config.model.clone(),
        Some(task_store),
        project_memory,
        Some(compactor::Compactor::default()),
        get_skill_folders(&config),
    ));

    let skill_names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();

    let mk_agent_config = || AgentConfig {
        model: config.model.clone(),
        skills: skills.clone(),
        ..config.agent.clone()
    };

    let context_window = mk_agent_config().compactor.context_window;

    let mk_config = || SessionConfig {
        agent: mk_agent_config(),
        project_path: working_dir.clone(),
        auto_approve_level: config.auto_approve,
    };

    print_startup_info(&config);

    let session_ctx = SessionContext {
        working_dir: working_dir.clone(),
        skill_names: skill_names.clone(),
        auto_approve: config.auto_approve,
        context_window,
    };

    let mut is_first_session = true;
    let mut input_history = app_storage
        .load_input_history(&working_dir)
        .await
        .unwrap_or_default();

    let session_arg = match args.resume {
        Some(None) => SessionArg::Last,
        Some(Some(id)) => SessionArg::Specific(id),
        None => SessionArg::New,
    };

    loop {
        let session_id = resolve_session(
            &session_arg,
            is_first_session,
            &coordinator,
            &app_storage,
            &working_dir,
            mk_config,
        )
        .await?;

        let session_messages = storage.get_messages(&session_id).await.unwrap_or_default();

        let result = run_session_loop(
            coordinator.clone(),
            session_id,
            session_ctx.clone(),
            app_storage.clone(),
            input_history.clone(),
            session_messages,
            is_first_session,
            args.prompt.clone(),
        )
        .await?;

        for entry in &result.new_history_entries {
            app_storage.add_input_entry(&working_dir, entry).await?;
        }
        input_history.extend(result.new_history_entries);

        if result.should_create_new_session {
            is_first_session = false;
            continue;
        }

        break;
    }

    Ok(())
}

fn get_skill_folders(config: &Config) -> Vec<PathBuf> {
    resolve_skill_folders(&config.skill_folders)
}

async fn load_skills(config: &Config) -> Vec<Arc<kernel::skill::Skill>> {
    let skill_folders = get_skill_folders(config);

    tracing::debug!("Loading skills from folders: {:?}", skill_folders);

    let mut skills = {
        let loader = SkillLoader::new(skill_folders.clone());
        loader.load_all().unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load skills: {e}");
            Vec::new()
        })
    };

    if config.load_claude_plugins {
        load_plugin_skills(config, &mut skills).await;
    }

    deduplicate_skills(&mut skills);

    if !skills.is_empty() {
        tracing::info!("Loaded {} skill(s)", skills.len());
        for skill in &skills {
            tracing::debug!("  - {} (from {})", skill.name, skill.source_path.display());
        }
    }

    skills
}

async fn load_plugin_skills(config: &Config, skills: &mut Vec<Arc<kernel::skill::Skill>>) {
    let plugin_dirs = if config.plugin_dirs.is_empty() {
        vec![expand_tilde("~/.claude/plugins/cache")]
    } else {
        config.plugin_dirs.clone()
    };

    tracing::debug!("Loading plugins from directories: {:?}", plugin_dirs);

    let claude_settings = ClaudeSettings::load();
    let has_enabled_filter = !claude_settings.enabled_plugins.is_empty();

    let plugins = {
        let loader =
            PluginLoader::new(plugin_dirs).with_enabled_plugins(claude_settings.enabled_plugins);
        loader.load_all().unwrap_or_else(|e| {
            tracing::warn!("Failed to load plugins: {e}");
            Vec::new()
        })
    };

    if has_enabled_filter {
        tracing::debug!("Applied enabledPlugins filter from ~/.claude/settings.json");
    }

    if !plugins.is_empty() {
        tracing::debug!("Loaded {} plugin(s)", plugins.len());
        for plugin in &plugins {
            tracing::debug!("  - {} (from {})", plugin.name, plugin.path.display());
            if let Ok(plugin_skills) = SkillLoader::load_from_plugin(plugin) {
                for skill in plugin_skills {
                    tracing::debug!("    - skill: {}", skill.name);
                    skills.push(skill);
                }
            }
        }
    }
}

fn deduplicate_skills(skills: &mut Vec<Arc<kernel::skill::Skill>>) {
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
}

fn create_provider(config: &Config) -> Result<Arc<dyn kernel::Provider>> {
    if !config.has_api_key() {
        eprintln!("Error: API key not configured.");
        std::process::exit(1);
    }

    let provider: Arc<dyn kernel::Provider> = match config.provider {
        ModelProvider::OpenAI => Arc::new(OpenAIProvider::new()?),
        ModelProvider::Anthropic => Arc::new(AnthropicProvider::new()?),
    };

    Ok(provider)
}

fn print_startup_info(config: &Config) {
    println!("Provider: {}", config.provider);
    println!("Model: {}", config.model.model_id);
    println!("Endpoint: {}", config.model.endpoint);
    let api_key = config.api_key();
    let key_preview = if api_key.len() > 8 {
        strs::truncate_with_suffix(api_key, 11, "...")
    } else {
        "not set".to_string()
    };
    println!("API Key: {key_preview}\n");
}

fn init_logging(config: &Config) -> Result<()> {
    let log_dir = std::env::var(env_names::LOG_DIR)
        .map_or_else(|_| config.data_dir.join("logs"), PathBuf::from);

    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create log directory: {}", log_dir.display()))?;

    let log_path = log_dir.join("app.log");
    let file_appender = tracing_rolling_file::RollingFileAppenderBase::builder()
        .filename(log_path.to_string_lossy().to_string())
        .condition_max_file_size(10 * 1024 * 1024)
        .max_filecount(5)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to create rolling file appender: {e}"))?;

    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    Box::leak(Box::new(_guard));

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
    }

    Ok(())
}

use crate::agent::AgentConfig;
use crate::permission::Level;
use crate::provider::ModelConfig;
use crate::types::KernelError;
use crate::utils::env::{env_bool_opt, env_parse, env_var, parse_number_with_unit};
use crate::utils::path::{default_skill_folders, expand_tilde, DEFAULT_DATA_DIR};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

static INJECTED_ENV: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();

/// Environment variable names (for easy reference and IDE completion)
pub mod env_names {
    /// Provider selection
    pub const PROVIDER: &str = env_name!("PROVIDER");

    /// Generic API settings
    pub const API_KEY: &str = env_name!("API_KEY");
    pub const MODEL: &str = env_name!("MODEL");
    pub const API_BASE: &str = env_name!("API_BASE");
    pub const MAX_TOKENS: &str = env_name!("MAX_TOKENS");
    pub const TEMPERATURE: &str = env_name!("TEMPERATURE");

    /// Application settings
    pub const DATA_DIR: &str = env_name!("DATA_DIR");
    pub const MAX_ITERATIONS: &str = env_name!("MAX_ITERATIONS");
    pub const ENABLE_SUB_AGENTS: &str = env_name!("ENABLE_SUB_AGENTS");

    /// Thinking configuration
    pub const THINKING: &str = env_name!("THINKING");
    pub const THINKING_BUDGET: &str = env_name!("THINKING_BUDGET");
    /// Reasoning effort for `OpenAI` o1/o3 models (low/medium/high)
    pub const THINKING_EFFORT: &str = env_name!("THINKING_EFFORT");

    /// Logging configuration
    pub const LOG_DIR: &str = env_name!("LOG_DIR");

    /// Skill folders (comma-separated paths)
    pub const SKILL_FOLDERS: &str = env_name!("SKILL_FOLDERS");

    /// Auto-approve level for tool permissions (safe | caution | dangerous)
    pub const AUTO_APPROVE: &str = env_name!("AUTO_APPROVE");

    /// Context window size for the model (e.g., 131072, 200000, 128k, 200k)
    pub const CONTEXT_WINDOW: &str = env_name!("CONTEXT_WINDOW");
    /// Default model name to activate for new sessions
    pub const DEFAULT_MODEL: &str = env_name!("DEFAULT_MODEL");
    /// Compaction threshold as a ratio of the context window (0.0–1.0, default: 0.8)
    pub const COMPACTOR_RATIO: &str = env_name!("COMPACTOR_RATIO");
    /// Maximum number of checkpoints to retain per session (default: 5)
    pub const MAX_CHECKPOINTS: &str = env_name!("MAX_CHECKPOINTS");
    /// Sessions are retained for this many days before gc collects them (default: 90)
    pub const GC_RETENTION_DAYS: &str = env_name!("GC_RETENTION_DAYS");
    /// Run gc automatically in the daemon (default: false)
    pub const GC_AUTO: &str = env_name!("GC_AUTO");
    /// Tool blocklist (comma-separated regex patterns)
    pub const TOOL_BLOCKLIST: &str = env_name!("TOOL_BLOCKLIST");
    /// Maximum tool output length in bytes (default `40_000`)
    pub const MAX_TOOL_OUTPUT_LENGTH: &str = env_name!("MAX_TOOL_OUTPUT_LENGTH");
    /// Path to a configuration file to use instead of the default
    pub const CONFIG: &str = env_name!("CONFIG");

    // NOTE: General-purpose environment variables injected at startup, overriding host values. Keys are used verbatim and do not require the [`crate::ENV_PREFIX`] prefix.

    /// Serper.dev API key (optional, no prefix)
    pub const SERPER_API_KEY: &str = "SERPER_API_KEY";
    /// Brave Search API key (optional, no prefix)
    pub const BRAVE_API_KEY: &str = "BRAVE_API_KEY";
    /// `SearXNG` instance base URL (optional, no prefix)
    pub const SEARXNG_URL: &str = "SEARXNG_URL";
    /// Kimi Search API key (optional, no prefix)
    pub const KIMI_AGENT_API_KEY: &str = "KIMI_AGENT_API_KEY";
    /// Kimi Search endpoint override (optional, no prefix). Defaults to the built-in endpoint if unset.
    pub const KIMI_SEARCH_ENDPOINT: &str = "KIMI_SEARCH_ENDPOINT";
    pub const LOG_LEVEL: &str = "RUST_LOG"; // Standard env var, no prefix
}

fn validate_env_entry(name: &str, value: &str) -> std::result::Result<(), KernelError> {
    if name.is_empty() || name.contains(['=', '\0']) {
        return Err(KernelError::config(format!(
            "Invalid environment variable name: {name:?}"
        )));
    }
    if value.contains('\0') {
        return Err(KernelError::config(format!(
            "Environment variable {name:?} contains a NUL byte"
        )));
    }
    Ok(())
}

/// Provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelProvider {
    #[default]
    OpenAI,
    Anthropic,
    /// `OpenAI` Responses API (`/v1/responses`), for GPT-5 / o-series reasoning models
    #[serde(rename = "openai_response")]
    OpenAIResponse,
}

impl ModelProvider {
    // Standard non-prefixed provider-specific env vars removed.
    // Only YOMI_* prefixed variables are supported.
}

impl std::str::FromStr for ModelProvider {
    type Err = String;

    #[inline]
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // Fast path: lowercase comparison without allocation
        match s.as_bytes() {
            b"openai" | b"OPENAI" | b"OpenAI" => Ok(Self::OpenAI),
            b"anthropic" | b"ANTHROPIC" | b"Anthropic" => Ok(Self::Anthropic),
            b"openai_response" | b"openai-response" => Ok(Self::OpenAIResponse),
            _ => {
                // Slow path: lowercase and compare
                match s.to_lowercase().as_str() {
                    "openai" => Ok(Self::OpenAI),
                    "anthropic" => Ok(Self::Anthropic),
                    "openai_response" | "openai-response" => Ok(Self::OpenAIResponse),
                    _ => Err(format!("Unknown provider: {s}")),
                }
            }
        }
    }
}

impl std::fmt::Display for ModelProvider {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenAI => f.write_str("openai"),
            Self::Anthropic => f.write_str("anthropic"),
            Self::OpenAIResponse => f.write_str("openai_response"),
        }
    }
}

/// Feature flags for experimental capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FeaturesConfig {
    /// Enable all features unless a feature is explicitly overridden.
    pub all: bool,
    /// Generate a session title with a model after receiving a user message.
    pub update_session_title: Option<bool>,
    /// Expose the cron tool to agents (create/list/update/delete/trigger jobs).
    pub cron_tool: Option<bool>,
    /// Expose the todo tool to agents (task list tracking).
    pub todo_tool: Option<bool>,
    /// Teach agents the `<yomi_attachments>` syntax for attaching files to
    /// replies (channels deliver the files, the app shows clickable items).
    /// Unlike the experimental flags above this is **on by default** and
    /// ignores `all`; set `false` to disable. Attachments already declared
    /// in history still surface when disabled — the flag only gates what
    /// agents are taught.
    pub attachments: Option<bool>,
}

impl FeaturesConfig {
    #[must_use]
    pub fn update_session_title_enabled(&self) -> bool {
        self.update_session_title.unwrap_or(self.all)
    }

    #[must_use]
    pub fn cron_tool_enabled(&self) -> bool {
        self.cron_tool.unwrap_or(self.all)
    }

    #[must_use]
    pub fn todo_tool_enabled(&self) -> bool {
        self.todo_tool.unwrap_or(self.all)
    }

    #[must_use]
    pub fn attachments_enabled(&self) -> bool {
        self.attachments.unwrap_or(true)
    }
}

/// Configuration for lightweight model-backed tasks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TasksConfig {
    /// Model key used for lightweight tasks. Falls back to the session model when absent.
    pub fast_model: Option<String>,
}

/// Garbage-collection policy for expired session resources (`[gc]` section).
///
/// These are *policy* settings: what to collect and, for the daemon, how
/// often. Whether a run actually deletes (`dry_run`) is a per-invocation
/// flag and deliberately not configurable here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct GcConfig {
    /// Sessions are retained for this many days after their last update
    /// before gc collects them (min 1)
    pub retention_days: i64,
    /// Skip pinned sessions
    pub keep_pinned: bool,
    /// Sweep orphan files whose session no longer exists in the DB
    pub sweep_orphans: bool,
    /// Run `VACUUM` + WAL truncate after deletion
    pub vacuum: bool,
    /// Run gc automatically in the daemon (opt-in; performs real deletions).
    /// Runs once at startup and then every day at local midnight.
    pub auto: bool,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            retention_days: 90,
            keep_pinned: true,
            sweep_orphans: true,
            vacuum: false,
            auto: false,
        }
    }
}

/// Editable kernel configuration and its effective startup representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelConfig {
    pub content: String,
    pub path: String,
    pub full_config: String,
}

/// Complete yomi configuration from environment
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent: AgentConfig,
    /// General-purpose environment variables injected at startup, overriding
    /// host values.
    /// Keys are used verbatim and do not require the [`crate::ENV_PREFIX`] prefix.
    pub env: BTreeMap<String, String>,
    pub tasks: TasksConfig,
    pub auto_approve: Level,
    pub data_dir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_folders: Option<Vec<String>>,
    /// Experimental feature flags
    pub features: FeaturesConfig,
    /// Maximum number of checkpoints to retain per session (default: 5)
    pub max_checkpoints: usize,
    /// Garbage-collection policy for expired session resources
    pub gc: GcConfig,
    /// External platform channels (Telegram, Feishu, etc.)
    #[serde(default)]
    pub channels: Vec<crate::channels::ChannelConfig>,
    /// Multi-model configuration array (TOML: `[[models]]`), at least one element
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    /// wire 外部扩展（TOML: `[[extensions]]`）：跟随 daemon 拉起的扩展进程。
    #[serde(default)]
    pub extensions: Vec<ExtensionConfig>,
}

/// `[[extensions]]` 条目：列出即拉起，daemon 死则组杀，崩溃固定退避重拉。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtensionConfig {
    pub name: String,
    /// 命令行（argv[0] 为可执行文件）。
    pub command: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = expand_tilde(DEFAULT_DATA_DIR);
        Self {
            agent: AgentConfig::default(),
            env: BTreeMap::new(),
            tasks: TasksConfig::default(),
            auto_approve: Level::default(),
            data_dir,
            log_dir: None,
            skill_folders: None,
            features: FeaturesConfig::default(),
            max_checkpoints: 5,
            gc: GcConfig::default(),
            channels: Vec::new(),
            models: vec![ModelConfig::default()],
            extensions: Vec::new(),
        }
    }
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();
        config.load_from_env();
        config
    }

    /// Load configuration from file (without env overrides).
    /// Call `apply_env_overrides()` after this if needed.
    pub fn from_file(path: &PathBuf) -> std::result::Result<Self, KernelError> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    /// Discover the first existing config file in standard locations.
    ///
    /// Search order:
    /// 1. `YOMI_CONFIG` environment variable
    /// 2. `~/.yomi/config.toml`
    pub fn discover_file() -> Option<PathBuf> {
        if let Some(config_path) = env_var(env_names::CONFIG).map(PathBuf::from) {
            if config_path.exists() {
                return Some(config_path);
            }
        }
        let default_path = expand_tilde(DEFAULT_DATA_DIR).join("config.toml");
        default_path.exists().then_some(default_path)
    }

    /// Path used for config reads and writes, including a non-existent target.
    pub fn write_path() -> PathBuf {
        env_var(env_names::CONFIG).map_or_else(
            || expand_tilde(DEFAULT_DATA_DIR).join("config.toml"),
            PathBuf::from,
        )
    }

    /// Read the editable config plus the effective startup config.
    pub fn get_kernel_config() -> std::result::Result<KernelConfig, KernelError> {
        Self::get_kernel_config_from(&Self::write_path())
    }

    pub fn get_kernel_config_from(path: &Path) -> std::result::Result<KernelConfig, KernelError> {
        let content = if path.exists() {
            std::fs::read_to_string(path)?
        } else {
            String::new()
        };

        let full_config = match parse_effective_config(&content) {
            Ok(effective) => toml::to_string_pretty(&redact_effective_config(effective))
                .map_err(|e| KernelError::serde(format!("Failed to serialize config: {e}")))?,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "editable config is invalid");
                String::new()
            }
        };

        Ok(KernelConfig {
            content,
            path: path.to_string_lossy().into_owned(),
            full_config,
        })
    }

    /// Validate and atomically replace the editable config file.
    pub fn set_kernel_config(content: &str) -> std::result::Result<(), KernelError> {
        Self::set_kernel_config_at(&Self::write_path(), content)
    }

    pub fn set_kernel_config_at(
        path: &Path,
        content: &str,
    ) -> std::result::Result<(), KernelError> {
        validate_editable_config(content)?;

        atomic_write_config(path, content)?;
        Ok(())
    }

    fn validate_env_entries(&self) -> std::result::Result<(), KernelError> {
        for (name, value) in &self.env {
            validate_env_entry(name, value)?;
        }
        Ok(())
    }

    /// Inject configured environment variables into the host process.
    ///
    /// Configured values override any existing host variables, and values
    /// previously injected by Yomi are updated when the configuration
    /// changes. Overriding is one-way: the previous host value cannot be
    /// restored. Removed `[env]` entries keep their last injected value in
    /// this process until it exits; daemon children explicitly remove
    /// tracked injected variables before starting so they reload the
    /// current configuration.
    pub fn inject_env(&self) -> std::result::Result<(), KernelError> {
        self.validate_env_entries()?;
        let mut injected = INJECTED_ENV
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|error| KernelError::config(format!("injected env lock poisoned: {error}")))?;
        for (name, value) in &self.env {
            std::env::set_var(name, value);
            injected.insert(name.clone(), value.clone());
        }
        Ok(())
    }

    /// Names of environment values previously installed by `inject_env`.
    /// Daemon process spawners remove these from the child environment so the
    /// replacement reloads the current `[env]` section itself.
    pub fn injected_env_names() -> Vec<String> {
        INJECTED_ENV
            .get()
            .and_then(|injected| injected.lock().ok())
            .map(|injected| injected.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Remove environment values previously installed by `inject_env`.
    /// This is used before restarting an in-process daemon so the replacement
    /// reloads `[env]` values from the saved config.
    pub fn clear_injected_env() {
        let Some(injected) = INJECTED_ENV.get() else {
            return;
        };
        let Ok(mut injected) = injected.lock() else {
            return;
        };
        for (name, value) in std::mem::take(&mut *injected) {
            if std::env::var(&name).ok().as_deref() == Some(value.as_str()) {
                std::env::remove_var(name);
            }
        }
    }

    /// Apply environment variable overrides to this config
    pub fn apply_env_overrides(&mut self) {
        self.load_from_env();
    }

    /// Get model configuration for the current default model.
    /// Returns `None` if `finalize()` has not been called or the default model is missing.
    #[inline]
    pub fn model(&self) -> Option<&ModelConfig> {
        self.models
            .iter()
            .find(|m| m.name == self.agent.default_model)
    }

    /// Finalize configuration by computing and filling in default values.
    /// Call this after all configuration sources are loaded.
    pub fn finalize(&mut self) {
        // Expand ~ in data_dir if not already done
        self.data_dir = expand_tilde(self.data_dir.to_string_lossy());

        // Fill log_dir default if not set
        if self.log_dir.is_none() {
            self.log_dir = Some(self.data_dir.join("logs"));
        }

        // Fill skill_folders default if not set
        if self.skill_folders.is_none() {
            self.skill_folders = Some(
                default_skill_folders(&self.data_dir)
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
            );
        }

        // Ensure models array is non-empty (use default model if empty)
        if self.models.is_empty() {
            self.models.push(ModelConfig::default());
        }

        // Ensure default_model points to a valid model in the array
        if !self
            .models
            .iter()
            .any(|m| m.name == self.agent.default_model)
        {
            self.agent.default_model = self.models[0].name.clone();
        }
    }

    /// Validate configuration after file and environment overrides are applied.
    pub fn validate(&self) -> std::result::Result<(), KernelError> {
        let compactor = &self.agent.compactor;
        if !(compactor.threshold_ratio.is_finite()
            && 0.0 < compactor.threshold_ratio
            && compactor.threshold_ratio <= 1.0)
        {
            return Err(KernelError::config(format!(
                "agent.compactor.threshold_ratio must be finite and in (0, 1], got {}",
                compactor.threshold_ratio
            )));
        }
        if compactor.summary_max_tokens == 0 {
            return Err(KernelError::config(
                "agent.compactor.summary_max_tokens must be greater than 0",
            ));
        }

        if self.gc.retention_days < 1 {
            return Err(KernelError::config(format!(
                "gc.retention_days must be at least 1, got {}",
                self.gc.retention_days
            )));
        }

        for model in &self.models {
            if model.context_window == 0 {
                return Err(KernelError::config(format!(
                    "models.{}.context_window must be greater than 0",
                    model.name
                )));
            }
            if model.max_tokens == Some(0) {
                return Err(KernelError::config(format!(
                    "models.{}.max_tokens must be greater than 0 when set",
                    model.name
                )));
            }
        }
        Ok(())
    }

    /// Get the log directory (defaults to `data_dir/logs`)
    pub fn log_dir(&self) -> PathBuf {
        self.log_dir
            .clone()
            .unwrap_or_else(|| self.data_dir.join("logs"))
    }

    /// Get the skill folders.
    ///
    /// # Panics
    /// Panics if `finalize` was not called (`skill_folders` is `None`).
    pub fn skill_folders(&self) -> &[String] {
        self.skill_folders
            .as_ref()
            .expect("Config::finalize must be called before using skill_folders")
    }

    /// Internal: Load all environment variables into config
    fn load_from_env(&mut self) {
        // Ensure models is non-empty before accessing entries
        if self.models.is_empty() {
            tracing::warn!(
                "Config `models` array is empty — adding a default model. \
                 Please update your config.toml to use the `[[models]]` array format."
            );
            self.models.push(ModelConfig::default());
        }

        // Default model name override — applied first so the single-model env vars
        // below target the correct entry.
        if let Some(key) = env_var(env_names::DEFAULT_MODEL) {
            self.agent.default_model = key;
        }

        // Model-related env vars are collected into a new `from_env` entry
        // instead of mutating an existing model in the array.
        let mut env_model = ModelConfig {
            name: "from_env".to_string(),
            ..ModelConfig::default()
        };
        let mut has_env_model = false;

        // Provider selection (may affect subsequent provider-specific lookups)
        if let Some(provider) = env_var(env_names::PROVIDER) {
            if let Ok(p) = provider.parse() {
                env_model.provider = p;
                has_env_model = true;
            }
        }

        // API Key: only YOMI_ prefixed generic variable is supported
        if let Some(key) = env_var(env_names::API_KEY) {
            env_model.api_key = key;
            has_env_model = true;
        }

        // Model: only YOMI_ prefixed generic variable is supported
        if let Some(model) = env_var(env_names::MODEL) {
            env_model.model_id = model;
            has_env_model = true;
        }

        // Endpoint: only YOMI_ prefixed generic variable is supported
        if let Some(endpoint) = env_var(env_names::API_BASE) {
            env_model.endpoint = endpoint;
            has_env_model = true;
        }

        // Numeric settings
        // Max tokens (supports formats like "4096", "4k", "8k")
        if let Some(max_tokens) = env_var(env_names::MAX_TOKENS) {
            if let Some(tokens) = parse_number_with_unit(&max_tokens) {
                env_model.max_tokens = Some(tokens);
                has_env_model = true;
            }
        }
        if let Some(temp) = env_parse::<f32>(env_names::TEMPERATURE) {
            env_model.temperature = Some(temp);
            has_env_model = true;
        }

        // Boolean / thinking settings
        if let Some(budget) = env_parse::<u32>(env_names::THINKING_BUDGET) {
            env_model.thinking.budget_tokens = budget;
            has_env_model = true;
        }
        if let Some(enabled) = env_bool_opt(env_names::THINKING) {
            env_model.thinking.enabled = enabled;
            has_env_model = true;
        }
        if let Some(effort) = env_var(env_names::THINKING_EFFORT) {
            env_model.thinking.effort = Some(effort);
            has_env_model = true;
        }

        // Context window size (supports formats like "131072", "128k", "200k", "200000")
        if let Some(context_window) = env_var(env_names::CONTEXT_WINDOW) {
            if let Some(tokens) = parse_number_with_unit(&context_window) {
                env_model.context_window = tokens;
                has_env_model = true;
            }
        }

        // Only push the env-derived model when at least one relevant env var was set.
        if has_env_model {
            // Replace any pre-existing entry with the same name to avoid duplicates.
            self.models.retain(|m| m.name != "from_env");
            self.models.push(env_model);
        }

        // Non-model agent / system settings

        if let Some(iters) = env_parse::<usize>(env_names::MAX_ITERATIONS) {
            self.agent.max_iterations = iters;
        }

        // Enable sub-agents (default true unless explicitly set to "false")
        if let Some(val) = env_var(env_names::ENABLE_SUB_AGENTS) {
            self.agent.enable_subagent = val != "false";
        }

        // Data directory (expands ~ to home)
        if let Some(dir) = env_var(env_names::DATA_DIR) {
            self.data_dir = expand_tilde(dir);
        }

        // Log directory (expands ~ to home, defaults to data_dir/logs)
        if let Some(dir) = env_var(env_names::LOG_DIR) {
            self.log_dir = Some(expand_tilde(dir));
        }

        // Skill folders (comma-separated)
        if let Some(folders) = env_var(env_names::SKILL_FOLDERS) {
            self.skill_folders = Some(folders.split(',').map(String::from).collect());
        }

        // Auto-approve level (safe | caution | dangerous)
        if let Some(level) = env_var(env_names::AUTO_APPROVE) {
            if let Ok(l) = Level::from_str(&level) {
                self.auto_approve = l;
            }
        }

        // Compactor threshold ratio; validated after all overrides are applied.
        if let Some(ratio) = env_parse::<f32>(env_names::COMPACTOR_RATIO) {
            self.agent.compactor.threshold_ratio = ratio;
        }

        // Maximum checkpoints per session
        if let Some(max) = env_parse::<usize>(env_names::MAX_CHECKPOINTS) {
            self.max_checkpoints = max;
        }

        // GC policy
        if let Some(days) = env_parse::<i64>(env_names::GC_RETENTION_DAYS) {
            self.gc.retention_days = days;
        }
        if let Some(auto) = env_bool_opt(env_names::GC_AUTO) {
            self.gc.auto = auto;
        }

        // Tool blocklist (comma-separated regex patterns)
        if let Some(list) = env_var(env_names::TOOL_BLOCKLIST) {
            self.agent.tool_blocklist = list
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        // Maximum tool output length
        if let Some(max_len) = env_parse::<usize>(env_names::MAX_TOOL_OUTPUT_LENGTH) {
            self.agent.max_tool_output_length = max_len;
        }
    }

    /// Get the API key for the current default model
    #[inline]
    pub fn api_key(&self) -> &str {
        self.model().map(|m| m.api_key.as_str()).unwrap_or_default()
    }

    /// Check if API key is configured for the default model
    #[inline]
    pub fn has_api_key(&self) -> bool {
        self.model().is_some_and(|m| !m.api_key.is_empty())
    }

    /// Set the data directory
    #[must_use]
    pub fn with_data_dir(mut self, data_dir: PathBuf) -> Self {
        self.data_dir = data_dir;
        self
    }
}

fn redact_effective_config(mut config: Config) -> Config {
    config.env.values_mut().for_each(|value| value.clear());
    for model in &mut config.models {
        model.api_key.clear();
        for (name, value) in &mut model.headers {
            let normalized = name.to_ascii_lowercase();
            if normalized.contains("authorization")
                || normalized.contains("api-key")
                || normalized.contains("api_key")
                || normalized.contains("token")
                || normalized.contains("secret")
            {
                value.clear();
            }
        }
    }
    for channel in &mut config.channels {
        match &mut channel.platform {
            crate::channels::PlatformConfig::Telegram { token } => token.clear(),
            crate::channels::PlatformConfig::Feishu { app_secret, .. } => app_secret.clear(),
        }
    }
    config
}

fn parse_effective_config(content: &str) -> std::result::Result<Config, KernelError> {
    let mut config = if content.is_empty() {
        Config::default()
    } else {
        toml::from_str(content).map_err(|e| KernelError::config(format!("Invalid TOML: {e}")))?
    };
    ensure_unique_models(&config)?;
    config.validate_env_entries()?;
    config.apply_env_overrides();
    config.finalize();
    config.validate()?;
    Ok(config)
}

fn ensure_unique_models(config: &Config) -> std::result::Result<(), KernelError> {
    let mut model_names = std::collections::HashSet::new();
    if config
        .models
        .iter()
        .any(|model| !model_names.insert(&model.name))
    {
        return Err(KernelError::config(
            "Invalid config: duplicate model name in [[models]]",
        ));
    }
    Ok(())
}

fn validate_editable_config(content: &str) -> std::result::Result<(), KernelError> {
    let config: Config =
        toml::from_str(content).map_err(|e| KernelError::config(format!("Invalid TOML: {e}")))?;

    ensure_unique_models(&config)?;
    config.validate_env_entries()?;

    if !config.models.is_empty()
        && !config
            .models
            .iter()
            .any(|model| model.name == config.agent.default_model)
    {
        return Err(KernelError::config(
            "Invalid config: agent.default_model must match a [[models]] name",
        ));
    }

    config.validate()
}

fn resolve_config_write_target(path: &Path) -> std::io::Result<PathBuf> {
    let mut target = path.to_path_buf();
    for _ in 0..40 {
        let metadata = match std::fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(target),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_symlink() {
            return Ok(target);
        }

        let link = std::fs::read_link(&target)?;
        target = if link.is_absolute() {
            link
        } else {
            target.parent().unwrap_or_else(|| Path::new(".")).join(link)
        };
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "too many symbolic links in config path",
    ))
}

fn atomic_write_config(path: &Path, content: &str) -> std::io::Result<()> {
    let path = resolve_config_write_target(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let existing_permissions = std::fs::metadata(&path)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(content.as_bytes())?;
    if let Some(permissions) = existing_permissions {
        temp_file.as_file().set_permissions(permissions)?;
    }
    temp_file.as_file().sync_all()?;
    temp_file.persist(&path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;

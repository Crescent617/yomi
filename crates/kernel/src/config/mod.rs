use crate::agent::AgentConfig;
use crate::permission::Level;
use crate::provider::ModelConfig;
use crate::types::KernelError;
use crate::utils::env::{env_bool_opt, env_first, env_parse, env_var, parse_number_with_unit};
use crate::utils::path::{default_skill_folders, expand_tilde, DEFAULT_DATA_DIR};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

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

    /// Standard non-prefixed provider-specific env vars
    pub const OPENAI_API_KEY: &str = "OPENAI_API_KEY";
    pub const ANTHROPIC_API_KEY: &str = "ANTHROPIC_API_KEY";
    pub const OPENAI_API_MODEL: &str = "OPENAI_API_MODEL";
    pub const ANTHROPIC_MODEL: &str = "ANTHROPIC_MODEL";
    pub const OPENAI_API_BASE: &str = "OPENAI_API_BASE";
    pub const ANTHROPIC_BASE_URL: &str = "ANTHROPIC_BASE_URL";

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
    /// Tool blocklist (comma-separated regex patterns)
    pub const TOOL_BLOCKLIST: &str = env_name!("TOOL_BLOCKLIST");
    /// Maximum tool output length in bytes (default `40_000`)
    pub const MAX_TOOL_OUTPUT_LENGTH: &str = env_name!("MAX_TOOL_OUTPUT_LENGTH");
    /// Path to a configuration file to use instead of the default
    pub const CONFIG: &str = env_name!("CONFIG");

    // NOTE: General-purpose environment variables injected at startup when absent from the host. Keys are used verbatim and do not require the [`crate::ENV_PREFIX`] prefix.

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
    /// Get the standard (non-prefixed) API key env var name
    #[inline]
    pub const fn standard_api_key_env(&self) -> &'static str {
        match self {
            Self::OpenAI | Self::OpenAIResponse => env_names::OPENAI_API_KEY,
            Self::Anthropic => env_names::ANTHROPIC_API_KEY,
        }
    }

    /// Get the standard (non-prefixed) model env var name
    #[inline]
    pub const fn standard_model_env(&self) -> &'static str {
        match self {
            Self::OpenAI | Self::OpenAIResponse => env_names::OPENAI_API_MODEL,
            Self::Anthropic => env_names::ANTHROPIC_MODEL,
        }
    }

    /// Get the standard (non-prefixed) API base env var name
    #[inline]
    pub const fn standard_api_base_env(&self) -> &'static str {
        match self {
            Self::OpenAI | Self::OpenAIResponse => env_names::OPENAI_API_BASE,
            Self::Anthropic => env_names::ANTHROPIC_BASE_URL,
        }
    }
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
}

impl FeaturesConfig {
    #[must_use]
    pub fn update_session_title_enabled(&self) -> bool {
        self.update_session_title.unwrap_or(self.all)
    }
}

/// Configuration for lightweight model-backed tasks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TasksConfig {
    /// Model key used for lightweight tasks. Falls back to the session model when absent.
    pub fast_model: Option<String>,
}

/// Complete yomi configuration from environment
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent: AgentConfig,
    /// General-purpose environment variables injected at startup when absent from the host.
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
    /// External platform channels (Telegram, Feishu, etc.)
    #[serde(default)]
    pub channels: Vec<crate::channels::ChannelConfig>,
    /// Multi-model configuration array (TOML: `[[models]]`), at least one element
    #[serde(default)]
    pub models: Vec<ModelConfig>,
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
            channels: Vec::new(),
            models: vec![ModelConfig::default()],
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
        if let Some(path) = env_var(env_names::CONFIG) {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
        let default = expand_tilde(DEFAULT_DATA_DIR).join("config.toml");
        if default.exists() {
            return Some(default);
        }
        None
    }

    /// Inject configured environment variables that are absent from the host process.
    ///
    /// Call this during startup, before applying environment overrides or spawning tasks.
    pub fn inject_env(&self) -> std::result::Result<(), KernelError> {
        for (name, value) in &self.env {
            validate_env_entry(name, value)?;
        }
        for (name, value) in &self.env {
            if std::env::var_os(name).is_none() {
                std::env::set_var(name, value);
            }
        }
        Ok(())
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

        // Single-model env vars apply to the entry named by `agent.default_model`,
        // falling back to models[0] when no entry matches.
        let default_idx = self
            .models
            .iter()
            .position(|m| m.name == self.agent.default_model)
            .unwrap_or(0);
        let default_model = &mut self.models[default_idx];

        // Provider selection (may affect subsequent provider-specific lookups)
        if let Some(provider) = env_var(env_names::PROVIDER) {
            if let Ok(p) = provider.parse() {
                default_model.provider = p;
            }
        }

        let provider = default_model.provider;

        // API Key: YOMI_ generic > provider-specific standard
        if let Some(key) = env_first(&[env_names::API_KEY, provider.standard_api_key_env()]) {
            default_model.api_key = key;
        }

        // Model: YOMI_ generic > provider-specific standard
        if let Some(model) = env_first(&[env_names::MODEL, provider.standard_model_env()]) {
            default_model.model_id = model;
        }

        // Endpoint: YOMI_ generic > provider-specific standard
        if let Some(endpoint) = env_first(&[env_names::API_BASE, provider.standard_api_base_env()])
        {
            default_model.endpoint = endpoint;
        }

        // Numeric settings
        // Max tokens (supports formats like "4096", "4k", "8k")
        if let Some(max_tokens) = env_var(env_names::MAX_TOKENS) {
            if let Some(tokens) = parse_number_with_unit(&max_tokens) {
                default_model.max_tokens = Some(tokens);
            }
        }
        if let Some(temp) = env_parse::<f32>(env_names::TEMPERATURE) {
            default_model.temperature = Some(temp);
        }
        if let Some(iters) = env_parse::<usize>(env_names::MAX_ITERATIONS) {
            self.agent.max_iterations = iters;
        }
        if let Some(budget) = env_parse::<u32>(env_names::THINKING_BUDGET) {
            default_model.thinking.budget_tokens = budget;
        }

        // Boolean settings
        if let Some(enabled) = env_bool_opt(env_names::THINKING) {
            default_model.thinking.enabled = enabled;
        }
        if let Some(effort) = env_var(env_names::THINKING_EFFORT) {
            default_model.thinking.effort = Some(effort);
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

        // Context window size (supports formats like "131072", "128k", "200k", "200000")
        if let Some(context_window) = env_var(env_names::CONTEXT_WINDOW) {
            if let Some(tokens) = parse_number_with_unit(&context_window) {
                default_model.context_window = tokens;
            }
        }

        // Compactor threshold ratio (0.0–1.0, default 0.8)
        if let Some(ratio) = env_parse::<f32>(env_names::COMPACTOR_RATIO) {
            self.agent.compactor.threshold_ratio = ratio.clamp(0.0, 1.0);
        }

        // Maximum checkpoints per session
        if let Some(max) = env_parse::<usize>(env_names::MAX_CHECKPOINTS) {
            self.max_checkpoints = max;
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

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;

use crate::agent::AgentConfig;
use crate::permissions::Level;
use crate::providers::ModelConfig;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use thiserror::Error;

/// Configuration error type
#[derive(Error, Debug, Clone)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    ReadError(String),
    #[error("Failed to parse config file: {0}")]
    ParseError(String),
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::ReadError(e.to_string())
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        Self::ParseError(e.to_string())
    }
}

/// Expand `~` to the user's home directory
pub fn expand_tilde(path: impl AsRef<str>) -> PathBuf {
    let path = path.as_ref();
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            return PathBuf::from(home).join(stripped);
        }
    }
    PathBuf::from(path)
}

/// Default data directory path
pub const DEFAULT_DATA_DIR: &str = "~/.yomi";

/// Generate default skill folders based on `working_dir` and `data_dir`
pub fn default_skill_folders(
    working_dir: &std::path::Path,
    data_dir: &std::path::Path,
) -> Vec<PathBuf> {
    vec![
        working_dir.join(".agents/skills"),
        data_dir.join("skills"),
        expand_tilde("~/.agents/skills"),
        expand_tilde("~/.claude/skills"),
    ]
}

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
    pub const YOLO: &str = env_name!("YOLO");
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
    pub const LOG_LEVEL: &str = "RUST_LOG"; // Standard env var, no prefix

    /// Skill folders (comma-separated paths)
    pub const SKILL_FOLDERS: &str = env_name!("SKILL_FOLDERS");

    /// Plugin directories to load skills from (colon-separated paths)
    pub const PLUGIN_DIRS: &str = env_name!("PLUGIN_DIRS");

    /// Load skills from claude plugins cache (true/false)
    pub const LOAD_CLAUDE_PLUGINS: &str = env_name!("LOAD_CLAUDE_PLUGINS");

    /// Auto-approve level for tool permissions (safe | caution | dangerous)
    pub const AUTO_APPROVE: &str = env_name!("AUTO_APPROVE");

    /// Context window size for the model (e.g., 131072, 200000, 128k, 200k)
    pub const CONTEXT_WINDOW: &str = env_name!("CONTEXT_WINDOW");
}

/// Provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelProvider {
    #[default]
    OpenAI,
    Anthropic,
}

impl ModelProvider {
    /// Get the standard (non-prefixed) API key env var name
    #[inline]
    pub const fn standard_api_key_env(&self) -> &'static str {
        match self {
            Self::OpenAI => env_names::OPENAI_API_KEY,
            Self::Anthropic => env_names::ANTHROPIC_API_KEY,
        }
    }

    /// Get the standard (non-prefixed) model env var name
    #[inline]
    pub const fn standard_model_env(&self) -> &'static str {
        match self {
            Self::OpenAI => env_names::OPENAI_API_MODEL,
            Self::Anthropic => env_names::ANTHROPIC_MODEL,
        }
    }

    /// Get the standard (non-prefixed) API base env var name
    #[inline]
    pub const fn standard_api_base_env(&self) -> &'static str {
        match self {
            Self::OpenAI => env_names::OPENAI_API_BASE,
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
            _ => {
                // Slow path: lowercase and compare
                match s.to_lowercase().as_str() {
                    "openai" => Ok(Self::OpenAI),
                    "anthropic" => Ok(Self::Anthropic),
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
        }
    }
}

/// Complete yomi configuration from environment
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent: AgentConfig,
    pub yolo: bool,
    pub auto_approve: Level,
    pub data_dir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_folders: Option<Vec<String>>,
    /// Claude plugin directories to load skills from
    pub claude_plugin_dirs: Vec<PathBuf>,
    /// Load skills from claude plugins cache (default: true)
    pub load_claude_plugins: bool,
}

impl Config {
    /// Get model configuration (convenience accessor)
    #[inline]
    pub fn model(&self) -> &ModelConfig {
        &self.agent.model
    }

    /// Finalize configuration by computing and filling in default values.
    /// Call this after all configuration sources are loaded.
    pub fn finalize(&mut self, working_dir: &std::path::Path) {
        // Fill log_dir default if not set
        if self.log_dir.is_none() {
            self.log_dir = Some(self.data_dir.join("logs"));
        }

        // Fill skill_folders default if not set
        if self.skill_folders.is_none() {
            self.skill_folders = Some(
                default_skill_folders(working_dir, &self.data_dir)
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
            );
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
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = expand_tilde(DEFAULT_DATA_DIR);
        Self {
            agent: AgentConfig::default(),
            yolo: false,
            auto_approve: Level::default(),
            data_dir,
            log_dir: None,
            skill_folders: None,
            claude_plugin_dirs: vec![expand_tilde("~/.claude/plugins/cache")],
            load_claude_plugins: true,
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

    /// Load configuration from file, then apply environment variable overrides
    pub fn from_file(path: &PathBuf) -> std::result::Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let mut config: Self = toml::from_str(&content)?;
        // Env vars always override file config
        config.load_from_env();
        Ok(config)
    }

    /// Apply environment variable overrides to this config
    pub fn apply_env_overrides(&mut self) {
        self.load_from_env();
    }

    /// Internal: Load all environment variables into config
    fn load_from_env(&mut self) {
        // Provider selection (may affect subsequent provider-specific lookups)
        if let Some(provider) = env_var(env_names::PROVIDER) {
            if let Ok(p) = provider.parse() {
                self.agent.model.provider = p;
            }
        }

        let provider = self.agent.model.provider;

        // API Key: YOMI_ generic > provider-specific standard
        if let Some(key) = env_first(&[env_names::API_KEY, provider.standard_api_key_env()]) {
            self.agent.model.api_key = key;
        }

        // Model: YOMI_ generic > provider-specific standard
        if let Some(model) = env_first(&[env_names::MODEL, provider.standard_model_env()]) {
            self.agent.model.model_id = model;
        }

        // Endpoint: YOMI_ generic > provider-specific standard
        if let Some(endpoint) = env_first(&[env_names::API_BASE, provider.standard_api_base_env()])
        {
            self.agent.model.endpoint = endpoint;
        }

        // Numeric settings
        if let Some(tokens) = env_parse::<u32>(env_names::MAX_TOKENS) {
            self.agent.model.max_tokens = Some(tokens);
        }
        if let Some(temp) = env_parse::<f32>(env_names::TEMPERATURE) {
            self.agent.model.temperature = Some(temp);
        }
        if let Some(iters) = env_parse::<usize>(env_names::MAX_ITERATIONS) {
            self.agent.max_iterations = iters;
        }
        if let Some(budget) = env_parse::<u32>(env_names::THINKING_BUDGET) {
            self.agent.model.thinking.budget_tokens = budget;
        }

        // Boolean settings
        if let Some(enabled) = env_bool_opt(env_names::THINKING) {
            self.agent.model.thinking.enabled = enabled;
        }
        if let Some(effort) = env_var(env_names::THINKING_EFFORT) {
            self.agent.model.thinking.effort = Some(effort);
        }
        self.yolo = env_bool(env_names::YOLO);

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

        // Plugin directories (colon-separated, like PATH)
        if let Some(dirs) = env_var(env_names::PLUGIN_DIRS) {
            self.claude_plugin_dirs = dirs.split(':').map(expand_tilde).collect();
        }

        self.load_claude_plugins = env_bool(env_names::LOAD_CLAUDE_PLUGINS);

        // Auto-approve level (safe | caution | dangerous)
        if let Some(level) = env_var(env_names::AUTO_APPROVE) {
            if let Ok(l) = Level::from_str(&level) {
                self.auto_approve = l;
            }
        }

        // If yolo mode is enabled, auto-approve level should be Dangerous
        if self.yolo {
            self.auto_approve = Level::Dangerous;
        }

        // Context window size (supports formats like "131072", "128k", "200k", "200000")
        if let Some(context_window) = env_var(env_names::CONTEXT_WINDOW) {
            if let Some(tokens) = parse_context_window(&context_window) {
                self.agent.compactor.context_window = tokens;
                // Also update compact_threshold to 80% of context window
                self.agent.compactor.compact_threshold = tokens * 8 / 10;
            }
        }
    }

    /// Get the API key for the current provider
    #[inline]
    pub fn api_key(&self) -> &str {
        &self.agent.model.api_key
    }

    /// Check if API key is configured
    #[inline]
    pub const fn has_api_key(&self) -> bool {
        !self.agent.model.api_key.is_empty()
    }

    /// Set the data directory
    #[must_use]
    pub fn with_data_dir(mut self, data_dir: PathBuf) -> Self {
        self.data_dir = data_dir;
        self
    }
}

/// Get environment variable - inlined for performance
#[inline]
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Try multiple env vars in order, return first set value
#[inline]
fn env_first(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| env_var(name))
}

/// Parse environment variable as a specific type
#[inline]
fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    env_var(name).and_then(|s| s.parse().ok())
}

/// Parse boolean from environment variable
#[inline]
fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .is_ok_and(|s| matches!(s.as_bytes(), b"true" | b"1" | b"yes" | b"TRUE" | b"YES"))
}

#[inline]
fn env_bool_opt(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .map(|s| matches!(s.as_bytes(), b"true" | b"1" | b"yes" | b"TRUE" | b"YES"))
}

/// Parse context window size from string
/// Supports formats like "131072", "128k", "200k", "1m"
fn parse_context_window(s: &str) -> Option<u32> {
    let s = s.trim().to_lowercase();

    // Check for 'k' suffix (thousands)
    if let Some(num_str) = s.strip_suffix('k') {
        let num: f32 = num_str.parse().ok()?;
        return Some((num * 1000.0) as u32);
    }

    // Check for 'm' suffix (millions)
    if let Some(num_str) = s.strip_suffix('m') {
        let num: f32 = num_str.parse().ok()?;
        return Some((num * 1_000_000.0) as u32);
    }

    // Plain number
    s.parse().ok()
}

#[cfg(test)]
mod test_helpers {
    use super::ModelProvider;

    pub fn default_model(provider: ModelProvider) -> String {
        match provider {
            ModelProvider::OpenAI => "gpt-4".to_string(),
            ModelProvider::Anthropic => "claude-3-5-sonnet-20241022".to_string(),
        }
    }

    pub fn default_endpoint(provider: ModelProvider) -> String {
        match provider {
            ModelProvider::OpenAI => "https://api.openai.com/v1".to_string(),
            ModelProvider::Anthropic => "https://api.anthropic.com".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_PREFIX;

    #[test]
    fn test_env_prefix_constant() {
        assert_eq!(ENV_PREFIX, "YOMI_");
    }

    #[test]
    fn test_provider_parse() {
        assert_eq!(
            "openai".parse::<ModelProvider>().unwrap(),
            ModelProvider::OpenAI
        );
        assert_eq!(
            "anthropic".parse::<ModelProvider>().unwrap(),
            ModelProvider::Anthropic
        );
        assert_eq!(
            "OPENAI".parse::<ModelProvider>().unwrap(),
            ModelProvider::OpenAI
        );
        assert_eq!(
            "OpenAI".parse::<ModelProvider>().unwrap(),
            ModelProvider::OpenAI
        );
        assert!("unknown".parse::<ModelProvider>().is_err());
    }

    #[test]
    fn test_provider_display() {
        assert_eq!(ModelProvider::OpenAI.to_string(), "openai");
        assert_eq!(ModelProvider::Anthropic.to_string(), "anthropic");
    }

    #[test]
    fn test_default_model() {
        assert_eq!(test_helpers::default_model(ModelProvider::OpenAI), "gpt-4");
        assert!(test_helpers::default_model(ModelProvider::Anthropic).contains("claude"));
    }

    #[test]
    fn test_default_endpoint() {
        assert!(test_helpers::default_endpoint(ModelProvider::OpenAI).contains("openai.com"));
        assert!(test_helpers::default_endpoint(ModelProvider::Anthropic).contains("anthropic.com"));
    }

    #[test]
    fn test_with_data_dir() {
        let config = Config::default().with_data_dir(PathBuf::from("/custom/path"));
        assert_eq!(config.data_dir, PathBuf::from("/custom/path"));
    }

    #[test]
    fn test_env_bool_parsing() {
        // Test via actual env var manipulation
        std::env::set_var("TEST_BOOL_TRUE", "true");
        std::env::set_var("TEST_BOOL_1", "1");
        std::env::set_var("TEST_BOOL_YES", "yes");
        std::env::set_var("TEST_BOOL_UPPER", "TRUE");
        std::env::set_var("TEST_BOOL_FALSE", "false");
        std::env::set_var("TEST_BOOL_0", "0");
        std::env::set_var("TEST_BOOL_EMPTY", "");

        assert!(env_bool("TEST_BOOL_TRUE"));
        assert!(env_bool("TEST_BOOL_1"));
        assert!(env_bool("TEST_BOOL_YES"));
        assert!(env_bool("TEST_BOOL_UPPER"));
        assert!(!env_bool("TEST_BOOL_FALSE"));
        assert!(!env_bool("TEST_BOOL_0"));
        assert!(!env_bool("TEST_BOOL_EMPTY"));
        assert!(!env_bool("TEST_BOOL_NONEXISTENT"));

        // Cleanup
        for key in [
            "TEST_BOOL_TRUE",
            "TEST_BOOL_1",
            "TEST_BOOL_YES",
            "TEST_BOOL_UPPER",
            "TEST_BOOL_FALSE",
            "TEST_BOOL_0",
            "TEST_BOOL_EMPTY",
        ] {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn test_expand_tilde() {
        let home = std::env::var("HOME").unwrap_or_default();

        // Test tilde expansion
        assert_eq!(expand_tilde("~/foo"), PathBuf::from(format!("{home}/foo")));
        assert_eq!(
            expand_tilde("~/.yomi"),
            PathBuf::from(format!("{home}/.yomi"))
        );

        // Test paths without tilde are unchanged
        assert_eq!(
            expand_tilde("/absolute/path"),
            PathBuf::from("/absolute/path")
        );
        assert_eq!(
            expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );

        // Test tilde not at start
        assert_eq!(expand_tilde("/foo~/bar"), PathBuf::from("/foo~/bar"));
    }

    #[test]
    fn test_default_data_dir_expanded() {
        let config = Config::default();
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(config.data_dir, PathBuf::from(format!("{home}/.yomi")));
    }

    #[test]
    fn test_parse_context_window() {
        // Plain numbers
        assert_eq!(parse_context_window("131072"), Some(131_072));
        assert_eq!(parse_context_window("200000"), Some(200_000));
        assert_eq!(parse_context_window("1000"), Some(1000));

        // k suffix
        assert_eq!(parse_context_window("128k"), Some(128_000));
        assert_eq!(parse_context_window("200k"), Some(200_000));
        assert_eq!(parse_context_window("1.5k"), Some(1500));

        // m suffix
        assert_eq!(parse_context_window("1m"), Some(1_000_000));
        assert_eq!(parse_context_window("2m"), Some(2_000_000));

        // With whitespace
        assert_eq!(parse_context_window(" 128k "), Some(128_000));

        // Invalid values
        assert_eq!(parse_context_window("invalid"), None);
        assert_eq!(parse_context_window(""), None);
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = Config::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();

        // Verify key fields are preserved
        assert_eq!(parsed.agent.model.provider, config.agent.model.provider);
        assert_eq!(parsed.yolo, config.yolo);
        assert_eq!(parsed.data_dir, config.data_dir);
    }

    #[test]
    fn test_config_model_accessor() {
        let config = Config::default();
        assert_eq!(config.model().provider, config.agent.model.provider);
        assert_eq!(config.model().model_id, config.agent.model.model_id);
    }
}

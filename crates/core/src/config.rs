//! Configuration management for nekoclaw
//!
//! Environment variables (all prefixed with NEKOCLAW_):
//!
//! # Provider Selection
//! - `NEKOCLAW_PROVIDER`: Provider to use (openai, anthropic)
//!
//! # Generic API Settings (used for selected provider)
//! - `NEKOCLAW_API_KEY`: API key for the selected provider
//! - `NEKOCLAW_MODEL`: Model ID (e.g., gpt-4, claude-3-opus-20240229)
//! - `NEKOCLAW_ENDPOINT`: Custom API endpoint URL
//! - `NEKOCLAW_MAX_TOKENS`: Maximum tokens per request
//! - `NEKOCLAW_TEMPERATURE`: Temperature (0.0 - 2.0)
//!
//! # Provider-Specific Settings (optional, takes precedence over generic)
//! - `NEKOCLAW_OPENAI_API_KEY` / `NEKOCLAW_ANTHROPIC_API_KEY`
//! - `NEKOCLAW_OPENAI_MODEL` / `NEKOCLAW_ANTHROPIC_MODEL`
//! - `NEKOCLAW_OPENAI_ENDPOINT` / `NEKOCLAW_ANTHROPIC_ENDPOINT`
//!
//! # Application Settings
//! - `NEKOCLAW_SANDBOX`: Enable sandbox mode (true/false)
//! - `NEKOCLAW_YOLO`: Skip all confirmations (true/false)
//! - `NEKOCLAW_DATA_DIR`: Data directory path (see `DEFAULT_DATA_DIR`)
//! - `NEKOCLAW_MAX_ITERATIONS`: Max agent iterations (default: 50)
//! - `NEKOCLAW_ENABLE_SUB_AGENTS`: Enable sub-agents (true/false)
//!
//! Priority: CLI args > Provider-specific env > Generic env > Defaults

use crate::agent::AgentConfig;
use crate::env_name;
use crate::provider::ModelConfig;
use crate::storage::StorageConfig;
use std::path::PathBuf;

/// Default data directory path
pub const DEFAULT_DATA_DIR: &str = "~/.nekoclaw";

/// Environment variable names (for easy reference and IDE completion)
pub mod env_names {
    use super::env_name;

    /// Provider selection
    pub const PROVIDER: &str = env_name!("PROVIDER");

    /// Generic API settings
    pub const API_KEY: &str = env_name!("API_KEY");
    pub const MODEL: &str = env_name!("MODEL");
    pub const ENDPOINT: &str = env_name!("ENDPOINT");
    pub const API_BASE: &str = env_name!("API_BASE");
    pub const MAX_TOKENS: &str = env_name!("MAX_TOKENS");
    pub const TEMPERATURE: &str = env_name!("TEMPERATURE");

    /// Provider-specific prefixes
    pub const OPENAI_PREFIX: &str = env_name!("OPENAI_");
    pub const ANTHROPIC_PREFIX: &str = env_name!("ANTHROPIC_");

    /// Application settings
    pub const SANDBOX: &str = env_name!("SANDBOX");
    pub const YOLO: &str = env_name!("YOLO");
    pub const DATA_DIR: &str = env_name!("DATA_DIR");
    pub const MAX_ITERATIONS: &str = env_name!("MAX_ITERATIONS");
    pub const ENABLE_SUB_AGENTS: &str = env_name!("ENABLE_SUB_AGENTS");

    /// Thinking configuration
    pub const THINKING: &str = env_name!("THINKING");
    pub const THINKING_BUDGET: &str = env_name!("THINKING_BUDGET");

    /// Logging configuration
    pub const LOG_DIR: &str = env_name!("LOG_DIR");
    pub const LOG_LEVEL: &str = "RUST_LOG"; // Standard env var, no prefix
}

/// Provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelProvider {
    #[default]
    OpenAI,
    Anthropic,
}

impl ModelProvider {
    /// Get the env prefix for this provider (e.g., "`NEKOCLAW_OPENAI`_")
    #[inline]
    pub const fn env_prefix(&self) -> &'static str {
        match self {
            Self::OpenAI => env_names::OPENAI_PREFIX,
            Self::Anthropic => env_names::ANTHROPIC_PREFIX,
        }
    }
}

impl std::str::FromStr for ModelProvider {
    type Err = String;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
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

/// Complete nekoclaw configuration from environment
#[derive(Debug, Clone)]
pub struct Config {
    pub provider: ModelProvider,
    pub model: ModelConfig,
    pub storage: StorageConfig,
    pub agent: AgentConfig,
    pub sandbox: bool,
    pub yolo: bool,
    pub data_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: ModelProvider::default(),
            model: ModelConfig::default(),
            storage: StorageConfig::default(),
            agent: AgentConfig::default(),
            sandbox: false,
            yolo: false,
            data_dir: PathBuf::from(DEFAULT_DATA_DIR),
        }
    }
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // Provider selection
        if let Some(provider) = env_var(env_names::PROVIDER) {
            if let Ok(p) = provider.parse() {
                config.provider = p;
            }
        }

        // Load provider-specific or generic API settings
        let provider_env_prefix = config.provider.env_prefix();

        // API Key: {PREFIX}{PROVIDER}_API_KEY > {PREFIX}API_KEY
        config.model.api_key = env_var_with_suffix(provider_env_prefix, "API_KEY")
            .or_else(|| env_var(env_names::API_KEY))
            .unwrap_or_default();

        // Model: {PREFIX}{PROVIDER}_MODEL > {PREFIX}MODEL > defaults
        config.model.model_id = env_var_with_suffix(provider_env_prefix, "MODEL")
            .or_else(|| env_var(env_names::MODEL))
            .unwrap_or_else(|| default_model(config.provider));

        // Endpoint: {PREFIX}{PROVIDER}_ENDPOINT > {PREFIX}ENDPOINT > defaults
        config.model.endpoint = env_var_with_suffix(provider_env_prefix, "ENDPOINT")
            .or_else(|| env_var(env_names::ENDPOINT))
            .or_else(|| env_var_with_suffix(provider_env_prefix, "API_BASE")) // Provider-specific API_BASE
            .or_else(|| env_var(env_names::API_BASE)) // Support legacy API_BASE for backward compatibility
            .unwrap_or_else(|| default_endpoint(config.provider));

        // Max tokens
        if let Some(tokens) = env_var(env_names::MAX_TOKENS).and_then(|s| s.parse().ok()) {
            config.model.max_tokens = Some(tokens);
            config.agent.model.max_tokens = Some(tokens);
        }

        // Temperature
        if let Some(temp) = env_var(env_names::TEMPERATURE).and_then(|s| s.parse().ok()) {
            config.model.temperature = Some(temp);
            config.agent.model.temperature = Some(temp);
        }

        // Update agent config model
        config.agent.model = config.model.clone();

        // Sandbox mode
        config.sandbox = env_bool(env_names::SANDBOX);

        // YOLO mode
        config.yolo = env_bool(env_names::YOLO);

        // Data directory
        if let Some(dir) = env_var(env_names::DATA_DIR) {
            config.data_dir = PathBuf::from(dir);
            config.storage.url = config.data_dir.to_string_lossy().to_string();
        }

        // Max iterations
        if let Some(iters) = env_var(env_names::MAX_ITERATIONS).and_then(|s| s.parse().ok()) {
            config.agent.max_iterations = iters;
        }

        // Enable sub-agents (default true unless explicitly set to "false")
        config.agent.enable_sub_agents =
            env_var(env_names::ENABLE_SUB_AGENTS).as_deref() != Some("false");

        // Thinking configuration
        config.model.thinking.enabled = env_bool(env_names::THINKING);
        if let Some(budget) = env_var(env_names::THINKING_BUDGET).and_then(|s| s.parse().ok()) {
            config.model.thinking.budget_tokens = budget;
            config.agent.model.thinking.budget_tokens = budget;
        }

        config
    }

    /// Get the API key for the current provider
    #[inline]
    pub fn api_key(&self) -> &str {
        &self.model.api_key
    }

    /// Check if API key is configured
    #[inline]
    pub const fn has_api_key(&self) -> bool {
        !self.model.api_key.is_empty()
    }

    /// Set the data directory
    pub fn with_data_dir(mut self, data_dir: PathBuf) -> Self {
        self.storage.url = data_dir.to_string_lossy().to_string();
        self.data_dir = data_dir;
        self
    }
}

/// Get environment variable - inlined for performance
#[inline]
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Build env var name with suffix efficiently using pre-allocated string
/// Format: {prefix}{suffix} (e.g., "`NEKOCLAW_OPENAI`" + "_`API_KEY`")
#[inline]
fn env_var_with_suffix(prefix: &str, suffix: &str) -> Option<String> {
    // Pre-calculate capacity to avoid reallocations
    let mut name = String::with_capacity(prefix.len() + suffix.len());
    name.push_str(prefix);
    name.push_str(suffix);
    std::env::var(&name).ok()
}

/// Parse boolean from environment variable
#[inline]
fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .map(|s| matches!(s.as_bytes(), b"true" | b"1" | b"yes" | b"TRUE" | b"YES"))
        .unwrap_or(false)
}

#[inline]
fn default_model(provider: ModelProvider) -> String {
    match provider {
        ModelProvider::OpenAI => "gpt-4".to_string(),
        ModelProvider::Anthropic => "claude-3-5-sonnet-20241022".to_string(),
    }
}

#[inline]
fn default_endpoint(provider: ModelProvider) -> String {
    match provider {
        ModelProvider::OpenAI => "https://api.openai.com/v1".to_string(),
        ModelProvider::Anthropic => "https://api.anthropic.com".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_PREFIX;

    #[test]
    fn test_env_prefix_constant() {
        assert_eq!(ENV_PREFIX, "NEKOCLAW_");
    }

    #[test]
    fn test_env_names() {
        assert_eq!(env_names::PROVIDER, "NEKOCLAW_PROVIDER");
        assert_eq!(env_names::API_KEY, "NEKOCLAW_API_KEY");
        assert_eq!(env_names::OPENAI_PREFIX, "NEKOCLAW_OPENAI_");
        assert_eq!(env_names::ANTHROPIC_PREFIX, "NEKOCLAW_ANTHROPIC_");
    }

    #[test]
    fn test_provider_env_prefix() {
        assert_eq!(ModelProvider::OpenAI.env_prefix(), "NEKOCLAW_OPENAI_");
        assert_eq!(ModelProvider::Anthropic.env_prefix(), "NEKOCLAW_ANTHROPIC_");
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
        assert_eq!(default_model(ModelProvider::OpenAI), "gpt-4");
        assert!(default_model(ModelProvider::Anthropic).contains("claude"));
    }

    #[test]
    fn test_default_endpoint() {
        assert!(default_endpoint(ModelProvider::OpenAI).contains("openai.com"));
        assert!(default_endpoint(ModelProvider::Anthropic).contains("anthropic.com"));
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
    fn test_env_var_with_suffix() {
        // Test the efficient string building
        std::env::set_var("NEKOCLAW_TEST_SUFFIX", "test_value");

        let result = env_var_with_suffix("NEKOCLAW_", "TEST_SUFFIX");
        assert_eq!(result, Some("test_value".to_string()));

        std::env::remove_var("NEKOCLAW_TEST_SUFFIX");
    }
}

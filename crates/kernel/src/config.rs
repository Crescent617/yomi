use crate::agent::AgentConfig;
use crate::env_name;
use crate::providers::ModelConfig;
use crate::storage::StorageConfig;
use std::path::PathBuf;

/// Expand `~` to the user's home directory
pub fn expand_tilde(path: impl AsRef<str>) -> PathBuf {
    let path = path.as_ref();
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    }
    PathBuf::from(path)
}

/// Default data directory path
pub const DEFAULT_DATA_DIR: &str = "~/.yomi";

/// Environment variable names (for easy reference and IDE completion)
pub mod env_names {
    use super::env_name;

    /// Provider selection
    pub const PROVIDER: &str = env_name!("PROVIDER");

    /// Generic API settings
    pub const API_KEY: &str = env_name!("API_KEY");
    pub const MODEL: &str = env_name!("MODEL");
    pub const API_BASE: &str = env_name!("API_BASE");
    pub const MAX_TOKENS: &str = env_name!("MAX_TOKENS");
    pub const TEMPERATURE: &str = env_name!("TEMPERATURE");

    /// Provider-specific prefixes (YOMI_ prefixed)
    pub const OPENAI_PREFIX: &str = env_name!("OPENAI_");
    pub const ANTHROPIC_PREFIX: &str = env_name!("ANTHROPIC_");

    /// Standard non-prefixed provider-specific env vars
    pub const OPENAI_API_KEY: &str = "OPENAI_API_KEY";
    pub const ANTHROPIC_API_KEY: &str = "ANTHROPIC_API_KEY";
    pub const OPENAI_API_MODEL: &str = "OPENAI_API_MODEL";
    pub const ANTHROPIC_MODEL: &str = "ANTHROPIC_MODEL";
    pub const OPENAI_API_BASE: &str = "OPENAI_API_BASE";
    pub const ANTHROPIC_BASE_URL: &str = "ANTHROPIC_BASE_URL";

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

    /// Skill folders (comma-separated paths)
    pub const SKILL_FOLDERS: &str = env_name!("SKILL_FOLDERS");
}

/// Provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

/// Complete yomi configuration from environment
#[derive(Debug, Clone)]
pub struct Config {
    pub provider: ModelProvider,
    pub model: ModelConfig,
    pub storage: StorageConfig,
    pub agent: AgentConfig,
    pub sandbox: bool,
    pub yolo: bool,
    pub data_dir: PathBuf,
    pub skill_folders: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = expand_tilde(DEFAULT_DATA_DIR);
        Self {
            provider: ModelProvider::default(),
            model: ModelConfig::default(),
            storage: StorageConfig::with_data_dir(&data_dir),
            agent: AgentConfig::default(),
            sandbox: false,
            yolo: false,
            data_dir,
            skill_folders: Vec::new(),
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
        // Priority: Provider-specific (YOMI_) > Provider-specific (standard) > Generic (YOMI_) > Defaults
        let provider = config.provider;

        config.model.api_key = env_var(provider.standard_api_key_env())
            .or_else(|| env_var(env_names::API_KEY))
            .unwrap_or_default();

        // Model: YOMI_OPENAI_MODEL > OPENAI_MODEL > YOMI_MODEL > defaults
        config.model.model_id = env_var(provider.standard_model_env())
            .or_else(|| env_var(env_names::MODEL))
            .unwrap_or_else(|| default_model(provider));

        // Endpoint: YOMI_OPENAI_ENDPOINT > OPENAI_ENDPOINT > YOMI_ENDPOINT > YOMI_OPENAI_API_BASE > OPENAI_API_BASE > YOMI_API_BASE > defaults
        config.model.endpoint = env_var(env_names::API_BASE)
            .or_else(|| env_var(provider.standard_api_base_env()))
            .unwrap_or_else(|| default_endpoint(provider));

        // Max tokens
        if let Some(tokens) = env_var(env_names::MAX_TOKENS).and_then(|s| s.parse().ok()) {
            config.model.max_tokens = Some(tokens);
            config.agent.model.max_tokens = Some(tokens);
        }

        // Temperature
        if let Some(temp) = env_var(env_names::TEMPERATURE).and_then(|s| s.parse().ok()) {
            config.model.temperature = Some(temp);
        }

        // Sandbox mode
        config.sandbox = env_bool(env_names::SANDBOX);

        // YOLO mode
        config.yolo = env_bool(env_names::YOLO);

        // Data directory
        if let Some(dir) = env_var(env_names::DATA_DIR) {
            config.data_dir = expand_tilde(dir);
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
        config.model.thinking.enabled = env_bool_opt(env_names::THINKING).unwrap_or(true);
        if let Some(budget) = env_var(env_names::THINKING_BUDGET).and_then(|s| s.parse().ok()) {
            config.model.thinking.budget_tokens = budget;
        }

        // Skill folders from env (comma-separated)
        if let Some(folders) = env_var(env_names::SKILL_FOLDERS) {
            config.skill_folders = folders.split(',').map(String::from).collect();
        }

        // Update agent config model (must be after all model config)
        config.agent.model = config.model.clone();

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
    #[must_use]
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

/// Parse boolean from environment variable
#[inline]
fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .map(|s| matches!(s.as_bytes(), b"true" | b"1" | b"yes" | b"TRUE" | b"YES"))
        .unwrap_or(false)
}

#[inline]
fn env_bool_opt(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .map(|s| matches!(s.as_bytes(), b"true" | b"1" | b"yes" | b"TRUE" | b"YES"))
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
        assert!(!config.storage.url.starts_with('~'));
    }
}

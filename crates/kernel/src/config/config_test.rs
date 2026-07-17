use super::*;

mod test_helpers {
    use super::ModelProvider;

    pub fn default_model(provider: ModelProvider) -> String {
        match provider {
            ModelProvider::OpenAI => "gpt-4".to_string(),
            ModelProvider::OpenAIResponse => "gpt-5".to_string(),
            ModelProvider::Anthropic => "claude-3-5-sonnet-20241022".to_string(),
        }
    }

    pub fn default_endpoint(provider: ModelProvider) -> String {
        match provider {
            ModelProvider::OpenAI | ModelProvider::OpenAIResponse => {
                "https://api.openai.com/v1".to_string()
            }
            ModelProvider::Anthropic => "https://api.anthropic.com".to_string(),
        }
    }
}

use crate::ENV_PREFIX;

#[test]
fn test_env_prefix_constant() {
    assert_eq!(ENV_PREFIX, "YOMI_");
}

#[test]
fn config_env_roundtrip() {
    let parsed: Config = toml::from_str(
        r#"
[env]
YOMI_TEST_CONFIG_ENV = "configured"
"#,
    )
    .unwrap();

    assert_eq!(
        parsed.env.get("YOMI_TEST_CONFIG_ENV").map(String::as_str),
        Some("configured")
    );
}

#[test]
fn inject_env_preserves_existing_values() {
    let key = format!("YOMI_TEST_ENV_{}", crate::types::MessageId::new().as_str());
    std::env::set_var(&key, "host");
    let mut config = Config::default();
    config.env.insert(key.clone(), "configured".to_string());

    config.inject_env().unwrap();

    assert_eq!(std::env::var(&key).unwrap(), "host");
    std::env::remove_var(key);
}

#[test]
fn inject_env_sets_missing_values() {
    let key = format!("YOMI_TEST_ENV_{}", crate::types::MessageId::new().as_str());
    let mut config = Config::default();
    config.env.insert(key.clone(), "configured".to_string());

    config.inject_env().unwrap();

    assert_eq!(std::env::var(&key).unwrap(), "configured");
    std::env::remove_var(key);
}

#[test]
fn inject_env_rejects_invalid_names() {
    let mut config = Config::default();
    config
        .env
        .insert("INVALID=NAME".to_string(), "value".to_string());
    assert!(config.inject_env().is_err());
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
    assert_eq!(
        "openai_response".parse::<ModelProvider>().unwrap(),
        ModelProvider::OpenAIResponse
    );
    assert_eq!(
        "openai-response".parse::<ModelProvider>().unwrap(),
        ModelProvider::OpenAIResponse
    );
    assert_eq!(
        "OpenAI-Response".parse::<ModelProvider>().unwrap(),
        ModelProvider::OpenAIResponse
    );
}

#[test]
fn test_provider_display() {
    assert_eq!(ModelProvider::OpenAI.to_string(), "openai");
    assert_eq!(ModelProvider::Anthropic.to_string(), "anthropic");
    assert_eq!(ModelProvider::OpenAIResponse.to_string(), "openai_response");
}

#[test]
fn test_provider_serde_roundtrip() {
    for provider in [
        ModelProvider::OpenAI,
        ModelProvider::Anthropic,
        ModelProvider::OpenAIResponse,
    ] {
        let json = serde_json::to_string(&provider).unwrap();
        let parsed: ModelProvider = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, provider);
    }
    // Explicit wire format checks
    assert_eq!(
        serde_json::to_string(&ModelProvider::OpenAI).unwrap(),
        "\"openai\""
    );
    assert_eq!(
        serde_json::to_string(&ModelProvider::Anthropic).unwrap(),
        "\"anthropic\""
    );
    assert_eq!(
        serde_json::to_string(&ModelProvider::OpenAIResponse).unwrap(),
        "\"openai_response\""
    );
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
fn test_config_serialization_roundtrip() {
    let config = Config::default();
    let toml_str = toml::to_string(&config).unwrap();
    let parsed: Config = toml::from_str(&toml_str).unwrap();

    assert_eq!(parsed.models[0].provider, config.models[0].provider);
    assert_eq!(parsed.data_dir, config.data_dir);
}

#[test]
fn features_default_to_disabled() {
    let features = FeaturesConfig::default();

    assert!(!features.update_session_title_enabled());
}

#[test]
fn features_inherit_all_and_allow_explicit_overrides() {
    let parsed: Config = toml::from_str(
        r"
[features]
all = true
update_session_title = false
",
    )
    .unwrap();

    assert!(!parsed.features.update_session_title_enabled());
}

#[test]
fn individual_feature_can_be_enabled_when_all_is_disabled() {
    let parsed: Config = toml::from_str(
        r"
[features]
update_session_title = true
",
    )
    .unwrap();

    assert!(parsed.features.update_session_title_enabled());
}

#[test]
fn test_tasks_fast_model_from_toml() {
    let toml = r#"
[tasks]
fast_model = "fast"

[[models]]
name = "fast"
model_id = "gpt-4.1-mini"
"#;
    let parsed: Config = toml::from_str(toml).unwrap();
    assert_eq!(parsed.tasks.fast_model.as_deref(), Some("fast"));
}

#[test]
fn test_config_model_headers_roundtrip() {
    let mut config = Config::default();
    config.models[0]
        .headers
        .insert("X-Custom-Key".to_string(), "my-value".to_string());
    config.models[0]
        .headers
        .insert("Authorization".to_string(), "Bearer override".to_string());

    let toml_str = toml::to_string(&config).unwrap();
    let parsed: Config = toml::from_str(&toml_str).unwrap();

    assert_eq!(
        parsed.models[0].headers.get("X-Custom-Key"),
        Some(&"my-value".to_string())
    );
    assert_eq!(
        parsed.models[0].headers.get("Authorization"),
        Some(&"Bearer override".to_string())
    );
}

#[test]
fn test_config_model_headers_from_toml() {
    let toml = r#"
[[models]]
name = "default"
provider = "openai"
model_id = "gpt-4"
endpoint = "https://api.example.com/v1"
api_key = "sk-test"

[models.headers]
"X-Custom-Key" = "my-value"
"Authorization" = "Bearer override"
"#;
    let parsed: Config = toml::from_str(toml).unwrap();
    assert_eq!(
        parsed.models[0].headers.get("X-Custom-Key"),
        Some(&"my-value".to_string())
    );
    assert_eq!(
        parsed.models[0].headers.get("Authorization"),
        Some(&"Bearer override".to_string())
    );
}

#[test]
fn test_config_model_accessor() {
    let config = Config::default();
    assert_eq!(config.model().unwrap().provider, config.models[0].provider);
    assert_eq!(config.model().unwrap().model_id, config.models[0].model_id);
}

#[test]
fn compactor_micro_compaction_config_defaults_to_disabled() {
    let default_config = Config::default();
    assert!(!default_config.agent.compactor.micro_compact_enabled);

    let parsed: Config = toml::from_str(
        "
[agent.compactor]
micro_compact_enabled = true
",
    )
    .unwrap();
    assert!(parsed.agent.compactor.micro_compact_enabled);
}

#[test]
fn validate_rejects_invalid_compactor_settings() {
    let mut config = Config::default();
    config.agent.compactor.threshold_ratio = 0.0;
    assert!(config.validate().is_err());

    config.agent.compactor.threshold_ratio = f32::NAN;
    assert!(config.validate().is_err());

    config.agent.compactor.threshold_ratio = 0.9;
    config.agent.compactor.summary_max_tokens = 0;
    assert!(config.validate().is_err());
}

#[test]
fn validate_rejects_zero_model_token_limits() {
    let mut config = Config::default();
    config.models[0].max_tokens = Some(0);
    assert!(config.validate().is_err());

    config.models[0].max_tokens = Some(1);
    config.models[0].context_window = 0;
    assert!(config.validate().is_err());
}

#[test]
fn default_config_is_valid() {
    assert!(Config::default().validate().is_ok());
}

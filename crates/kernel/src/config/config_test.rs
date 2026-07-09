use super::*;

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

use crate::hooks::{HookEntry, HookEvent};
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
fn test_config_serialization_roundtrip() {
    let config = Config {
        hooks: vec![HookEntry {
            name: "test-hook".to_string(),
            event: HookEvent::PreToolUse,
            matcher: "shell".to_string(),
            handler_type: String::new(),
            command: "echo test".to_string(),
            timeout: 10,
        }],
        ..Config::default()
    };
    let toml_str = toml::to_string(&config).unwrap();
    let parsed: Config = toml::from_str(&toml_str).unwrap();

    // Verify key fields are preserved
    assert_eq!(parsed.models[0].provider, config.models[0].provider);
    assert_eq!(parsed.data_dir, config.data_dir);
    assert_eq!(parsed.hooks.len(), 1);
    assert_eq!(parsed.hooks[0].name, "test-hook");
    assert_eq!(parsed.hooks[0].command, "echo test");
    assert_eq!(parsed.hooks[0].timeout, 10);
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
fn test_hook_entry_default_timeout() {
    let entry: HookEntry = toml::from_str(
        r#"
            name = "default-timeout-hook"
            event = "PreToolUse"
            matcher = "shell"
            command = "echo ok"
            "#,
    )
    .unwrap();
    assert_eq!(entry.timeout, 30);
}

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
fn inject_env_replaces_values_set_by_an_earlier_injection() {
    let key = format!("YOMI_TEST_ENV_{}", crate::types::MessageId::new().as_str());
    let mut first = Config::default();
    first.env.insert(key.clone(), "first".to_string());
    first.inject_env().unwrap();

    let mut second = Config::default();
    second.env.insert(key.clone(), "second".to_string());
    second.inject_env().unwrap();

    assert_eq!(std::env::var(&key).unwrap(), "second");
    Config::clear_injected_env();
    assert!(std::env::var_os(key).is_none());
}

#[test]
fn injected_env_names_tracks_only_values_set_by_config() {
    let injected_key = format!("YOMI_TEST_ENV_{}", crate::types::MessageId::new().as_str());
    let host_key = format!("YOMI_TEST_ENV_{}", crate::types::MessageId::new().as_str());
    std::env::set_var(&host_key, "host");
    let mut config = Config::default();
    config
        .env
        .insert(injected_key.clone(), "configured".to_string());
    config
        .env
        .insert(host_key.clone(), "configured".to_string());

    config.inject_env().unwrap();
    let names = Config::injected_env_names();

    assert!(names.contains(&injected_key));
    assert!(!names.contains(&host_key));
    Config::clear_injected_env();
    std::env::remove_var(host_key);
}

#[test]
fn inject_env_does_not_clear_removed_values() {
    let key = format!("YOMI_TEST_ENV_{}", crate::types::MessageId::new().as_str());
    let mut config = Config::default();
    config.env.insert(key.clone(), "configured".to_string());
    config.inject_env().unwrap();

    let next = Config::default();
    next.inject_env().unwrap();

    assert_eq!(std::env::var(&key).unwrap(), "configured");
    Config::clear_injected_env();
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
    assert!(!features.cron_tool_enabled());
    assert!(!features.todo_tool_enabled());
    // Attachments is the exception: a default capability, not an experiment.
    assert!(features.attachments_enabled());
}

#[test]
fn attachments_default_on_and_explicitly_disableable() {
    // `all` does not affect attachments in either direction.
    let parsed: Config = toml::from_str(
        r"
[features]
all = true
",
    )
    .unwrap();
    assert!(parsed.features.attachments_enabled());

    let parsed: Config = toml::from_str(
        r"
[features]
all = true
attachments = false
",
    )
    .unwrap();
    assert!(!parsed.features.attachments_enabled());

    let parsed: Config = toml::from_str(
        r"
[features]
attachments = false
",
    )
    .unwrap();
    assert!(!parsed.features.attachments_enabled());
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
    assert!(parsed.features.cron_tool_enabled());
    assert!(parsed.features.todo_tool_enabled());

    let parsed: Config = toml::from_str(
        r"
[features]
all = true
cron_tool = false
todo_tool = false
",
    )
    .unwrap();

    assert!(!parsed.features.cron_tool_enabled());
    assert!(!parsed.features.todo_tool_enabled());
}

#[test]
fn individual_feature_can_be_enabled_when_all_is_disabled() {
    let parsed: Config = toml::from_str(
        r"
[features]
update_session_title = true
cron_tool = true
todo_tool = true
",
    )
    .unwrap();

    assert!(parsed.features.update_session_title_enabled());
    assert!(parsed.features.cron_tool_enabled());
    assert!(parsed.features.todo_tool_enabled());
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

#[test]
fn set_kernel_config_retains_parser_location() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");

    let error = Config::set_kernel_config_at(&path, "# comment\nauto_approve = \"unsupported\"\n")
        .expect_err("typed config should be rejected");
    let message = error.to_string();

    assert!(message.contains("Invalid TOML:"));
    assert!(message.contains("line 2, column"));
}

#[test]
fn set_kernel_config_rejects_duplicate_model_names() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let content = r#"
[[models]]
name = "duplicate"

[[models]]
name = "duplicate"
"#;

    let error = Config::set_kernel_config_at(&path, content)
        .expect_err("duplicate names should be rejected");

    assert!(error
        .to_string()
        .contains("Invalid config: duplicate model name in [[models]]"));
}

#[test]
fn set_kernel_config_rejects_missing_default_model() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let content = r#"
[agent]
default_model = "missing"

[[models]]
name = "available"
"#;

    let error = Config::set_kernel_config_at(&path, content)
        .expect_err("missing default model should fail");

    assert!(error
        .to_string()
        .contains("Invalid config: agent.default_model must match a [[models]] name"));
}

#[test]
fn set_kernel_config_rejects_invalid_env_names() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let content = r#"
[env]
"INVALID=NAME" = "value"
"#;

    let error = Config::set_kernel_config_at(&path, content)
        .expect_err("invalid environment variable name should fail");

    assert!(error
        .to_string()
        .contains("Invalid environment variable name"));
    assert!(!path.exists());
}

#[test]
fn set_kernel_config_preserves_original_toml_text() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let content = "# keep this comment\nmax_checkpoints = 7  # and spacing\n";

    Config::set_kernel_config_at(&path, content).expect("save config");

    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(files.len(), 1, "temporary file should be renamed");
}

#[test]
fn set_kernel_config_replaces_existing_config() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# old\n").unwrap();

    Config::set_kernel_config_at(&path, "# new\n").expect("replace config");

    assert_eq!(std::fs::read_to_string(path).unwrap(), "# new\n");
}

#[test]
fn invalid_kernel_config_does_not_replace_existing_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let original = "# existing config\n";
    std::fs::write(&path, original).unwrap();

    Config::set_kernel_config_at(&path, "auto_approve = \"unsupported\"\n")
        .expect_err("invalid config should not be written");

    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

#[test]
fn get_kernel_config_returns_invalid_content_without_effective_config() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let content = "auto_approve = \"unsupported\"\n";
    std::fs::write(&path, content).unwrap();

    let config = Config::get_kernel_config_from(&path).expect("read editable config");

    assert_eq!(config.content, content);
    assert_eq!(config.path, path.to_string_lossy());
    assert!(config.full_config.is_empty());
}

#[test]
fn get_kernel_config_returns_defaults_for_missing_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");

    let config = Config::get_kernel_config_from(&path).expect("read default config");
    let effective: Config = toml::from_str(&config.full_config).unwrap();

    assert!(config.content.is_empty());
    assert_eq!(config.path, path.to_string_lossy());
    assert!(effective.model().is_some());
}

#[cfg(unix)]
#[test]
fn set_kernel_config_follows_relative_symlink_without_replacing_it() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let target = dir.path().join("managed-config.toml");
    let link = dir.path().join("config.toml");
    std::fs::write(&target, "# old\n").unwrap();
    symlink("managed-config.toml", &link).unwrap();

    Config::set_kernel_config_at(&link, "# new\n").expect("save through symlink");

    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        std::path::Path::new("managed-config.toml")
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "# new\n");
}

#[cfg(unix)]
#[test]
fn newly_created_kernel_config_has_private_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");

    Config::set_kernel_config_at(&path, "# valid config\n").expect("save config");

    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

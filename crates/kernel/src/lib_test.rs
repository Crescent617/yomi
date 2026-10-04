use super::*;

fn hermetic_config() -> Config {
    // Keep the test hermetic: no skill folders from the host machine.
    let mut config = Config {
        skill_folders: Some(vec![]),
        ..Config::default()
    };
    config.finalize();
    config
}

#[tokio::test]
async fn build_agent_config_renders_default_name() {
    let config = hermetic_config();
    let agent = build_agent_config(&config).await;
    assert!(agent.system_prompt.starts_with("You are Yomi,"));
    assert!(!agent.system_prompt.contains("{{name}}"));
}

#[tokio::test]
async fn build_agent_config_renders_configured_name() {
    let mut config = hermetic_config();
    config.agent.name = "Claw".to_string();
    let agent = build_agent_config(&config).await;
    assert!(agent.system_prompt.starts_with("You are Claw,"));
}

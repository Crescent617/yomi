use kernel::{expand_tilde, misc::plugin::EnabledPlugins};
use serde::Deserialize;

/// Claude Code settings.json structure (partial)
#[derive(Debug, Deserialize, Default)]
pub struct ClaudeSettings {
    #[serde(default, rename = "enabledPlugins")]
    pub enabled_plugins: EnabledPlugins,
}

impl ClaudeSettings {
    /// Load settings from ~/.claude/settings.json if it exists
    pub fn load() -> Self {
        let settings_path = expand_tilde("~/.claude/settings.json");
        if !settings_path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(&settings_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Failed to parse {}: {}", settings_path.display(), e);
                Self::default()
            }),
            Err(e) => {
                tracing::warn!("Failed to read {}: {}", settings_path.display(), e);
                Self::default()
            }
        }
    }
}

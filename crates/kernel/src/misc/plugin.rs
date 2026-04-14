use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A loaded plugin with metadata
#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub path: PathBuf,
    pub skills_path: Option<PathBuf>,
    pub skills_paths: Vec<PathBuf>,
}

/// Plugin manifest (plugin.json)
#[derive(Debug, Deserialize)]
pub struct PluginManifest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub skills: Option<PluginSkills>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PluginSkills {
    Single(String),
    Multiple(Vec<String>),
}

/// Installed plugins index (`installed_plugins.json`)
#[derive(Debug, Deserialize, Serialize)]
pub struct InstalledPluginsIndex {
    pub version: u32,
    pub plugins: HashMap<String, Vec<InstalledPluginInfo>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InstalledPluginInfo {
    pub scope: String,
    pub install_path: String,
    pub version: String,
    #[serde(rename = "installedAt")]
    pub installed_at: String,
    #[serde(rename = "lastUpdated")]
    pub last_updated: String,
    #[serde(rename = "gitCommitSha")]
    pub git_commit_sha: String,
}

/// Plugin loader that scans directories for plugins
#[derive(Debug, Clone)]
pub struct PluginLoader {
    plugin_dirs: Vec<PathBuf>,
}

impl PluginLoader {
    pub const fn new(plugin_dirs: Vec<PathBuf>) -> Self {
        Self { plugin_dirs }
    }

    /// Load all plugins from configured directories
    /// First tries to read `installed_plugins.json`, then falls back to directory scanning
    pub fn load_all(&self) -> Result<Vec<Plugin>> {
        let mut plugins = Vec::new();
        let mut loaded_paths = std::collections::HashSet::new();

        for dir in &self.plugin_dirs {
            if !dir.exists() {
                tracing::warn!("Plugin directory does not exist: {}", dir.display());
                continue;
            }

            // Try to load from installed_plugins.json first
            let index_path = dir.parent().map(|p| p.join("installed_plugins.json"));
            if let Some(ref index_path) = index_path {
                if index_path.exists() {
                    match Self::load_from_installed_plugins(index_path, &mut loaded_paths) {
                        Ok(ps) => {
                            plugins.extend(ps);
                            continue; // Skip directory scanning if index was loaded
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load installed_plugins.json: {}, falling back to directory scan", e);
                        }
                    }
                }
            }

            // Fall back to directory scanning
            self.load_from_dir(dir, &mut plugins, &mut loaded_paths)
                .with_context(|| format!("Failed to load plugins from {}", dir.display()))?;
        }

        Ok(plugins)
    }

    /// Load plugins from `installed_plugins.json` index
    /// Directly checks {`install_path}/skills`/ for each installed plugin
    fn load_from_installed_plugins(
        index_path: &Path,
        loaded_paths: &mut std::collections::HashSet<PathBuf>,
    ) -> Result<Vec<Plugin>> {
        let content = std::fs::read_to_string(index_path)
            .with_context(|| format!("Failed to read {}", index_path.display()))?;
        let index: InstalledPluginsIndex = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", index_path.display()))?;

        let mut plugins = Vec::new();

        for (plugin_id, installs) in index.plugins {
            for install in installs {
                let install_path = PathBuf::from(&install.install_path);
                let skills_path = install_path.join("skills");

                // Skip if already loaded
                if loaded_paths.contains(&install_path) {
                    continue;
                }

                // Only load if skills path exists
                if !skills_path.exists() {
                    tracing::debug!(
                        "Plugin skills path does not exist: {}",
                        skills_path.display()
                    );
                    continue;
                }

                let name = plugin_id
                    .split('@')
                    .next()
                    .unwrap_or(&plugin_id)
                    .to_string();

                loaded_paths.insert(install_path.clone());
                plugins.push(Plugin {
                    name,
                    path: install_path,
                    skills_path: Some(skills_path),
                    skills_paths: Vec::new(),
                });
            }
        }

        Ok(plugins)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn load_from_dir(
        &self,
        dir: &Path,
        plugins: &mut Vec<Plugin>,
        loaded_paths: &mut std::collections::HashSet<PathBuf>,
    ) -> Result<()> {
        // Try to find plugins in this directory
        // First, check if this directory itself is a plugin (has skills/ subdirectory)
        if Self::is_plugin_dir(dir) {
            if let Ok(plugin) = Self::load_plugin(dir) {
                if !loaded_paths.contains(&plugin.path) {
                    loaded_paths.insert(plugin.path.clone());
                    plugins.push(plugin);
                }
                return Ok(());
            }
        }

        // Otherwise, recursively scan subdirectories
        // This handles the structure: cache/{marketplace}/{plugin}/{version}/
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Check if this is a version directory (contains skills/)
                if Self::is_plugin_dir(&path) {
                    if let Ok(plugin) = Self::load_plugin(&path) {
                        if !loaded_paths.contains(&plugin.path) {
                            loaded_paths.insert(plugin.path.clone());
                            plugins.push(plugin);
                        }
                    }
                } else {
                    // Recurse into subdirectory
                    self.load_from_dir(&path, plugins, loaded_paths)?;
                }
            }
        }
        Ok(())
    }

    /// Check if a directory is a plugin directory (contains skills/ or has .claude-plugin/plugin.json)
    fn is_plugin_dir(path: &Path) -> bool {
        path.join("skills").exists() || Self::find_manifest_path(path).is_some()
    }

    /// Find plugin manifest in .claude-plugin/, .cursor-plugin/, or .codex-plugin/ subdirs
    fn find_manifest_path(path: &Path) -> Option<PathBuf> {
        for subdir in [".claude-plugin", ".cursor-plugin", ".codex-plugin"] {
            let manifest_path = path.join(subdir).join("plugin.json");
            if manifest_path.exists() {
                return Some(manifest_path);
            }
        }
        None
    }

    fn load_plugin(path: &Path) -> Result<Plugin> {
        // Try to find manifest first
        let manifest_path = Self::find_manifest_path(path);

        let (name, skills_path, skills_paths) = if let Some(ref manifest_path) = manifest_path {
            Self::load_plugin_with_manifest(path, manifest_path)?
        } else {
            let name = Self::derive_plugin_name(path);
            let (skills_path, skills_paths) = Self::load_plugin_without_manifest(path);
            (name, skills_path, skills_paths)
        };

        Ok(Plugin {
            name,
            path: path.to_path_buf(),
            skills_path,
            skills_paths,
        })
    }

    /// Derive a meaningful plugin name from the path
    /// For claude-code structure: cache/{marketplace}/{plugin}/{version}/ -> {plugin}
    fn derive_plugin_name(path: &Path) -> String {
        // Check if this looks like a claude-code plugin cache structure
        // cache/{marketplace}/{plugin}/{version}/
        let components: Vec<_> = path.components().collect();

        // Look for "cache" in the path to identify claude-code structure
        if let Some(cache_idx) = components.iter().position(|c| {
            if let std::path::Component::Normal(name) = c {
                name.to_str() == Some("cache")
            } else {
                false
            }
        }) {
            // We have: .../cache/{marketplace}/{plugin}/{version}/
            // Use just the plugin name (skip marketplace)
            if components.len() >= cache_idx + 3 {
                let plugin = components.get(cache_idx + 2).and_then(|c| {
                    if let std::path::Component::Normal(n) = c {
                        n.to_str()
                    } else {
                        None
                    }
                });

                if let Some(p) = plugin {
                    return p.to_string();
                }
            }
        }

        // Fallback: use the parent directory name or last component
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    }

    fn load_plugin_with_manifest(
        plugin_path: &Path,
        manifest_path: &Path,
    ) -> Result<(String, Option<PathBuf>, Vec<PathBuf>)> {
        let content = std::fs::read_to_string(manifest_path)?;
        let manifest: PluginManifest = serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse plugin manifest: {}",
                manifest_path.display()
            )
        })?;

        // Use name from manifest, or derive from path
        let name = manifest
            .name
            .unwrap_or_else(|| Self::derive_plugin_name(plugin_path));

        let mut skills_paths = Vec::new();

        if let Some(skills) = manifest.skills {
            let paths: Vec<String> = match skills {
                PluginSkills::Single(p) => vec![p],
                PluginSkills::Multiple(ps) => ps,
            };
            for p in paths {
                let full_path = plugin_path.join(&p);
                if full_path.exists() {
                    skills_paths.push(full_path);
                } else {
                    tracing::warn!("Plugin skills path does not exist: {}", full_path.display());
                }
            }
        }

        let default_skills_path = plugin_path.join("skills");
        let skills_path = if default_skills_path.exists() && skills_paths.is_empty() {
            Some(default_skills_path)
        } else {
            None
        };

        Ok((name, skills_path, skills_paths))
    }

    fn load_plugin_without_manifest(plugin_path: &Path) -> (Option<PathBuf>, Vec<PathBuf>) {
        let skills_path = plugin_path.join("skills");
        if skills_path.exists() {
            (Some(skills_path), Vec::new())
        } else {
            (None, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_load_plugin_without_manifest() {
        let temp = TempDir::new().unwrap();
        let plugin_dir = temp.path().join("test-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::create_dir(plugin_dir.join("skills")).unwrap();

        let loader = PluginLoader::new(vec![temp.path().to_path_buf()]);
        let plugins = loader.load_all().unwrap();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test-plugin");
        assert!(plugins[0].skills_path.is_some());
    }

    #[test]
    fn test_load_plugin_with_manifest() {
        let temp = TempDir::new().unwrap();
        let plugin_dir = temp.path().join("test-plugin");
        let claude_plugin_dir = plugin_dir.join(".claude-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::create_dir(&claude_plugin_dir).unwrap();
        std::fs::create_dir(plugin_dir.join("my-skills")).unwrap();

        let manifest = r#"{"name": "test-plugin", "skills": "my-skills"}"#;
        let mut file = std::fs::File::create(claude_plugin_dir.join("plugin.json")).unwrap();
        file.write_all(manifest.as_bytes()).unwrap();

        let loader = PluginLoader::new(vec![temp.path().to_path_buf()]);
        let plugins = loader.load_all().unwrap();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test-plugin");
        assert_eq!(plugins[0].skills_paths.len(), 1);
    }

    #[test]
    fn test_load_multiple_plugins() {
        let temp = TempDir::new().unwrap();

        // Create plugin A
        let plugin_a = temp.path().join("plugin-a");
        std::fs::create_dir(&plugin_a).unwrap();
        std::fs::create_dir(plugin_a.join("skills")).unwrap();

        // Create plugin B
        let plugin_b = temp.path().join("plugin-b");
        std::fs::create_dir(&plugin_b).unwrap();
        std::fs::create_dir(plugin_b.join("skills")).unwrap();

        let loader = PluginLoader::new(vec![temp.path().to_path_buf()]);
        let plugins = loader.load_all().unwrap();

        assert_eq!(plugins.len(), 2);
        let names: Vec<_> = plugins.iter().map(|p| p.name.clone()).collect();
        assert!(names.contains(&"plugin-a".to_string()));
        assert!(names.contains(&"plugin-b".to_string()));
    }

    #[test]
    fn test_skip_nonexistent_plugin_dir() {
        let loader = PluginLoader::new(vec![PathBuf::from("/nonexistent/path")]);
        let plugins = loader.load_all().unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_derive_plugin_name_from_cache_structure() {
        // Test claude-code cache structure - should only use plugin name, not marketplace
        let path = Path::new(
            "/home/user/.claude/plugins/cache/everything-claude-code/my-plugin/660e0d3badd3",
        );
        let name = PluginLoader::derive_plugin_name(path);
        assert_eq!(name, "my-plugin");

        // Another example with superpowers
        let path2 =
            Path::new("/home/user/.claude/plugins/cache/claude-plugins-official/superpowers/5.0.7");
        let name2 = PluginLoader::derive_plugin_name(path2);
        assert_eq!(name2, "superpowers");

        // Test simple structure (fallback)
        let simple_path = Path::new("/some/path/my-plugin");
        let simple_name = PluginLoader::derive_plugin_name(simple_path);
        assert_eq!(simple_name, "my-plugin");
    }

    #[test]
    fn test_load_plugin_from_nested_cache_structure() {
        let temp = TempDir::new().unwrap();

        // Create nested structure: cache/marketplace/plugin/version/skills/
        let cache_dir = temp.path().join("cache");
        let marketplace_dir = cache_dir.join("test-marketplace");
        let plugin_dir = marketplace_dir.join("test-plugin");
        let version_dir = plugin_dir.join("v1.0.0");
        let skills_dir = version_dir.join("skills");
        let skill_dir = skills_dir.join("hello-skill");

        std::fs::create_dir_all(&skill_dir).unwrap();

        // Create SKILL.md
        let skill_content = r"---
description: A hello world skill
---

Hello!";
        let mut file = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        file.write_all(skill_content.as_bytes()).unwrap();

        let loader = PluginLoader::new(vec![cache_dir]);
        let plugins = loader.load_all().unwrap();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test-plugin");
        assert!(plugins[0].skills_path.is_some());
    }
}

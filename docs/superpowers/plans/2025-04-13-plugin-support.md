# Plugin Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add simplified plugin system to yomi that can load skills from claude-code plugin cache directories.

**Architecture:** Add a `Plugin` struct to represent loaded plugins with their skill paths. Extend `SkillLoader` to scan plugin directories and load skills with namespaced format (`plugin:skill`). Reuse existing skill loading logic where possible.

**Tech Stack:** Rust, anyhow, serde, existing yomi kernel infrastructure

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/kernel/src/plugin.rs` | Plugin struct, PluginLoader, loading logic for plugin directories |
| `crates/kernel/src/skill.rs` | Extend SkillLoader to accept plugin-loaded skills, add derive_skill_name_from_plugin helper |
| `crates/kernel/src/config.rs` | Add plugin_dirs configuration option |
| `crates/kernel/src/lib.rs` | Export plugin module |
| `crates/cli/src/main.rs` | Load plugins and pass skills to coordinator |

---

### Task 1: Create plugin.rs module

**Files:**
- Create: `crates/kernel/src/plugin.rs`

- [ ] **Step 1: Define Plugin struct**

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A loaded plugin with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
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
```

- [ ] **Step 2: Implement PluginLoader**

```rust
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
    pub fn load_all(&self) -> Result<Vec<Plugin>> {
        let mut plugins = Vec::new();

        for dir in &self.plugin_dirs {
            if dir.exists() {
                self.load_from_dir(dir, &mut plugins)
                    .with_context(|| format!("Failed to load plugins from {}", dir.display()))?;
            } else {
                tracing::warn!("Plugin directory does not exist: {}", dir.display());
            }
        }

        Ok(plugins)
    }

    fn load_from_dir(&self, dir: &Path, plugins: &mut Vec<Plugin>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                if let Ok(plugin) = self.load_plugin(&path) {
                    plugins.push(plugin);
                }
            }
        }
        Ok(())
    }

    fn load_plugin(&self, path: &Path) -> Result<Plugin> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let manifest_path = path.join("plugin.json");
        let (skills_path, skills_paths) = if manifest_path.exists() {
            self.load_plugin_with_manifest(path, &manifest_path)?
        } else {
            self.load_plugin_without_manifest(path)?
        };

        Ok(Plugin {
            name,
            path: path.to_path_buf(),
            skills_path,
            skills_paths,
        })
    }

    fn load_plugin_with_manifest(
        &self,
        plugin_path: &Path,
        manifest_path: &Path,
    ) -> Result<(Option<PathBuf>, Vec<PathBuf>)> {
        let content = std::fs::read_to_string(manifest_path)?;
        let manifest: PluginManifest = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse plugin manifest: {}", manifest_path.display()))?;

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

        Ok((skills_path, skills_paths))
    }

    fn load_plugin_without_manifest(&self, plugin_path: &Path) -> Result<(Option<PathBuf>, Vec<PathBuf>)> {
        let skills_path = plugin_path.join("skills");
        if skills_path.exists() {
            Ok((Some(skills_path), Vec::new()))
        } else {
            Ok((None, Vec::new()))
        }
    }
}
```

- [ ] **Step 3: Add unit tests**

```rust
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
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::create_dir(plugin_dir.join("my-skills")).unwrap();

        let manifest = r#"{"name": "test-plugin", "skills": "my-skills"}"#;
        let mut file = std::fs::File::create(plugin_dir.join("plugin.json")).unwrap();
        file.write_all(manifest.as_bytes()).unwrap();

        let loader = PluginLoader::new(vec![temp.path().to_path_buf()]);
        let plugins = loader.load_all().unwrap();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test-plugin");
        assert_eq!(plugins[0].skills_paths.len(), 1);
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/kernel/src/plugin.rs
git commit -m "feat: add plugin module with PluginLoader"
```

---

### Task 2: Export plugin module from kernel

**Files:**
- Modify: `crates/kernel/src/lib.rs`

- [ ] **Step 1: Add plugin module export**

```rust
// Add after existing modules
pub mod plugin;
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check --package kernel
```

- [ ] **Step 3: Commit**

```bash
git add crates/kernel/src/lib.rs
git commit -m "feat: export plugin module from kernel"
```

---

### Task 3: Extend SkillLoader to support plugin skills

**Files:**
- Modify: `crates/kernel/src/skill.rs`

- [ ] **Step 1: Add method to load skills from a plugin**

```rust
use crate::plugin::Plugin;

impl SkillLoader {
    /// Load skills from a plugin
    pub fn load_from_plugin(&self, plugin: &Plugin) -> Result<Vec<Arc<Skill>>> {
        let mut skills = Vec::new();

        // Load from default skills path
        if let Some(ref skills_path) = plugin.skills_path {
            self.load_plugin_skills_dir(skills_path, &plugin.name, &mut skills)?;
        }

        // Load from additional skills paths
        for skills_path in &plugin.skills_paths {
            self.load_plugin_skills_dir(skills_path, &plugin.name, &mut skills)?;
        }

        Ok(skills)
    }

    fn load_plugin_skills_dir(
        &self,
        skills_path: &Path,
        plugin_name: &str,
        skills: &mut Vec<Arc<Skill>>,
    ) -> Result<()> {
        if !skills_path.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(skills_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    match self.load_plugin_skill(&skill_file, plugin_name) {
                        Ok(skill) => {
                            tracing::debug!(
                                "Loaded plugin skill '{}' from {}",
                                skill.name,
                                skill_file.display()
                            );
                            skills.push(Arc::new(skill));
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load plugin skill from {}: {}", skill_file.display(), e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn load_plugin_skill(&self, path: &Path, plugin_name: &str) -> Result<Skill> {
        let skill_name = self.derive_plugin_skill_name(path, plugin_name)?;
        
        use std::io::{BufRead, BufReader};

        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open skill file: {}", path.display()))?;
        let reader = BufReader::new(file);

        let mut lines = reader.lines();

        // Check if file starts with ---
        let first_line = lines.next().transpose()?;
        if first_line.as_deref() != Some("---") {
            anyhow::bail!("Skill file must start with frontmatter delimiter ---");
        }

        // Collect frontmatter lines until second ---
        let mut yaml_lines = Vec::new();
        let mut found_end = false;

        for line in lines {
            let line = line?;
            if line == "---" {
                found_end = true;
                break;
            }
            yaml_lines.push(line);
        }

        if !found_end {
            anyhow::bail!("Frontmatter end delimiter not found");
        }

        // Parse just the frontmatter YAML
        let yaml_content = yaml_lines.join("\n");
        let frontmatter: SkillFrontmatter = serde_yaml::from_str(&yaml_content)
            .context("Failed to parse skill frontmatter YAML")?;

        Ok(Skill {
            name: skill_name,
            description: frontmatter.description,
            triggers: frontmatter.triggers,
            source_path: path.to_path_buf(),
        })
    }

    fn derive_plugin_skill_name(&self, path: &Path, plugin_name: &str) -> Result<String> {
        let skill_dir = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid skill path: {}", path.display()))?;
        
        Ok(format!("{}:{}", plugin_name, skill_dir))
    }
}
```

- [ ] **Step 2: Add unit tests for plugin skill loading**

```rust
#[test]
fn test_derive_plugin_skill_name() {
    let loader = SkillLoader::new(vec![]);
    let path = Path::new("/plugins/my-plugin/skills/debugging/SKILL.md");
    let name = loader.derive_plugin_skill_name(path, "my-plugin").unwrap();
    assert_eq!(name, "my-plugin:debugging");
}

#[test]
fn test_load_plugin_skill() {
    use std::io::Write;
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("debugging");
    std::fs::create_dir(&skill_dir).unwrap();

    let skill_content = r"---
description: A debugging skill
triggers:
  - debug
---

# Debugging Skill

Content here.";

    let mut file = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
    file.write_all(skill_content.as_bytes()).unwrap();

    let loader = SkillLoader::new(vec![]);
    let skill = loader.load_plugin_skill(&skill_dir.join("SKILL.md"), "my-plugin").unwrap();

    assert_eq!(skill.name, "my-plugin:debugging");
    assert_eq!(skill.description, "A debugging skill");
    assert_eq!(skill.triggers, vec!["debug"]);
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/kernel/src/skill.rs
git commit -m "feat: extend SkillLoader to support plugin skills"
```

---

### Task 4: Update Config to support plugin_dirs

**Files:**
- Modify: `crates/kernel/src/config.rs`

- [ ] **Step 1: Add plugin_dirs field to Config**

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    // ... existing fields
    
    /// Plugin directories to load skills from
    #[serde(default)]
    pub plugin_dirs: Vec<PathBuf>,
}
```

- [ ] **Step 2: Update default config implementation**

```rust
impl Default for Config {
    fn default() -> Self {
        Self {
            // ... existing defaults
            plugin_dirs: vec![
                expand_tilde("~/.claude/plugins/cache"),
            ],
        }
    }
}
```

- [ ] **Step 3: Add env var support for plugin_dirs**

```rust
pub mod env_names {
    // ... existing constants
    pub const PLUGIN_DIRS: &str = "YOMI_PLUGIN_DIRS";
}
```

- [ ] **Step 4: Update from_env() to parse plugin_dirs**

```rust
pub fn from_env() -> Self {
    // ... existing env parsing
    
    let plugin_dirs = std::env::var(env_names::PLUGIN_DIRS)
        .map(|s| s.split(':').map(PathBuf::from).collect())
        .unwrap_or_else(|_| vec![
            expand_tilde("~/.claude/plugins/cache"),
        ]);

    Self {
        // ... existing fields
        plugin_dirs,
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/kernel/src/config.rs
git commit -m "feat: add plugin_dirs configuration support"
```

---

### Task 5: Update CLI to load plugin skills

**Files:**
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Import plugin types**

```rust
use kernel::{
    // ... existing imports
    plugin::{Plugin, PluginLoader},
};
```

- [ ] **Step 2: Load plugins and their skills**

```rust
// Load plugins from configured directories
let plugin_dirs = if config.plugin_dirs.is_empty() {
    vec![expand_tilde("~/.claude/plugins/cache")]
} else {
    config.plugin_dirs.clone()
};

tracing::debug!("Loading plugins from directories: {:?}", plugin_dirs);

let plugins: Vec<Plugin> = {
    let loader = PluginLoader::new(plugin_dirs);
    loader.load_all().unwrap_or_else(|e| {
        tracing::warn!("Failed to load plugins: {e}");
        Vec::new()
    })
};

// Log loaded plugins
if !plugins.is_empty() {
    tracing::info!("Loaded {} plugin(s)", plugins.len());
    for plugin in &plugins {
        tracing::info!("  - {} (from {})", plugin.name, plugin.path.display());
    }
}
```

- [ ] **Step 3: Load skills from plugins**

```rust
// Load regular skills
let skill_folders = if config.skill_folders.is_empty() {
    vec!["~/.yomi/skills".into(), "~/.claude/skills".into()]
} else {
    config.skill_folders.clone()
};

let mut skills: Vec<Arc<kernel::skill::Skill>> = {
    let loader = SkillLoader::new(skill_folders.iter().map(expand_tilde).collect());
    loader.load_all().unwrap_or_else(|e| {
        eprintln!("Warning: Failed to load skills: {e}");
        Vec::new()
    })
};

// Load plugin skills
for plugin in &plugins {
    let skill_loader = SkillLoader::new(vec![]);
    match skill_loader.load_from_plugin(plugin) {
        Ok(plugin_skills) => {
            for skill in plugin_skills {
                tracing::info!("  - {} (from plugin {})", skill.name, plugin.name);
                skills.push(skill);
            }
        }
        Err(e) => {
            tracing::warn!("Failed to load skills from plugin {}: {e}", plugin.name);
        }
    }
}

// Deduplicate skills by name (regular skills take precedence over plugin skills)
let mut seen_names = std::collections::HashSet::new();
skills.retain(|skill| {
    if seen_names.contains(&skill.name) {
        tracing::debug!(
            "Duplicate skill name '{}' found, keeping first instance.",
            skill.name
        );
        false
    } else {
        seen_names.insert(skill.name.clone());
        true
    }
});
```

- [ ] **Step 4: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "feat: load plugin skills in CLI"
```

---

### Task 6: Run all tests

**Files:**
- All modified files

- [ ] **Step 1: Run kernel tests**

```bash
cargo test --package kernel
```

Expected: All tests pass

- [ ] **Step 2: Run CLI tests**

```bash
cargo test --package cli
```

Expected: All tests pass

- [ ] **Step 3: Check formatting**

```bash
cargo fmt --all -- --check
```

Expected: No formatting issues

- [ ] **Step 4: Run clippy**

```bash
cargo clippy --all-targets --all-features
```

Expected: No warnings

- [ ] **Step 5: Commit**

```bash
git commit -m "test: verify plugin support implementation"
```

---

## Verification Steps

After implementing all tasks:

1. Create a test plugin:
   ```bash
   mkdir -p ~/.claude/plugins/cache/test-plugin/skills/hello
   cat > ~/.claude/plugins/cache/test-plugin/skills/hello/SKILL.md << 'EOF'
   ---
   description: A hello world skill
   ---
   
   Say hello!
   EOF
   ```

2. Run yomi and verify the skill is loaded:
   ```bash
   cargo run
   # Check logs for: "Loaded plugin skill 'test-plugin:hello'"
   ```

3. Verify skill is available in the system prompt

---

## Summary

This plan adds:
- `Plugin` and `PluginLoader` for scanning plugin directories
- `PluginManifest` parsing for plugin.json
- Extended `SkillLoader` to load skills from plugins with `plugin:skill` namespacing
- Configuration support for `plugin_dirs`
- CLI integration to load and merge plugin skills with regular skills

use crate::tools::{FileTool, Tool};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;

pub const GLOB_TOOL_NAME: &str = "glob";
pub const MAX_RESULTS: usize = 100;

pub struct GlobTool {
    base_dir: PathBuf,
}

impl GlobTool {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Build a globset from pattern
    fn build_glob_matcher(pattern: &str) -> Result<Option<globset::GlobSet>> {
        if pattern.is_empty() || pattern == "**/*" {
            return Ok(None);
        }

        // Handle multiple patterns
        let patterns: Vec<&str> = pattern
            .split([',', ' '])
            .filter(|s| !s.is_empty())
            .collect();

        let mut builder = globset::GlobSetBuilder::new();
        for pat in patterns {
            let glob = globset::Glob::new(pat)
                .map_err(|e| anyhow::anyhow!("Invalid glob pattern '{pat}': {e}"))?;
            builder.add(glob);
        }

        let set = builder
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build glob matcher: {e}"))?;

        Ok(Some(set))
    }

    /// Search files using ignore crate with proper glob matching
    async fn search_files(
        &self,
        search_dir: PathBuf,
        pattern: String,
        no_ignore: bool,
        include_hidden: bool,
        limit: usize,
    ) -> Result<Vec<PathBuf>> {
        let matcher = Self::build_glob_matcher(&pattern)?;

        let files = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();

            let walker = ignore::WalkBuilder::new(&search_dir)
                .standard_filters(!no_ignore)
                .hidden(!include_hidden)
                .follow_links(false)
                .build();

            for entry in walker {
                let Ok(entry) = entry else {
                    continue;
                };

                if let Some(file_type) = entry.file_type() {
                    if file_type.is_file() {
                        let path = entry.path();

                        // Apply glob pattern matching
                        if let Some(ref m) = matcher {
                            // Get relative path from search_dir for matching
                            let relative_path = path
                                .strip_prefix(&search_dir)
                                .unwrap_or(path)
                                .to_string_lossy();

                            if !m.is_match(&*relative_path) {
                                continue;
                            }
                        }

                        files.push(path.to_path_buf());
                    }
                }
            }

            files
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {e}"))?;

        // Get modification times concurrently and sort by mtime (newest first)
        let mtime_futures: Vec<_> = files
            .into_iter()
            .map(|file_path| async move {
                let mtime = self.get_mtime(&file_path).await;
                (file_path, mtime)
            })
            .collect();

        let mut files_with_mtime: Vec<(PathBuf, u64)> =
            futures::future::join_all(mtime_futures).await;

        // Sort by mtime descending (newest first), then by path for deterministic order
        files_with_mtime.sort_by(|a, b| {
            b.1.cmp(&a.1) // Descending by mtime
                .then_with(|| a.0.cmp(&b.0)) // Ascending by path as tiebreaker
        });

        // Limit results
        let result: Vec<PathBuf> = files_with_mtime
            .into_iter()
            .take(limit)
            .map(|(path, _)| path)
            .collect();

        Ok(result)
    }
}

impl FileTool for GlobTool {
    fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}
#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        GLOB_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Find files matching a glob pattern. Supports patterns like '**/*.rs' or 'src/**/*.ts'. Respects .gitignore files by default."
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against (e.g., '**/*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in. If not specified, the current working directory will be used."
                },
                "no_ignore": {
                    "type": "boolean",
                    "description": "Whether to ignore .gitignore files. Default: false"
                },
                "hidden": {
                    "type": "boolean",
                    "description": "Whether to include hidden files (starting with .). Default: false"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn exec(&self, args: Value) -> Result<ToolOutput> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;
        let path = args["path"].as_str();
        let no_ignore = args["no_ignore"].as_bool().unwrap_or(false);
        let include_hidden = args["hidden"].as_bool().unwrap_or(false);

        // Determine search directory
        let search_dir = match path {
            Some(p) => self.resolve_path(p),
            None => self.base_dir.clone(),
        };

        // Validate directory exists
        if !tokio::fs::try_exists(&search_dir).await? {
            return Ok(ToolOutput::new_err(format!(
                "Directory does not exist: {}",
                path.unwrap_or(".")
            )));
        }

        if !tokio::fs::metadata(&search_dir).await?.is_dir() {
            return Ok(ToolOutput::new_err(format!(
                "Path is not a directory: {}",
                path.unwrap_or(".")
            )));
        }

        tracing::debug!(
            "Glob: searching for '{}' in {}",
            pattern,
            search_dir.display()
        );

        // Search files using ignore crate
        let files = self
            .search_files(
                search_dir,
                pattern.to_string(),
                no_ignore,
                include_hidden,
                MAX_RESULTS,
            )
            .await?;

        let truncated = files.len() >= MAX_RESULTS;
        let total_files = files.len();

        // Convert to relative paths
        let filenames: Vec<String> = files
            .into_iter()
            .map(|path| {
                path.strip_prefix(&self.base_dir).map_or_else(|_| path.to_string_lossy().to_string(), |p| p.to_string_lossy().to_string())
            })
            .collect();

        // Build response
        let mut response = if filenames.is_empty() {
            "No files found".to_string()
        } else {
            filenames.join("\n")
        };

        if truncated {
            response.push_str(
                "\n\n(Results are truncated. Consider using a more specific path or pattern.)",
            );
        }

        let summary = if filenames.is_empty() {
            String::new()
        } else {
            format!(
                "Found {} file{}",
                total_files,
                if total_files == 1 { "" } else { "s" }
            )
        };

        Ok(ToolOutput::new(response, &summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_glob_tool_basic() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let mut file1 = std::fs::File::create(base_path.join("test1.rs")).unwrap();
        writeln!(file1, "content").unwrap();

        let mut file2 = std::fs::File::create(base_path.join("test2.rs")).unwrap();
        writeln!(file2, "content").unwrap();

        std::fs::File::create(base_path.join("test.txt")).unwrap();

        let tool = GlobTool::new(base_path);
        let args = serde_json::json!({
            "pattern": "*.rs"
        });

        let result = tool.exec(args).await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("test1.rs"));
        assert!(result.stdout.contains("test2.rs"));
        assert!(!result.stdout.contains("test.txt"));
    }

    #[tokio::test]
    async fn test_glob_tool_recursive() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let sub_dir = base_path.join("src");
        std::fs::create_dir(&sub_dir).unwrap();

        let mut file = std::fs::File::create(sub_dir.join("main.rs")).unwrap();
        writeln!(file, "content").unwrap();

        let tool = GlobTool::new(base_path);
        let args = serde_json::json!({
            "pattern": "**/*.rs"
        });

        let result = tool.exec(args).await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("src/main.rs"));
    }

    #[tokio::test]
    async fn test_glob_tool_respects_gitignore() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Initialize git repo
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(base_path)
            .output()
            .expect("Failed to init git repo");

        let mut file1 = std::fs::File::create(base_path.join("tracked.rs")).unwrap();
        writeln!(file1, "content").unwrap();

        let target_dir = base_path.join("target");
        std::fs::create_dir(&target_dir).unwrap();
        let mut file2 = std::fs::File::create(target_dir.join("ignored.rs")).unwrap();
        writeln!(file2, "content").unwrap();

        let mut gitignore = std::fs::File::create(base_path.join(".gitignore")).unwrap();
        writeln!(gitignore, "target/").unwrap();

        let tool = GlobTool::new(base_path);
        let args = serde_json::json!({
            "pattern": "**/*.rs"
        });

        let result = tool.exec(args).await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("tracked.rs"));
        assert!(!result.stdout.contains("target/ignored.rs"));
    }

    #[tokio::test]
    async fn test_glob_tool_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = GlobTool::new(base_path);
        let args = serde_json::json!({
            "pattern": "*.nonexistent"
        });

        let result = tool.exec(args).await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("No files found"));
    }

    #[tokio::test]
    async fn test_glob_tool_with_path() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let sub_dir = base_path.join("src");
        std::fs::create_dir(&sub_dir).unwrap();

        let mut file = std::fs::File::create(sub_dir.join("main.rs")).unwrap();
        writeln!(file, "content").unwrap();

        let tool = GlobTool::new(base_path);
        let args = serde_json::json!({
            "pattern": "*.rs",
            "path": "src"
        });

        let result = tool.exec(args).await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("main.rs"));
    }

    #[tokio::test]
    async fn test_glob_tool_nonexistent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = GlobTool::new(base_path);
        let args = serde_json::json!({
            "pattern": "*.rs",
            "path": "nonexistent"
        });

        let result = tool.exec(args).await.unwrap();
        assert!(!result.success());
        assert!(result.stderr.contains("does not exist"));
    }

    #[tokio::test]
    async fn test_glob_tool_hidden_files() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create hidden file
        let mut hidden = std::fs::File::create(base_path.join(".hidden.rs")).unwrap();
        writeln!(hidden, "content").unwrap();

        // Create normal file
        let mut normal = std::fs::File::create(base_path.join("normal.rs")).unwrap();
        writeln!(normal, "content").unwrap();

        let tool = GlobTool::new(base_path);

        // Without hidden flag - should not include .hidden.rs
        let args = serde_json::json!({
            "pattern": "*.rs"
        });
        let result = tool.exec(args).await.unwrap();
        assert!(result.success());
        assert!(!result.stdout.contains(".hidden.rs"));
        assert!(result.stdout.contains("normal.rs"));

        // With hidden flag - should include .hidden.rs
        let args = serde_json::json!({
            "pattern": "*.rs",
            "hidden": true
        });
        let result = tool.exec(args).await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains(".hidden.rs"));
    }
}

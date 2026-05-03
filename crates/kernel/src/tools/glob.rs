use crate::tools::base::get_mtimes_concurrent;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

pub const GLOB_TOOL_NAME: &str = "glob";
pub const MAX_RESULTS: usize = 100;

pub struct GlobTool;

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobTool {
    pub fn new() -> Self {
        Self
    }

    /// Build glob matcher for pattern
    fn build_matcher(pattern: &str) -> Result<globset::GlobMatcher> {
        let glob = globset::Glob::new(pattern)
            .map_err(|e| KernelError::tool(format!("Invalid glob pattern '{pattern}': {e}")))?;

        Ok(glob.compile_matcher())
    }

    /// Search files using ignore crate with proper glob matching
    async fn search_files(
        &self,
        search_dir: PathBuf,
        pattern: String,
        include_ignored: bool,
        include_hidden: bool,
        limit: usize,
    ) -> Result<Vec<PathBuf>> {
        let matcher = Self::build_matcher(&pattern)?;

        let files = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();

            let walker = ignore::WalkBuilder::new(&search_dir)
                .standard_filters(!include_ignored)
                .hidden(!include_hidden)
                .follow_links(false)
                .filter_entry(move |e| {
                    if include_ignored {
                        true
                    } else {
                        !e.path().components().any(|c| {
                            let name = c.as_os_str();
                            name == ".git" || name == ".jj"
                        })
                    }
                })
                .build();

            for entry in walker {
                let Ok(entry) = entry else {
                    continue;
                };

                if let Some(file_type) = entry.file_type() {
                    if file_type.is_file() {
                        let path = entry.path();

                        // Apply glob pattern matching
                        // Get relative path from search_dir for matching
                        let relative_path = path
                            .strip_prefix(&search_dir)
                            .unwrap_or(path)
                            .to_string_lossy();

                        if !matcher.is_match(&*relative_path) {
                            continue;
                        }

                        files.push(path.to_path_buf());
                    }
                }
            }

            files
        })
        .await
        .map_err(|e| KernelError::tool(format!("Task join error: {e}")))?;

        // Get modification times concurrently with limited concurrency
        // to avoid file descriptor exhaustion on large directories
        let mut files_with_mtime: Vec<(PathBuf, u64)> = get_mtimes_concurrent(files, None).await;

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

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        GLOB_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Find files matching a glob pattern. Supports patterns like '**/*.rs' or 'src/**/*.ts'. Respects .gitignore files by default."
    }

    fn schema(&self) -> Value {
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
                "include_ignored": {
                    "type": "boolean",
                    "description": "Whether to include files ignored by .gitignore. Default: false"
                },
                "include_hidden": {
                    "type": "boolean",
                    "description": "Whether to include hidden files (starting with .). Default: true"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'pattern' argument"))?;
        let path = args["path"].as_str();
        let include_ignored = args["include_ignored"].as_bool().unwrap_or(false);
        let include_hidden = args["include_hidden"].as_bool().unwrap_or(true);

        // Determine search directory
        let search_dir = match path {
            Some(p) => ctx.working_dir.join(p),
            None => ctx.working_dir.clone(),
        };

        // Validate directory exists
        if !tokio::fs::try_exists(&search_dir).await? {
            return Ok(ToolOutput::error(format!(
                "Directory does not exist: {}",
                path.unwrap_or(".")
            )));
        }

        if !tokio::fs::metadata(&search_dir).await?.is_dir() {
            return Ok(ToolOutput::error(format!(
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
                include_ignored,
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
                path.strip_prefix(&ctx.working_dir).map_or_else(
                    |_| path.to_string_lossy().to_string(),
                    |p| p.to_string_lossy().to_string(),
                )
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

        Ok(ToolOutput::text_with_summary(response, &summary))
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

        let tool = GlobTool::new();
        let args = serde_json::json!({
            "pattern": "*.rs"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("test1.rs"));
        assert!(result.text_content().contains("test2.rs"));
        assert!(!result.text_content().contains("test.txt"));
    }

    #[tokio::test]
    async fn test_glob_tool_recursive() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let sub_dir = base_path.join("src");
        std::fs::create_dir(&sub_dir).unwrap();

        let mut file = std::fs::File::create(sub_dir.join("main.rs")).unwrap();
        writeln!(file, "content").unwrap();

        let tool = GlobTool::new();
        let args = serde_json::json!({
            "pattern": "**/*.rs"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("src/main.rs"));
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

        let tool = GlobTool::new();
        let args = serde_json::json!({
            "pattern": "**/*.rs"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("tracked.rs"));
        assert!(!result.text_content().contains("target/ignored.rs"));
    }

    #[tokio::test]
    async fn test_glob_tool_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = GlobTool::new();
        let args = serde_json::json!({
            "pattern": "*.nonexistent"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("No files found"));
    }

    #[tokio::test]
    async fn test_glob_tool_with_path() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let sub_dir = base_path.join("src");
        std::fs::create_dir(&sub_dir).unwrap();

        let mut file = std::fs::File::create(sub_dir.join("main.rs")).unwrap();
        writeln!(file, "content").unwrap();

        let tool = GlobTool::new();
        let args = serde_json::json!({
            "pattern": "*.rs",
            "path": "src"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("main.rs"));
    }

    #[tokio::test]
    async fn test_glob_tool_nonexistent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = GlobTool::new();
        let args = serde_json::json!({
            "pattern": "*.rs",
            "path": "nonexistent"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.error_text().contains("does not exist"));
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

        let tool = GlobTool::new();

        // Without include_hidden flag (default true) - should include .hidden.rs
        let args = serde_json::json!({
            "pattern": "*.rs"
        });
        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains(".hidden.rs"));
        assert!(result.text_content().contains("normal.rs"));

        // With include_hidden: false - should not include .hidden.rs
        let args = serde_json::json!({
            "pattern": "*.rs",
            "include_hidden": false
        });
        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(!result.text_content().contains(".hidden.rs"));
        assert!(result.text_content().contains("normal.rs"));
    }

    #[tokio::test]
    async fn test_glob_tool_brace_expansion() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create files with different extensions
        let mut file1 = std::fs::File::create(base_path.join("test.rs")).unwrap();
        writeln!(file1, "content").unwrap();

        let mut file2 = std::fs::File::create(base_path.join("test.ts")).unwrap();
        writeln!(file2, "content").unwrap();

        let mut file3 = std::fs::File::create(base_path.join("test.js")).unwrap();
        writeln!(file3, "content").unwrap();

        std::fs::File::create(base_path.join("test.txt")).unwrap();

        let tool = GlobTool::new();
        // Use brace expansion to match multiple extensions
        let args = serde_json::json!({
            "pattern": "*.{rs,ts,js}"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(
            result.text_content().contains("test.rs"),
            "Should match .rs files"
        );
        assert!(
            result.text_content().contains("test.ts"),
            "Should match .ts files"
        );
        assert!(
            result.text_content().contains("test.js"),
            "Should match .js files"
        );
        assert!(
            !result.text_content().contains("test.txt"),
            "Should not match .txt files"
        );
    }
}

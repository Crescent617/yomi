use crate::tools::helper::get_mtimes_concurrent;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::path::expand_tilde;
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
            Some(p) => {
                let p = expand_tilde(p);
                if p.is_absolute() {
                    p
                } else {
                    ctx.working_dir.join(p)
                }
            }
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
#[path = "glob_test.rs"]
mod tests;

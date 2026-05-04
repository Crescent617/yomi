use crate::tools::base::{get_mtime, get_mtimes_concurrent};
use crate::tools::file_state::FileStateStore;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::rg_helper::parse_json_output;
use async_trait::async_trait;
use serde_json::Value;

use std::fmt::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

pub const GREP_TOOL_NAME: &str = "grep";
const DEFAULT_HEAD_LIMIT: usize = 250;
const RIPGREP_TIMEOUT: Duration = Duration::from_secs(30);
const TRUNCATED_MSG: &str = "\n\n(Results are truncated. Consider using a more specific pattern or increase limit.)";

pub struct GrepTool {
    file_state_store: Option<Arc<FileStateStore>>,
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepTool {
    pub fn new() -> Self {
        Self {
            file_state_store: None,
        }
    }

    /// Set the file state store for tracking reads
    #[must_use]
    pub fn with_file_state_store(mut self, store: Arc<FileStateStore>) -> Self {
        self.file_state_store = Some(store);
        self
    }

    /// Build ripgrep command arguments
    #[allow(clippy::too_many_arguments)]
    fn build_rg_args(
        pattern: &str,
        output_mode: &str,
        context_before: usize,
        context_after: usize,
        show_line_numbers: bool,
        case_insensitive: bool,
        multiline: bool,
        glob_pattern: Option<&str>,
        file_type: Option<&str>,
    ) -> Vec<String> {
        let mut args = Vec::new();

        // Always include hidden files
        args.push("--hidden".to_string());

        // Line length limit to prevent noise from minified files
        args.push("--max-columns".to_string());
        args.push("500".to_string());

        // Multiline mode
        if multiline {
            args.push("-U".to_string());
            args.push("--multiline-dotall".to_string());
        }

        // Case insensitive
        if case_insensitive {
            args.push("-i".to_string());
        }

        // Output mode flags
        match output_mode {
            "files_with_matches" => {
                args.push("-l".to_string());
            }
            "count" => {
                args.push("-c".to_string());
            }
            _ => {
                // content mode - use JSON for structured parsing
                args.push("--json".to_string());
                if show_line_numbers {
                    args.push("-n".to_string());
                }
            }
        }

        // Context lines (only for content mode)
        if output_mode == "content" && (context_before > 0 || context_after > 0) {
            // Use -C if both are same, otherwise use -B and -A
            if context_before == context_after {
                args.push("-C".to_string());
                args.push(context_before.to_string());
            } else {
                if context_before > 0 {
                    args.push("-B".to_string());
                    args.push(context_before.to_string());
                }
                if context_after > 0 {
                    args.push("-A".to_string());
                    args.push(context_after.to_string());
                }
            }
        }

        // File type filter
        if let Some(ft) = file_type {
            args.push("--type".to_string());
            args.push(ft.to_string());
        }

        // Glob pattern filter - split on spaces but preserve braces
        if let Some(glob) = glob_pattern {
            let glob_patterns = Self::parse_glob_patterns(glob);
            for pat in glob_patterns {
                if !pat.is_empty() {
                    args.push("--glob".to_string());
                    args.push(pat);
                }
            }
        }

        // Exclude VCS directories
        args.push("--glob".to_string());
        args.push("!.git".to_string());
        args.push("--glob".to_string());
        args.push("!.svn".to_string());
        args.push("--glob".to_string());
        args.push("!.hg".to_string());

        // Pattern - if it starts with -, use -e to avoid interpretation as flag
        if pattern.starts_with('-') {
            args.push("-e".to_string());
        }
        args.push(pattern.to_string());

        args
    }

    /// Parse glob patterns - split on spaces but preserve patterns with braces
    fn parse_glob_patterns(glob: &str) -> Vec<String> {
        let mut patterns = Vec::new();
        let raw_patterns: Vec<&str> = glob.split_whitespace().collect();

        for raw_pattern in raw_patterns {
            // If pattern contains braces, don't split further
            if raw_pattern.contains('{') && raw_pattern.contains('}') {
                patterns.push(raw_pattern.to_string());
            } else {
                // Split on commas for patterns without braces
                patterns.extend(
                    raw_pattern
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string()),
                );
            }
        }

        patterns.into_iter().filter(|s| !s.is_empty()).collect()
    }

    /// Apply offset and limit to a collection of items
    fn apply_pagination<T>(items: &[T], limit: usize, offset: usize) -> (Vec<T>, bool)
    where
        T: Clone,
    {
        let skip = offset.min(items.len());
        let take = if limit == 0 {
            items.len() - skip
        } else {
            (items.len() - skip).min(limit)
        };

        let was_truncated = items.len() - skip > limit && limit > 0;
        let limited: Vec<T> = items.iter().skip(skip).take(take).cloned().collect();

        (limited, was_truncated)
    }

    /// Run ripgrep and return output
    async fn run_ripgrep(
        &self,
        args: Vec<String>,
        search_path: &PathBuf,
        working_dir: &std::path::Path,
    ) -> Result<(String, String, i32)> {
        let mut cmd = Command::new("rg");
        cmd.args(&args)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Add search path as final argument
        cmd.arg(search_path);

        tracing::debug!("Running ripgrep: rg {}", args.join(" "));

        let output_result = timeout(RIPGREP_TIMEOUT, cmd.output()).await.map_err(|_| {
            KernelError::tool(format!(
                "ripgrep timed out after {} seconds",
                RIPGREP_TIMEOUT.as_secs()
            ))
        })?;

        let output = output_result?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // ripgrep exit codes:
        // 0 = matches found
        // 1 = no matches
        // 2 = error
        let code = output.status.code().unwrap_or(-1);

        if code == 2 && !stderr.is_empty() {
            return Err(KernelError::tool(format!("ripgrep error: {stderr}")));
        }

        Ok((stdout, stderr, code))
    }

    /// Get file modification time in milliseconds since epoch
    /// Format `files_with_matches` output with sorting by mtime
    async fn format_files_output(
        &self,
        stdout: &str,
        limit: usize,
        offset: usize,
        working_dir: &std::path::Path,
    ) -> String {
        let lines: Vec<&str> = stdout.lines().collect();

        if lines.is_empty() {
            return "No files found".to_string();
        }

        // Parse file paths and get modification times concurrently with limited concurrency
        // to avoid file descriptor exhaustion when there are many matches
        let paths: Vec<PathBuf> = lines.into_iter().map(PathBuf::from).collect();
        let mut files_with_mtime: Vec<(PathBuf, u64)> = get_mtimes_concurrent(paths, None).await;

        // Sort by mtime descending (newest first), then by path ascending as tiebreaker
        files_with_mtime.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        // Convert to relative paths
        let sorted_paths: Vec<String> = files_with_mtime
            .into_iter()
            .map(|(path, _)| {
                path.strip_prefix(working_dir).map_or_else(
                    |_| path.to_string_lossy().to_string(),
                    |p| p.to_string_lossy().to_string(),
                )
            })
            .collect();

        // Apply offset and limit
        let (limited, was_truncated) = Self::apply_pagination(&sorted_paths, limit, offset);

        let mut result = if limited.is_empty() {
            "No files found".to_string()
        } else {
            limited.join("\n")
        };

        if was_truncated {
            result.push_str(TRUNCATED_MSG);
        }

        result
    }

    /// Format count output with pagination
    fn format_count_output(stdout: &str, limit: usize, offset: usize) -> String {
        let lines: Vec<&str> = stdout.lines().collect();

        if lines.is_empty() {
            return "No matches found".to_string();
        }

        let mut total_matches = 0;
        let mut file_count = 0;

        for line in &lines {
            if let Some(colon_pos) = line.rfind(':') {
                if let Ok(count) = line[colon_pos + 1..].parse::<usize>() {
                    total_matches += count;
                    file_count += 1;
                }
            }
        }

        // Apply offset and limit
        let (limited, was_truncated) = Self::apply_pagination(&lines, limit, offset);

        let mut result = limited.join("\n");

        write!(
            result,
            "\n\nFound {total_matches} total {} across {file_count} {}",
            if total_matches == 1 {
                "occurrence"
            } else {
                "occurrences"
            },
            if file_count == 1 { "file" } else { "files" }
        )
        .unwrap();

        if was_truncated {
            result.push_str(TRUNCATED_MSG);
        }

        result
    }

    /// Process content mode output using JSON parsing
    /// Returns formatted output and the list of files that were displayed
    fn process_content_output(
        stdout: &str,
        limit: usize,
        offset: usize,
        show_line_numbers: bool,
    ) -> (String, Vec<PathBuf>) {
        let parsed = parse_json_output(stdout);

        if parsed.is_empty() {
            return ("No matches found".to_string(), Vec::new());
        }

        let was_truncated = parsed.paginate(limit, offset).1;
        let mut result = parsed.format_paginated(limit, offset, show_line_numbers);

        if was_truncated {
            result.push_str(TRUNCATED_MSG);
        }

        (result, parsed.unique_files_paginated(limit, offset))
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        GREP_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Search file contents using regex patterns (powered by ripgrep). Supports various output modes, context lines, and file filtering. Respects .gitignore by default. Always searches hidden files."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression pattern to search for in file contents"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in. Defaults to current working directory."
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., '*.js', '*.{ts,tsx}') - maps to rg --glob"
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Output mode: 'content' shows matching lines, 'files_with_matches' shows file paths, 'count' shows match counts. Defaults to 'files_with_matches'."
                },
                "-B": {
                    "type": "integer",
                    "description": "Number of lines to show before each match (rg -B). Requires output_mode: 'content', ignored otherwise."
                },
                "-A": {
                    "type": "integer",
                    "description": "Number of lines to show after each match (rg -A). Requires output_mode: 'content', ignored otherwise."
                },
                "-C": {
                    "type": "integer",
                    "description": "Alias for context."
                },
                "context": {
                    "type": "integer",
                    "description": "Number of lines to show before and after each match (rg -C). Requires output_mode: 'content', ignored otherwise."
                },
                "-n": {
                    "type": "boolean",
                    "description": "Show line numbers in output (rg -n). Requires output_mode: 'content', ignored otherwise. Defaults to true."
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case insensitive search (rg -i)"
                },
                "type": {
                    "type": "string",
                    "description": "File type to search (rg --type). Common types: js, py, rust, go, java, etc."
                },
                "limit": {
                    "type": "integer",
                    "description": "Limit output to first N lines/entries. Defaults to 250 when unspecified. Pass 0 for unlimited."
                },
                "offset": {
                    "type": "integer",
                    "description": "Skip first N lines/entries before applying limit. Defaults to 0."
                },
                "multiline": {
                    "type": "boolean",
                    "description": "Enable multiline mode where . matches newlines and patterns can span lines (rg -U --multiline-dotall). Default: false."
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
        let glob_pattern = args["glob"].as_str();
        let file_type = args["type"].as_str();
        let output_mode = args["output_mode"].as_str().unwrap_or("files_with_matches");
        let context_before = args["-B"].as_u64().map_or(0, |n| n as usize);
        let context_after = args["-A"].as_u64().map_or(0, |n| n as usize);
        let context = args["-C"]
            .as_u64()
            .or_else(|| args["context"].as_u64())
            .map(|n| n as usize);
        let show_line_numbers = args["-n"].as_bool().unwrap_or(true);
        let case_insensitive = args["-i"].as_bool().unwrap_or(false);
        let limit = args["limit"].as_u64().map(|n| n as usize);
        let offset = args["offset"].as_u64().map_or(0, |n| n as usize);
        let multiline = args["multiline"].as_bool().unwrap_or(false);

        // Use context if specified (overrides -B/-A)
        let (ctx_before, ctx_after) = if let Some(c) = context {
            (c, c)
        } else {
            (context_before, context_after)
        };

        // Determine search path
        let search_path = match path {
            Some(p) => ctx.working_dir.join(p),
            None => ctx.working_dir.clone(),
        };

        // Validate path exists
        if !tokio::fs::try_exists(&search_path).await? {
            return Ok(ToolOutput::error(format!(
                "Path does not exist: {}",
                path.unwrap_or(".")
            )));
        }

        // Build ripgrep arguments
        let rg_args = Self::build_rg_args(
            pattern,
            output_mode,
            ctx_before,
            ctx_after,
            show_line_numbers,
            case_insensitive,
            multiline,
            glob_pattern,
            file_type,
        );

        tracing::debug!("Running ripgrep with args: {:?}", rg_args);

        // Run ripgrep
        let (stdout, stderr, code) = self
            .run_ripgrep(rg_args, &search_path, &ctx.working_dir)
            .await?;

        // Handle different output modes
        let response = if code == 0 || code == 1 {
            // code 0 = matches found, code 1 = no matches (not an error)
            match output_mode {
                "files_with_matches" => {
                    self.format_files_output(
                        &stdout,
                        limit.unwrap_or(DEFAULT_HEAD_LIMIT),
                        offset,
                        &ctx.working_dir,
                    )
                    .await
                }
                "count" => {
                    Self::format_count_output(&stdout, limit.unwrap_or(DEFAULT_HEAD_LIMIT), offset)
                }
                _ => {
                    // content mode - use JSON parsing for accurate pagination and file tracking
                    let (response, displayed_files) = Self::process_content_output(
                        &stdout,
                        limit.unwrap_or(DEFAULT_HEAD_LIMIT),
                        offset,
                        show_line_numbers,
                    );

                    // Record only the files that were actually displayed (after pagination)
                    if let Some(ref store) = self.file_state_store {
                        for file_path in displayed_files {
                            // Convert relative paths to absolute for recording
                            let absolute_path = if file_path.is_absolute() {
                                file_path
                            } else {
                                ctx.working_dir.join(file_path)
                            };
                            let mtime = get_mtime(&absolute_path).await;
                            store.record(absolute_path, mtime);
                        }
                    }

                    response
                }
            }
        } else {
            // Unexpected exit code - return stderr as error
            return Ok(ToolOutput::error(format!(
                "ripgrep exited with code {code}: {stderr}"
            )));
        };

        Ok(ToolOutput::text_with_summary(response, &stderr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_grep_tool_files_with_matches() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(
            base_path.join("test1.rs"),
            "fn main() {\n    println!(\"hello\");\n}",
        )
        .await
        .unwrap();
        tokio::fs::write(
            base_path.join("test2.rs"),
            "fn foo() {\n    println!(\"world\");\n}",
        )
        .await
        .unwrap();
        tokio::fs::write(base_path.join("test.txt"), "just text")
            .await
            .unwrap();

        let tool = GrepTool::new();
        let args = serde_json::json!({
            "pattern": "println!",
            "output_mode": "files_with_matches"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("test1.rs"));
        assert!(result.text_content().contains("test2.rs"));
        assert!(!result.text_content().contains("test.txt"));
    }

    #[tokio::test]
    async fn test_grep_tool_content_mode() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(
            base_path.join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}",
        )
        .await
        .unwrap();

        let tool = GrepTool::new();
        let args = serde_json::json!({
            "pattern": "println!",
            "output_mode": "content"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("println"));
    }

    #[tokio::test]
    async fn test_grep_tool_case_insensitive() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.rs"), "fn MAIN() {}")
            .await
            .unwrap();

        let tool = GrepTool::new();
        let args = serde_json::json!({
            "pattern": "main",
            "output_mode": "content",
            "-i": true
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("MAIN"));
    }

    #[tokio::test]
    async fn test_grep_tool_glob_filter() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.rs"), "fn main() {}")
            .await
            .unwrap();
        tokio::fs::write(base_path.join("test.js"), "function main() {}")
            .await
            .unwrap();

        let tool = GrepTool::new();
        let args = serde_json::json!({
            "pattern": "main",
            "glob": "*.rs"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("test.rs"));
        assert!(!result.text_content().contains("test.js"));
    }

    #[tokio::test]
    async fn test_grep_tool_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.rs"), "fn main() {}")
            .await
            .unwrap();

        let tool = GrepTool::new();
        let args = serde_json::json!({
            "pattern": "nonexistent",
            "output_mode": "files_with_matches"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("No files found"));
    }

    #[tokio::test]
    async fn test_grep_tool_context_lines() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(
            base_path.join("test.rs"),
            "line 1\nline 2\nfn main() {\nline 4\nline 5\n}",
        )
        .await
        .unwrap();

        let tool = GrepTool::new();
        let args = serde_json::json!({
            "pattern": "fn main",
            "output_mode": "content",
            "-B": 2,
            "-A": 2
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        let content = result.text_content();
        println!("Content output:\n{content}");
        assert!(content.contains("line 1"), "Expected 'line 1' in:\n{content}");
        assert!(content.contains("line 2"), "Expected 'line 2' in:\n{content}");
        assert!(content.contains("fn main"), "Expected 'fn main' in:\n{content}");
        assert!(content.contains("line 4"), "Expected 'line 4' in:\n{content}");
        assert!(content.contains("line 5"), "Expected 'line 5' in:\n{content}");
    }

    #[tokio::test]
    async fn test_grep_tool_hidden_files() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join(".hidden.rs"), "fn secret() {}")
            .await
            .unwrap();
        tokio::fs::write(base_path.join("normal.rs"), "fn main() {}")
            .await
            .unwrap();

        let tool = GrepTool::new();

        // Always searches hidden files (claude-code behavior)
        let args = serde_json::json!({
            "pattern": "fn secret"
        });
        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains(".hidden.rs"));
    }

    #[tokio::test]
    async fn test_grep_tool_content_mode_records_files() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(
            base_path.join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}",
        )
        .await
        .unwrap();

        // Use file_state_store to track reads
        let store = Arc::new(FileStateStore::new());
        let tool = GrepTool::new().with_file_state_store(Arc::clone(&store));

        let args = serde_json::json!({
            "pattern": "println!",
            "output_mode": "content"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());

        // Verify file was recorded in the store
        let file_path = base_path.join("test.rs").canonicalize().unwrap();
        assert!(store.has_recorded(&file_path));
        assert!(store.get_mtime(&file_path).unwrap() > 0);
    }

    #[tokio::test]
    async fn test_grep_tool_files_with_matches_does_not_record() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(
            base_path.join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}",
        )
        .await
        .unwrap();

        // Use file_state_store to track reads
        let store = Arc::new(FileStateStore::new());
        let tool = GrepTool::new().with_file_state_store(Arc::clone(&store));

        let args = serde_json::json!({
            "pattern": "println!",
            "output_mode": "files_with_matches"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());

        // Verify file was NOT recorded (files_with_matches doesn't record)
        let file_path = base_path.join("test.rs").canonicalize().unwrap();
        assert!(!store.has_recorded(&file_path));
    }

    #[tokio::test]
    async fn test_grep_tool_content_mode_pagination_records_only_displayed() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create multiple files with matches
        tokio::fs::write(base_path.join("file1.rs"), "fn main() { println!(\"1\"); }")
            .await
            .unwrap();
        tokio::fs::write(base_path.join("file2.rs"), "fn foo() { println!(\"2\"); }")
            .await
            .unwrap();
        tokio::fs::write(base_path.join("file3.rs"), "fn bar() { println!(\"3\"); }")
            .await
            .unwrap();

        // Use file_state_store to track reads
        let store = Arc::new(FileStateStore::new());
        let tool = GrepTool::new().with_file_state_store(Arc::clone(&store));

        // Search with limit=1, only first match should be recorded
        let args = serde_json::json!({
            "pattern": "println!",
            "output_mode": "content",
            "limit": 1
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());

        println!("Result content:\n{}", result.text_content());

        // Verify only one file was recorded
        let file1 = base_path.join("file1.rs").canonicalize().unwrap();
        let file2 = base_path.join("file2.rs").canonicalize().unwrap();
        let file3 = base_path.join("file3.rs").canonicalize().unwrap();

        println!("file1: {file1:?}");
        println!("file2: {file2:?}");
        println!("file3: {file3:?}");

        // Check how many files were recorded
        let recorded_count = [store.has_recorded(&file1), store.has_recorded(&file2), store.has_recorded(&file3)]
            .iter()
            .filter(|&&b| b)
            .count();

        assert_eq!(recorded_count, 1, "Expected exactly 1 file recorded. file1={}, file2={}, file3={}",
            store.has_recorded(&file1), store.has_recorded(&file2), store.has_recorded(&file3));

        // The recorded file should be the one that appears in the output
        let content = result.text_content();
        if content.contains("file1.rs") {
            assert!(store.has_recorded(&file1), "file1.rs should be recorded");
        } else if content.contains("file2.rs") {
            assert!(store.has_recorded(&file2), "file2.rs should be recorded");
        } else if content.contains("file3.rs") {
            assert!(store.has_recorded(&file3), "file3.rs should be recorded");
        }
    }
}

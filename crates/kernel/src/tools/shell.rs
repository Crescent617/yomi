use crate::agent::AgentInput;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{AgentId, KernelError, Result, ToolOutput};
use crate::utils::id::gen_base56_id;

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Regex to match ANSI escape sequences
static ANSI_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])").unwrap());

/// Strip ANSI escape sequences from text
#[inline]
fn strip_ansi(text: &str) -> String {
    ANSI_REGEX.replace_all(text, "").to_string()
}

pub const SHELL_TOOL_NAME: &str = "shell";

#[derive(Clone)]
pub struct ShellToolCtx {
    input_tx: Option<mpsc::Sender<AgentInput>>,
}

impl ShellToolCtx {
    pub fn new(_agent_id: AgentId, input_tx: Option<mpsc::Sender<AgentInput>>) -> Self {
        Self { input_tx }
    }
}

pub struct ShellTool {
    ctx: Option<ShellToolCtx>,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellTool {
    pub fn new() -> Self {
        Self { ctx: None }
    }

    #[must_use]
    pub fn with_ctx(mut self, ctx: ShellToolCtx) -> Self {
        self.ctx = Some(ctx);
        self
    }

    fn gen_task_id() -> String {
        format!("sh-{}", gen_base56_id(12))
    }

    fn log_path(task_id: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("yomi_{task_id}.log"))
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        SHELL_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        if cfg!(target_os = "windows") {
            "Execute a shell command using cmd.exe. Reserve exclusively for system commands that require shell execution. Prefer dedicated tools (read, edit, grep) when available. Supports background=true for async execution. DO NOT use for git push or dangerous operations without explicit user request."
        } else {
            "Execute a bash command. Reserve exclusively for system commands that require shell execution. Prefer dedicated tools (read, edit, grep) when available. Supports background=true for async execution. DO NOT use for git push or dangerous operations without explicit user request."
        }
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds. For synchronous mode (default), default is 60s. For background mode, no timeout if not specified.",
                    "minimum": 1
                },
                "background": {
                    "type": "boolean",
                    "description": "Run command in background. When true, returns immediately with task_id, pid, and output file path. Output will be sent via notification when complete.",
                    "default": false
                }
            },
            "required": ["command"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'command' argument"))?;
        let timeout_secs = args["timeout"].as_u64();
        let background = args["background"].as_bool().unwrap_or(false);

        tracing::debug!("Executing bash command: {}", command);

        if background {
            self.exec_async(command, timeout_secs, &ctx.working_dir)
                .await
        } else {
            self.exec_sync(command, timeout_secs, &ctx.working_dir)
                .await
        }
    }
}

impl ShellTool {
    /// Get the appropriate shell command for the current platform
    #[inline]
    fn shell_command() -> (String, String) {
        if cfg!(target_os = "windows") {
            ("cmd.exe".to_string(), "/C".to_string())
        } else {
            ("bash".to_string(), "-c".to_string())
        }
    }

    /// Execute command synchronously and return output directly
    async fn exec_sync(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
        working_dir: &std::path::Path,
    ) -> Result<ToolOutput> {
        let (shell, arg) = Self::shell_command();
        let child = Command::new(&shell)
            .arg(&arg)
            .arg(command)
            .current_dir(working_dir)
            .stdin(Stdio::null()) // Prevent commands from hanging on interactive input
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();

        let output = match timeout(Duration::from_secs(timeout_secs.unwrap_or(60)), child).await {
            Ok(result) => result?,
            Err(_) => {
                tracing::warn!(
                    "Bash command timed out after {}s: {}",
                    timeout_secs.unwrap_or(60),
                    command
                );
                return Ok(ToolOutput::error("Command timed out"));
            }
        };

        let exit_code = output.status.code().unwrap_or(-1);

        // Strip ANSI escape sequences from output
        let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
        let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));

        if exit_code == 0 {
            tracing::debug!(
                "Bash command completed successfully (exit code: {})",
                exit_code
            );
        } else {
            tracing::warn!(
                "Bash command failed (exit code: {}): stderr={}",
                exit_code,
                stderr
            );
        }

        if exit_code == 0 {
            Ok(ToolOutput::text(stdout))
        } else {
            Ok(ToolOutput::error(format!("{stdout}\n{stderr}")))
        }
    }

    /// Execute command in background and notify via `TaskResult` when complete
    async fn exec_async(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
        working_dir: &std::path::Path,
    ) -> Result<ToolOutput> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| KernelError::tool("Background mode requires context"))?;

        // Check if input_tx is available (subagents don't have this)
        let input_tx = ctx
            .input_tx
            .clone()
            .ok_or_else(|| KernelError::tool("Background mode not supported in subagents"))?;

        let task_id = Self::gen_task_id();
        let output_path = Self::log_path(&task_id);
        let output_path_str = output_path.to_string_lossy().to_string();

        // Start the process and get PID immediately
        let (shell, arg) = Self::shell_command();
        let child = Command::new(&shell)
            .arg(&arg)
            .arg(command)
            .current_dir(working_dir)
            .stdin(Stdio::null()) // Prevent commands from hanging on interactive input
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let pid = child.id().unwrap_or(0);

        let task_id_clone = task_id.clone();
        let output_path_clone = output_path;
        let command_clone = command.to_string();

        tokio::spawn(async move {
            let result = wait_for_child(
                child,
                command_clone,
                output_path_clone.clone(),
                timeout_secs,
            )
            .await;

            let text = match result {
                Ok((code, timed_out)) => {
                    if timed_out {
                        format!(
                            "[Task {task_id_clone} (PID: {pid}) timed out]\nPartial output: {}",
                            output_path_clone.display()
                        )
                    } else {
                        format!(
                            "[Task {task_id_clone} (PID: {pid}) completed]\nExit code: {code}\nOutput: {}",
                            output_path_clone.display()
                        )
                    }
                }
                Err(e) => format!(
                    "[Task {task_id_clone} (PID: {pid}) failed]\nError: {e}\nOutput: {}",
                    output_path_clone.display()
                ),
            };

            let _ = input_tx
                .send(AgentInput::TaskResult {
                    task_id: task_id_clone.clone(),
                    content: vec![crate::types::ContentBlock::Text { text }],
                })
                .await;
        });

        Ok(ToolOutput::text(format!(
            "Task {task_id} started (PID: {pid}).\nOutput file: {output_path_str}\nYou will be notified when it completes."
        )))
    }
}

async fn wait_for_child(
    mut child: tokio::process::Child,
    command: String,
    output_path: std::path::PathBuf,
    timeout_secs: Option<u64>,
) -> Result<(i32, bool)> {
    use tokio::time::timeout;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let mut file = File::create(&output_path).await?;
    file.write_all(format!("# Command: {command}\n").as_bytes())
        .await?;
    if let Some(t) = timeout_secs {
        file.write_all(format!("# Timeout: {t}s\n").as_bytes())
            .await?;
    }
    file.write_all(b"\n").await?;
    drop(file);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1000);
    let out_path = output_path.clone();

    let writer = tokio::spawn(async move {
        match File::options().append(true).open(&out_path).await {
            Ok(mut file) => {
                while let Some(line) = rx.recv().await {
                    if file.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to open output file for writing: {e}");
            }
        }
    });

    let tx_out = tx.clone();
    let out_reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let cleaned = strip_ansi(&line);
            if tx_out.send(format!("{cleaned}\n")).await.is_err() {
                break;
            }
        }
    });

    let tx_err = tx.clone();
    let err_reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let cleaned = strip_ansi(&line);
            if tx_err.send(format!("[stderr] {cleaned}\n")).await.is_err() {
                break;
            }
        }
    });

    let result = if let Some(secs) = timeout_secs {
        match timeout(Duration::from_secs(secs), child.wait()).await {
            Ok(Ok(status)) => Ok((status.code().unwrap_or(-1), false)),
            Ok(Err(e)) => Err(KernelError::tool(format!("Process error: {e}"))),
            Err(_) => {
                let _ = child.kill().await;
                Ok((-1, true))
            }
        }
    } else {
        let status = child.wait().await?;
        Ok((status.code().unwrap_or(-1), false))
    };

    let _ = tokio::join!(out_reader, err_reader);
    drop(tx);
    let _ = writer.await;

    let (code, timed_out) = result?;

    match File::options().append(true).open(&output_path).await {
        Ok(mut file) => {
            if timed_out {
                let _ = file
                    .write_all(format!("\n# Task timed out after {timeout_secs:?}s\n").as_bytes())
                    .await;
            }
            let _ = file
                .write_all(format!("\n# Exit: {code}\n").as_bytes())
                .await;
        }
        Err(e) => {
            tracing::error!("Failed to append exit code: {e}");
        }
    }

    Ok((code, timed_out))
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn test_strip_ansi_colors() {
        // Red text
        let input = "\x1b[31mred text\x1b[0m";
        assert_eq!(strip_ansi(input), "red text");

        // Green text
        let input = "\x1b[32mgreen text\x1b[0m";
        assert_eq!(strip_ansi(input), "green text");

        // Bold + blue
        let input = "\x1b[1;34mbold blue\x1b[0m";
        assert_eq!(strip_ansi(input), "bold blue");
    }

    #[test]
    fn test_strip_ansi_cursor_control() {
        // Clear screen
        let input = "\x1b[2Jcleared";
        assert_eq!(strip_ansi(input), "cleared");

        // Cursor up
        let input = "\x1b[Aup";
        assert_eq!(strip_ansi(input), "up");
    }

    #[test]
    fn test_strip_ansi_mixed_content() {
        let input = "normal \x1b[31mred\x1b[0m normal \x1b[32mgreen\x1b[0m";
        assert_eq!(strip_ansi(input), "normal red normal green");
    }

    #[test]
    fn test_strip_ansi_no_escape() {
        let input = "no escape codes here";
        assert_eq!(strip_ansi(input), "no escape codes here");
    }

    #[test]
    fn test_strip_ansi_empty() {
        assert_eq!(strip_ansi(""), "");
    }
}

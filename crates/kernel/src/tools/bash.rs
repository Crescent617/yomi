use crate::agent::AgentInput;
use crate::tool::Tool;
use crate::types::{AgentId, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use rand::Rng;
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

const BASE56_CHARS: &[u8] = b"23456789abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ";

/// Generate a random base56 ID of specified length
fn gen_base56_id(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| {
            let idx = rng.gen_range(0..BASE56_CHARS.len());
            BASE56_CHARS[idx] as char
        })
        .collect()
}

#[derive(Clone)]
pub struct BashToolCtx {
    input_tx: mpsc::Sender<AgentInput>,
}

impl BashToolCtx {
    pub fn new(
        _agent_id: AgentId,
        input_tx: mpsc::Sender<AgentInput>,
        _working_dir: std::path::PathBuf,
    ) -> Self {
        Self { input_tx }
    }
}

pub struct BashTool {
    working_dir: std::path::PathBuf,
    ctx: Option<BashToolCtx>,
}

impl BashTool {
    pub fn new(working_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            ctx: None,
        }
    }

    pub fn with_ctx(mut self, ctx: BashToolCtx) -> Self {
        self.ctx = Some(ctx);
        self
    }

    fn gen_task_id() -> String {
        format!("task_{}", gen_base56_id(12))
    }

    fn log_path(task_id: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("yomi_{task_id}.log"))
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn desc(&self) -> &'static str {
        "Execute a bash command in the working directory. Use 'background: true' to run long commands asynchronously."
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
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

    async fn exec(&self, args: Value) -> Result<ToolOutput> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;
        let timeout_secs = args["timeout"].as_u64();
        let background = args["background"].as_bool().unwrap_or(false);

        tracing::debug!("Executing bash command: {}", command);

        if background {
            self.exec_async(command, timeout_secs).await
        } else {
            self.exec_sync(command, timeout_secs).await
        }
    }
}

impl BashTool {
    /// Execute command synchronously and return output directly
    async fn exec_sync(&self, command: &str, timeout_secs: Option<u64>) -> Result<ToolOutput> {
        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&self.working_dir)
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
                return Ok(ToolOutput {
                    stdout: String::new(),
                    stderr: "Command timed out".to_string(),
                    exit_code: -1,
                });
            }
        };

        let exit_code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

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

        Ok(ToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr,
            exit_code,
        })
    }

    /// Execute command in background and notify via `TaskResult` when complete
    async fn exec_async(&self, command: &str, timeout_secs: Option<u64>) -> Result<ToolOutput> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Background mode requires context"))?;

        let task_id = Self::gen_task_id();
        let output_path = Self::log_path(&task_id);
        let output_path_str = output_path.to_string_lossy().to_string();

        // Start the process and get PID immediately
        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&self.working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let pid = child.id().unwrap_or(0);

        let ctx_clone = ctx.clone();
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

            let _ = ctx_clone
                .input_tx
                .send(AgentInput::TaskResult {
                    task_id: task_id_clone.clone(),
                    content: vec![crate::types::ContentBlock::Text { text }],
                })
                .await;
        });

        Ok(ToolOutput {
            stdout: format!(
                "Task {task_id} started (PID: {pid}).\nOutput file: {output_path_str}\nYou will be notified when it completes."
            ),
            stderr: String::new(),
            exit_code: 0,
        })
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
            if tx_out.send(format!("{line}\n")).await.is_err() {
                break;
            }
        }
    });

    let tx_err = tx.clone();
    let err_reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx_err.send(format!("[stderr] {line}\n")).await.is_err() {
                break;
            }
        }
    });

    let result = if let Some(secs) = timeout_secs {
        match timeout(Duration::from_secs(secs), child.wait()).await {
            Ok(Ok(status)) => Ok((status.code().unwrap_or(-1), false)),
            Ok(Err(e)) => Err(anyhow::anyhow!("Process error: {e}")),
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

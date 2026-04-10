use crate::agent::AgentInput;
use crate::tool::Tool;
use crate::types::{AgentId, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, RwLock};

#[derive(Clone)]
pub struct TaskCtx {
    #[allow(dead_code)]
    agent_id: AgentId,
    input_tx: mpsc::Sender<AgentInput>,
    working_dir: PathBuf,
    registry: Arc<TaskRegistry>,
}

impl TaskCtx {
    pub fn new(
        agent_id: AgentId,
        input_tx: mpsc::Sender<AgentInput>,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            agent_id,
            input_tx,
            working_dir,
            registry: Arc::new(TaskRegistry::new()),
        }
    }
}

pub struct TaskRegistry {
    tasks: RwLock<HashMap<String, tokio::task::AbortHandle>>,
}

impl TaskRegistry {
    fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    async fn register(&self, task_id: &str, handle: tokio::task::AbortHandle) {
        self.tasks.write().await.insert(task_id.to_string(), handle);
    }

    async fn remove(&self, task_id: &str) {
        self.tasks.write().await.remove(task_id);
    }

    pub async fn cancel(&self, task_id: &str) -> bool {
        let handle = self.tasks.write().await.remove(task_id);
        handle.is_some_and(|handle| {
            handle.abort();
            true
        })
    }
}

pub struct RunTask {
    ctx: TaskCtx,
}

impl RunTask {
    pub const fn new(ctx: TaskCtx) -> Self {
        Self { ctx }
    }

    fn generate_id() -> String {
        format!("task_{}", uuid::Uuid::now_v7())
    }

    fn output_path(task_id: &str) -> PathBuf {
        std::env::temp_dir().join(format!("yomi_{task_id}.log"))
    }
}

#[async_trait]
impl Tool for RunTask {
    fn name(&self) -> &'static str {
        "run_task"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command in the background. Output is written to a temp file. \
         You will be notified when the task completes via a message."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (optional, no timeout if not specified)",
                    "minimum": 1
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing command argument"))?
            .to_string();

        let timeout_secs = args["timeout"].as_u64();

        let task_id = Self::generate_id();
        let output_path = Self::output_path(&task_id);
        let output_path_str = output_path.to_string_lossy().to_string();

        let ctx = self.ctx.clone();
        let task_id_clone = task_id.clone();

        let spawn_handle = tokio::spawn(async move {
            let result = run_shell(
                command,
                output_path.clone(),
                ctx.working_dir.clone(),
                timeout_secs,
            )
            .await;

            let text = match result {
                Ok((code, timed_out)) => {
                    if timed_out {
                        format!(
                            "[Task {task_id_clone} timed out]\nPartial output: {}",
                            output_path.display()
                        )
                    } else {
                        format!(
                            "[Task {task_id_clone} completed]\nExit code: {code}\nOutput: {}",
                            output_path.display()
                        )
                    }
                }
                Err(e) => format!(
                    "[Task {task_id_clone} failed]\nError: {e}\nOutput: {}",
                    output_path.display()
                ),
            };

            let _ = ctx
                .input_tx
                .send(AgentInput::TaskResult {
                    task_id: task_id_clone.clone(),
                    content: vec![crate::types::ContentBlock::Text { text }],
                })
                .await;

            ctx.registry.remove(&task_id_clone).await;
        });

        self.ctx
            .registry
            .register(&task_id, spawn_handle.abort_handle())
            .await;

        Ok(ToolOutput {
            stdout: format!(
                "Task {task_id} started.\nOutput file: {output_path_str}\nYou will be notified when it completes."
            ),
            stderr: String::new(),
            exit_code: 0,
        })
    }

}

/// Cancel a running background task
pub struct CancelTask {
    ctx: TaskCtx,
}

impl CancelTask {
    pub const fn new(ctx: TaskCtx) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for CancelTask {
    fn name(&self) -> &'static str {
        "cancel_task"
    }

    fn description(&self) -> &'static str {
        "Cancel a running background task"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task ID to cancel"
                }
            },
            "required": ["task_id"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput> {
        let task_id = args["task_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing task_id"))?;

        if self.ctx.registry.cancel(task_id).await {
            Ok(ToolOutput {
                stdout: format!("Task {task_id} cancelled"),
                stderr: String::new(),
                exit_code: 0,
            })
        } else {
            Ok(ToolOutput {
                stdout: String::new(),
                stderr: format!("Task {task_id} not found"),
                exit_code: 1,
            })
        }
    }
}

async fn run_shell(
    command: String,
    output_path: PathBuf,
    working_dir: PathBuf,
    timeout_secs: Option<u64>,
) -> Result<(i32, bool)> {
    use std::process::Stdio;
    use tokio::time::timeout;

    let mut child = Command::new("bash")
        .arg("-c")
        .arg(&command)
        .current_dir(&working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

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
            let _ = file.write_all(format!("\n# Exit: {code}\n").as_bytes()).await;
        }
        Err(e) => {
            tracing::error!("Failed to append exit code: {e}");
        }
    }

    Ok((code, timed_out))
}

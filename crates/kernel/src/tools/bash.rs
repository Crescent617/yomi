use crate::tool::Tool;
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

pub struct BashTool {
    working_dir: std::path::PathBuf,
}

impl BashTool {
    pub fn new(working_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a bash command in the working directory"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 60)",
                    "default": 60
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;
        let timeout_secs = args["timeout"].as_u64().unwrap_or(60);

        tracing::debug!("Executing bash command: {}", command);

        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&self.working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();

        let output = match timeout(Duration::from_secs(timeout_secs), child).await {
            Ok(result) => result?,
            Err(_) => {
                tracing::warn!(
                    "Bash command timed out after {}s: {}",
                    timeout_secs,
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
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

}

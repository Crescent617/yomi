use super::{HookContext, HookEvent, HookHandler, HookResult};
use crate::types::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::timeout;

/// Read all bytes from an async pipe into a buffer.
async fn read_pipe(mut pipe: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    let mut buf = Vec::new();
    AsyncReadExt::read_to_end(&mut pipe, &mut buf).await.ok();
    buf
}

/// Run an external command as a hook.
/// Compatible with Claude Code / Codex hook scripts.
#[derive(Debug)]
pub struct CommandHookHandler {
    name: String,
    events: Vec<HookEvent>,
    matcher: regex::Regex,
    command: String,
    timeout_secs: u64,
}

impl CommandHookHandler {
    pub fn new(
        name: impl Into<String>,
        event: HookEvent,
        matcher: impl AsRef<str>,
        command: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            name: name.into(),
            events: vec![event],
            matcher: super::case_insensitive_regex(matcher.as_ref())?,
            command: command.into(),
            timeout_secs: 30,
        })
    }

    #[must_use]
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

#[async_trait]
impl HookHandler for CommandHookHandler {
    fn name(&self) -> &str {
        &self.name
    }

    fn events(&self) -> &[HookEvent] {
        &self.events
    }

    fn matches(&self, ctx: &HookContext) -> bool {
        ctx.tool_matches(&self.matcher)
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookResult> {
        let input = serde_json::to_string(ctx)
            .map_err(|e| crate::types::KernelError::serde(e.to_string()))?;

        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C");
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c");
            c
        };
        cmd.arg(&self.command)
            .current_dir(&ctx.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| crate::types::KernelError::tool(format!("Hook spawn failed: {e}")))?;

        // Write stdin asynchronously.
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| crate::types::KernelError::tool("Failed to open hook stdin"))?;
        let input_bytes = input.into_bytes();
        tokio::spawn(async move {
            if let Err(e) = stdin.write_all(&input_bytes).await {
                tracing::warn!("Failed to write hook stdin: {e}");
            }
            let _ = stdin.shutdown().await;
        });

        // Start reading stdout/stderr concurrently to avoid pipe deadlock.
        let stdout_handle = {
            let pipe = child.stdout.take().expect("piped stdout");
            tokio::spawn(read_pipe(pipe))
        };
        let stderr_handle = {
            let pipe = child.stderr.take().expect("piped stderr");
            tokio::spawn(read_pipe(pipe))
        };

        let abort_readers = || {
            stdout_handle.abort();
            stderr_handle.abort();
        };

        let dur = Duration::from_secs(self.timeout_secs);
        let wait_result = timeout(dur, child.wait()).await;

        let status = match wait_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                abort_readers();
                return Err(crate::types::KernelError::tool(format!(
                    "Hook command failed: {e}"
                )));
            }
            Err(_) => {
                if let Err(e) = child.kill().await {
                    tracing::warn!("Failed to kill timed-out hook '{}': {}", self.name, e);
                }
                abort_readers();
                return Err(crate::types::KernelError::tool(format!(
                    "Hook '{}' timed out after {}s",
                    self.name, self.timeout_secs
                )));
            }
        };

        let stdout_bytes = stdout_handle.await.unwrap_or_default();
        let stderr_bytes = stderr_handle.await.unwrap_or_default();
        let stdout = String::from_utf8_lossy(&stdout_bytes);
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        let trimmed = stdout.trim();

        // Always trace stderr when present.
        if !stderr.is_empty() {
            tracing::trace!("Hook '{}' stderr: {}", self.name, stderr);
        }

        // Exit code 2 = block (Claude Code convention)
        if status.code() == Some(2) {
            return match ctx.event {
                HookEvent::PreToolUse => {
                    if let Ok(d) = serde_json::from_str::<super::PreToolDecision>(trimmed) {
                        Ok(HookResult::PreTool(d))
                    } else {
                        let reason = if trimmed.is_empty() {
                            "Blocked by hook".to_string()
                        } else {
                            trimmed.to_string()
                        };
                        Ok(HookResult::PreTool(super::PreToolDecision {
                            action: super::PreToolAction::Block,
                            reason: Some(reason),
                            ..Default::default()
                        }))
                    }
                }
                HookEvent::PostToolUse => {
                    if let Ok(d) = serde_json::from_str::<super::PostToolDecision>(trimmed) {
                        Ok(HookResult::PostTool(d))
                    } else {
                        let reason = if trimmed.is_empty() {
                            "Blocked by hook".to_string()
                        } else {
                            trimmed.to_string()
                        };
                        Ok(HookResult::PostTool(super::PostToolDecision {
                            continue_session: false,
                            context: Some(reason),
                            ..Default::default()
                        }))
                    }
                }
            };
        }

        if !status.success() {
            tracing::warn!(
                "Hook '{}' exited with {:?}: stderr={}",
                self.name,
                status.code(),
                stderr
            );
            return Ok(HookResult::Passthrough);
        }

        if trimmed.is_empty() {
            return Ok(HookResult::Passthrough);
        }

        // Try to parse as a typed decision; fall back to plain-text context.
        match ctx.event {
            HookEvent::PreToolUse => {
                if let Ok(d) = serde_json::from_str::<super::PreToolDecision>(trimmed) {
                    Ok(HookResult::PreTool(d))
                } else {
                    Ok(HookResult::PreTool(super::PreToolDecision {
                        context: Some(trimmed.to_string()),
                        ..Default::default()
                    }))
                }
            }
            HookEvent::PostToolUse => {
                if let Ok(d) = serde_json::from_str::<super::PostToolDecision>(trimmed) {
                    Ok(HookResult::PostTool(d))
                } else {
                    Ok(HookResult::PostTool(super::PostToolDecision {
                        context: Some(trimmed.to_string()),
                        ..Default::default()
                    }))
                }
            }
        }
    }
}

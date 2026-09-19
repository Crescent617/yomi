use crate::agent::AgentInput;
use crate::comms::InputBus;
use crate::const_concat;
use crate::tools::helper::truncate::truncate_keep_edges;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, SessionId, ToolOutput};
use crate::utils::id::gen_base56_id;

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use std::borrow::Cow;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

/// Regex to match ANSI escape sequences
static ANSI_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])").unwrap());

/// Strip ANSI escape sequences from text
#[inline]
fn strip_ansi(text: &str) -> Cow<'_, str> {
    ANSI_REGEX.replace_all(text, "")
}

pub const SHELL_TOOL_NAME: &str = "shell";

#[derive(Clone)]
pub struct ShellToolCtx {
    input_bus: Option<Arc<InputBus>>,
    background_tasks: Arc<crate::agent::BgTaskTracker>,
    data_dir: std::path::PathBuf,
}

impl ShellToolCtx {
    pub fn new(
        input_bus: Option<Arc<InputBus>>,
        background_tasks: Arc<crate::agent::BgTaskTracker>,
        data_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            input_bus,
            background_tasks,
            data_dir,
        }
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

    /// The daemon's data dir, when the tool was built with a context.
    fn data_dir(&self) -> Option<&std::path::Path> {
        self.ctx.as_ref().map(|c| c.data_dir.as_path())
    }

    fn log_path(task_id: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("yomi_{task_id}.log"))
    }
}

/// 工具描述为平台无关的静态文本：解释器种类与路径随机器变，由 base
/// prompt 的 Environment 段（prompt/mod.rs）按会话告知模型，工具文档
/// 本身全平台一致（跨机器 prompt 缓存与快照稳定）。
const DESC: &str = const_concat!(
    "Execute a shell command. Use it only for tasks that genuinely require the shell; for reading, editing, or searching files, prefer the dedicated read, edit, and grep tools. Do not run dangerous operations (e.g. git push, rm -rf) without explicit user request. Commands run non-interactively (stdin is closed, no terminal attached), so interactive prompts (e.g. sudo password, ssh confirmation) fail immediately instead of waiting for input.\n\nUse `background: true` for long-running commands (servers, scripts of unknown duration) so you can monitor output in real time; short commands should run synchronously. ",
    crate::tools::ASYNC_LAUNCH_GUIDE,
    " The PID in the launch message can be used to kill the process.",
    " A command can also return images to you: print a line `yomi://image/<path>` (alone on its own line) and the image content is attached to the result (max 10 per call, png/jpeg/gif/webp). Examples: `browser screenshot -s /tmp/x.jpg && echo yomi://image//tmp/x.jpg` (absolute path — note the doubled slash); `browser screenshot -s x.jpg && echo yomi://image/x.jpg` (relative to the working directory)."
);

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        SHELL_TOOL_NAME
    }

    fn desc(&self) -> &str {
        DESC
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
                    "description": "Timeout in seconds. For synchronous mode (default), default is 300s. For background mode, run forever if not specified.",
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
        let timeout_secs = args["timeout"]
            .as_u64()
            .and_then(|s| if s > 0 { Some(s) } else { None });
        let background = args["background"].as_bool().unwrap_or(false);

        tracing::debug!("Executing shell command: {}", command);

        let cancel_token = ctx.cancel_token.clone();
        if background {
            self.exec_async(
                command,
                timeout_secs,
                &ctx.working_dir,
                cancel_token,
                &ctx.session_id,
                ctx.max_tool_output_length,
            )
            .await
        } else {
            self.exec_sync(
                command,
                timeout_secs,
                &ctx.working_dir,
                cancel_token,
                &ctx.session_id,
                ctx.max_tool_output_length,
            )
            .await
        }
    }
}

impl ShellTool {
    /// Build the base `Command` for shell execution, hardened for
    /// non-interactive use:
    ///
    /// - 解释器由 `utils::shell::detect()` 统一选择（bash 优先，Windows
    ///   Git Bash → pwsh → powershell → cmd），命令文本经
    ///   `wrap_command` 注入 UTF-8 输出前缀；
    /// - stdin is null, so reads get immediate EOF;
    /// - env vars disable the remaining interactive prompters (git, ssh);
    /// - yomi 标准环境变量（`YOMI_SESSION_ID` / `YOMI_DATA_DIR`，见
    ///   [`crate::utils::env::inject_child_env`]）让脚本回连 yomi。
    ///
    /// 进程树（unix setsid / windows Job Object，让子进程没有控制终端、
    /// 超时能连后裔一起收）由 `utils::process::spawn_in_new_tree` 在
    /// spawn 时统一建立，本函数不管。
    fn build_command(
        command: &str,
        working_dir: &std::path::Path,
        session_id: &str,
        data_dir: Option<&std::path::Path>,
    ) -> Command {
        Self::build_command_with_shell(
            crate::utils::shell::detect(),
            command,
            working_dir,
            session_id,
            data_dir,
        )
    }

    /// 用指定的 shell 构造命令（`build_command` 的探测解耦版：Windows
    /// 实机测试需要逐档 shell 验证，而 `detect()` 是进程级缓存）。
    fn build_command_with_shell(
        shell: &crate::utils::shell::AgentShell,
        command: &str,
        working_dir: &std::path::Path,
        session_id: &str,
        data_dir: Option<&std::path::Path>,
    ) -> Command {
        let wrapped = shell.wrap_command(command);
        let mut cmd = Command::new(&shell.path);
        cmd.args(shell.leading_args());
        Self::append_wrapped(&mut cmd, shell.kind, &wrapped);
        cmd.current_dir(working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_PAGER", "cat")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .env("PAGER", "cat")
            .env("EDITOR", "true")
            .kill_on_drop(true);
        crate::utils::env::inject_child_env(&mut cmd, data_dir, Some(session_id));

        // BatchMode makes ssh fail instead of prompting (host-key confirm,
        // passphrase). Only set when the user hasn't configured their own,
        // since GIT_SSH_COMMAND may carry functional settings (key, port).
        if std::env::var_os("GIT_SSH_COMMAND").is_none() {
            cmd.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes");
        }

        cmd
    }

    /// 追加包装后的命令串。Windows 上 cmd 的 `/C` 串须 `raw_arg`
    /// 直传：std 的 argv 转义把内嵌 `"` 变成 `\"`，而 cmd 只做首尾
    /// 引号剥离、不认该转义（`echo "a b"` 会残留反斜杠、带引号的
    /// 程序路径启动失败）。pwsh/powershell 按 CommandLineToArgvW
    /// 规则解析 argv，std 转义能被正确还原，保持 `.arg`。
    #[cfg(windows)]
    fn append_wrapped(cmd: &mut Command, kind: crate::utils::shell::ShellKind, wrapped: &str) {
        use std::os::windows::process::CommandExt;
        match kind {
            crate::utils::shell::ShellKind::Cmd => {
                cmd.as_std_mut().raw_arg(wrapped);
            }
            _ => {
                cmd.arg(wrapped);
            }
        }
    }

    #[cfg(not(windows))]
    fn append_wrapped(cmd: &mut Command, _kind: crate::utils::shell::ShellKind, wrapped: &str) {
        cmd.arg(wrapped);
    }

    /// Execute command synchronously and return output directly
    async fn exec_sync(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
        working_dir: &std::path::Path,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
        session_id: &str,
        max_tool_output_length: usize,
    ) -> Result<ToolOutput> {
        let mut cmd = Self::build_command(command, working_dir, session_id, self.data_dir());
        let timeout_duration = Duration::from_secs(timeout_secs.unwrap_or(300));

        // drain 上限给足截断所需素材：输出预算的两倍（截断保头尾），
        // 至少为引擎默认 cap。超 cap 的流全文落盘（引擎惰性创建：
        // 未超 cap 零 IO），路径写进 footer 供 read/grep 工具回查。
        let drain_cap = max_tool_output_length
            .saturating_mul(2)
            .max(crate::utils::spawn::DRAIN_CAP);
        let overflow = Some(crate::utils::spawn::OverflowLog {
            dir: std::env::temp_dir(),
            stem: format!("yomi_{}", Self::gen_task_id()),
        });
        let captured = match crate::utils::spawn::spawn_captured_with_cap(
            &mut cmd,
            None,
            timeout_duration,
            cancel_token.as_ref(),
            drain_cap,
            overflow,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => return Err(KernelError::tool(format!("Process error: {e}"))),
        };

        if captured.cancelled {
            tracing::info!("Shell command cancelled: {}", command);
            return Ok(ToolOutput::error("Command cancelled"));
        }
        if captured.timed_out {
            tracing::warn!(
                "Shell command timed out after {}s: {}",
                timeout_duration.as_secs(),
                command
            );
            return Ok(ToolOutput::error("Command timed out"));
        }

        // exit_code 为 None 仅见于 unix 信号终止（超时/取消已在上面
        // 分支返回；windows 的 job 强杀同）。
        let status = match (captured.exit_code, captured.signal) {
            (Some(code), _) => format!("exit code: {code}"),
            (None, Some(sig)) => format!("killed by signal {sig}"),
            (None, None) => "killed by signal".to_string(),
        };
        let success = captured.exit_code == Some(0);

        if success {
            tracing::debug!("Shell command completed successfully ({status})");
        } else {
            tracing::warn!("Shell command failed ({status})");
        }

        let stdout_raw = String::from_utf8_lossy(&captured.stdout);
        let stderr_raw = String::from_utf8_lossy(&captured.stderr);
        let stdout = strip_ansi(&stdout_raw);
        let stderr = strip_ansi(&stderr_raw);

        // `yomi://image/` marker（协议见 utils::image_marker 模块文档）：
        // 在截断前提取，marker 行不会被保头尾截掉；图片不占文本预算。
        // 相对路径按本次调用的 working_dir 解析。
        let collected =
            crate::utils::image_marker::collect_image_markers(&stdout, working_dir).await;

        let log_note = format_log_note(&captured.log_files);
        let footer = format!(
            "\n\n---\n[{status}] Command {}.{log_note}",
            if success { "completed" } else { "failed" }
        );

        let total_budget = max_tool_output_length.saturating_sub(footer.len());
        let content = format_sync_output(&collected.text, &stderr, total_budget);

        let output_text = format!("{content}{footer}");
        if collected.images.is_empty() {
            Ok(ToolOutput::text(output_text))
        } else {
            let mut contents = vec![crate::types::ToolOutputBlock::Text { text: output_text }];
            contents.extend(collected.images);
            Ok(ToolOutput {
                contents,
                is_error: false,
            })
        }
    }

    /// Execute command in background and send its completion as a steer message.
    #[allow(clippy::too_many_arguments)]
    async fn exec_async(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
        working_dir: &std::path::Path,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
        session_id: &str,
        max_tool_output_length: usize,
    ) -> Result<ToolOutput> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| KernelError::tool("Background mode requires context"))?;

        // Check if input_bus is available (subagents don't have this)
        let input_bus = ctx
            .input_bus
            .clone()
            .ok_or_else(|| KernelError::tool("Background mode not supported in subagents"))?;

        let task_id = Self::gen_task_id();
        let output_path = Self::log_path(&task_id);
        let output_path_str = output_path.to_string_lossy().to_string();

        // Start the process and get PID immediately
        let mut cmd = Self::build_command(command, working_dir, session_id, Some(&ctx.data_dir));
        let (child, tree) = crate::utils::process::spawn_in_new_tree(&mut cmd)?;

        let pid = child.id().unwrap_or(0);
        let tracker_guard = ctx
            .background_tasks
            .start_shell(crate::agent::BackgroundShellTask {
                task_id: task_id.clone(),
                session_id: SessionId::from(session_id.to_string()),
                pid,
                command: command.to_string(),
                output_path: output_path_str.clone(),
                started_at: chrono::Utc::now(),
            });

        let task_id_clone = task_id.clone();
        let output_path_clone = output_path;
        let command_clone = command.to_string();
        let session_id = session_id.to_string();

        tokio::spawn(async move {
            let result = wait_for_child(
                child,
                tree,
                command_clone,
                output_path_clone.clone(),
                timeout_secs,
                cancel_token,
            )
            .await;

            let output = match tokio::fs::read_to_string(&output_path_clone).await {
                Ok(log) => extract_log_body(&log).to_string(),
                Err(e) => {
                    tracing::warn!("Failed to read background task output: {e}");
                    String::new()
                }
            };
            let text = format_background_result(
                result,
                &output_path_clone,
                &output,
                max_tool_output_length,
            );
            let text = crate::tools::format_shell_message(task_id_clone, text);

            drop(tracker_guard);
            if let Err(e) = input_bus.publish(
                SessionId::from(session_id.clone()),
                AgentInput::Steer(vec![crate::types::ContentBlock::Text { text }]),
            ) {
                tracing::warn!("Failed to publish shell async result: {}", e);
            }
        });

        Ok(ToolOutput::text(format!(
            "Task {task_id} started (PID: {pid}).\nOutput file: {output_path_str}\n{}\nYou will be notified when it completes.",
            crate::tools::ASYNC_LAUNCH_GUIDE
        )))
    }
}

/// sync 路径输出超 cap 全文落盘后的引用说明（与 background 路径的
/// 「Log file:」词汇一致——同一份输出约定，模型见到路径可用 read /
/// grep 工具回查）。
fn format_log_note(log_files: &[(std::path::PathBuf, u64)]) -> String {
    if log_files.is_empty() {
        return String::new();
    }
    let list = log_files
        .iter()
        .map(|(path, bytes)| format!("{} ({bytes} bytes)", path.display()))
        .collect::<Vec<_>>()
        .join("; ");
    format!("\nLog file: {list} (full output, truncated above)")
}

fn format_background_result<E: std::fmt::Display>(
    result: std::result::Result<(i32, bool, bool), E>,
    output_path: &std::path::Path,
    output: &str,
    max_output_length: usize,
) -> String {
    let status = match result {
        Ok((_code, _timed_out, true)) => "[Task cancelled]".to_string(),
        Ok((_code, true, _cancelled)) => "[Task timed_out]".to_string(),
        Ok((0, false, false)) => "[Task completed] Exit code: 0".to_string(),
        Ok((code, false, false)) => format!("[Task failed] Exit code: {code}"),
        Err(error) => format!("[Task failed] Error: {error}"),
    };

    let header = format!("{status} · Log file: {}", output_path.display());

    let output = output.trim();
    if output.is_empty() {
        return format!("{header}\n[No output]");
    }

    let budget = max_output_length.saturating_sub(header.len());
    format!("{header}\n[output]\n{}", format_stream(output, budget))
}

/// Extract the command output from a background task log, stripping the
/// `# Command:`/`# Timeout:` header and the trailing `# Exit:`/`# Task ...`
/// footer lines written by `wait_for_child`.
fn extract_log_body(log: &str) -> &str {
    let mut body = log;

    // Strip leading header lines ("# Command: ...", optional "# Timeout: ...").
    while let Some(rest) = body.strip_prefix("# ") {
        match rest.find('\n') {
            Some(pos) => body = &rest[pos + 1..],
            None => return "",
        }
    }
    // Strip the blank line separating the header from the output.
    if let Some(rest) = body.strip_prefix('\n') {
        body = rest;
    }

    // Strip trailing footer lines ("# Task ...", "# Exit: ...") and the blank
    // line separating them from the output.
    while let Some(pos) = body.rfind('\n') {
        let last_line = &body[pos + 1..];
        if last_line.is_empty() || last_line.starts_with("# ") {
            body = &body[..pos];
        } else {
            break;
        }
    }

    body
}

/// Format a single stream with optional truncation and label.
fn format_stream(text: &str, budget: usize) -> String {
    if text.len() > budget {
        truncate_keep_edges(text, budget, "\n... [truncated] ...\n")
    } else {
        text.to_string()
    }
}

/// Assemble the result body from the captured streams. The `[stdout]` /
/// `[stderr]` labels (and their newlines) only appear when both streams
/// have content and need disambiguating; a single non-empty stream is
/// returned bare.
fn format_sync_output(stdout: &str, stderr: &str, budget: usize) -> String {
    match (!stdout.is_empty(), !stderr.is_empty()) {
        (true, true) => {
            let per_stream = budget / 2;
            format!(
                "[stdout]\n{}\n\n[stderr]\n{}",
                format_stream(stdout, per_stream),
                format_stream(stderr, per_stream)
            )
        }
        (true, false) => format_stream(stdout, budget),
        (false, true) => format_stream(stderr, budget),
        (false, false) => String::new(),
    }
}

/// Parse a successful `child.wait()` result.
fn parse_wait_result(
    result: std::result::Result<std::process::ExitStatus, std::io::Error>,
) -> Result<(i32, bool, bool)> {
    match result {
        Ok(status) => Ok((status.code().unwrap_or(-1), false, false)),
        Err(e) => Err(KernelError::tool(format!("Process error: {e}"))),
    }
}

/// Handle a timed-out `child.wait()` result.
async fn handle_timeout_result(
    result: std::result::Result<
        std::result::Result<std::process::ExitStatus, std::io::Error>,
        tokio::time::error::Elapsed,
    >,
    child: &mut tokio::process::Child,
    tree: &crate::utils::process::ProcessTree,
) -> Result<(i32, bool, bool)> {
    match result {
        Ok(result) => parse_wait_result(result),
        Err(_) => {
            crate::utils::process::kill_tree(child, tree).await;
            Ok((-1, true, false))
        }
    }
}

async fn wait_for_child(
    mut child: tokio::process::Child,
    tree: crate::utils::process::ProcessTree,
    command: String,
    output_path: std::path::PathBuf,
    timeout_secs: Option<u64>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<(i32, bool, bool)> {
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let mut file = crate::utils::spawn::open_log_file(&output_path).await?;
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
        let timeout_fut = timeout(Duration::from_secs(secs), child.wait());
        match cancel_token {
            Some(token) => {
                tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        crate::utils::process::kill_tree(&mut child, &tree).await;
                        Ok((-1, false, true))
                    }
                    result = timeout_fut => handle_timeout_result(result, &mut child, &tree).await,
                }
            }
            None => handle_timeout_result(timeout_fut.await, &mut child, &tree).await,
        }
    } else {
        match cancel_token {
            Some(token) => {
                tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        crate::utils::process::kill_tree(&mut child, &tree).await;
                        Ok((-1, false, true))
                    }
                    result = child.wait() => parse_wait_result(result),
                }
            }
            None => parse_wait_result(child.wait().await),
        }
    };

    let _ = tokio::join!(out_reader, err_reader);
    drop(tx);
    let _ = writer.await;

    let (code, timed_out, cancelled) = result?;

    match File::options().append(true).open(&output_path).await {
        Ok(mut file) => {
            if cancelled {
                let _ = file.write_all(b"\n# Task cancelled\n").await;
            } else if timed_out {
                let timeout_str = timeout_secs.map_or_else(|| "unknown".into(), |s| format!("{s}"));
                let _ = file
                    .write_all(format!("\n# Task timed out after {timeout_str}s\n").as_bytes())
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

    Ok((code, timed_out, cancelled))
}

#[cfg(test)]
#[path = "shell_test.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "shell_windows_test.rs"]
mod windows_tests;

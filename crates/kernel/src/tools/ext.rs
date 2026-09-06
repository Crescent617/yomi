//! ext/ — `tools/` 目录外挂工具（Capability 端口的脚本形态）。
//!
//! 目录即注册表：`<data_dir>/tools/<name>/` 下 `tool.json`（`desc`/
//! `schema`/`level`/`timeout_secs`）+ 带执行位的 `run`（执行位 = 开关）。每次调用由
//! `utils::spawn` 引擎 spawn 一次 `run`：stdin 喂单行 JSON
//! （`event/session_id/cwd/tool_name/args`），exit 0 → stdout 即工具结果，
//! 非零/超时/spawn 失败 → stderr 作 tool error 喂回 agent（fail-closed，
//! 对齐 hook 的镜像语义：调用方在等结果）。
//!
//! 命名约束取各 provider 的最紧交集（OpenAI 函数名只允许 `[a-zA-Z0-9_-]`
//! 且字母开头）；目录名唯一 ⇒ 外挂撞外挂不可能，撞内建在 Agent 合并时
//! 让位（agent.rs，warn 跳过）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tracing::{debug, warn};

use crate::permission::Level;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{Result, ToolOutput};

/// 外挂工具库目录名（相对 `data_dir`）。
pub(crate) const DIR_NAME: &str = "tools";
/// manifest 文件名。
pub(crate) const MANIFEST_FILE: &str = "tool.json";
/// 入口文件名（执行位 = 开关）。
pub(crate) const ENTRY_FILE: &str = "run";

/// 单次调用缺省超时（manifest `timeout_secs` 可覆盖，上限
/// [`MAX_TIMEOUT_SECS`]）。
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 600;
/// 失败回流给 agent 的文本上限（对齐 hook 的否决原因上限）。
const MAX_ERROR_CHARS: usize = 2000;

#[derive(Debug, serde::Deserialize)]
struct ManifestFile {
    desc: String,
    schema: Value,
    #[serde(default = "default_level")]
    level: Level,
    timeout_secs: Option<u64>,
}

/// 缺省 caution（走审批）：外挂是任意外部代码，不给"默认免审"的口子。
fn default_level() -> Level {
    Level::Caution
}

/// 一个扫描到的外挂工具。
pub struct SpawnTool {
    name: String,
    entry: PathBuf,
    data_dir: PathBuf,
    state_dir: PathBuf,
    desc: String,
    schema: Value,
    level: Level,
    timeout: Duration,
}

#[async_trait::async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn desc(&self) -> &str {
        &self.desc
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn level(&self) -> Option<Level> {
        Some(self.level)
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // state 目录（state/tools/<名>）惰性创建；失败不致命——脚本可
        // 自行 mkdir。
        crate::utils::env::ensure_state_dir("tool", &self.name, &self.state_dir).await;
        let payload = match serde_json::to_vec(&serde_json::json!({
            "event": "tool",
            "session_id": ctx.session_id,
            "cwd": ctx.working_dir.to_string_lossy(),
            "tool_name": self.name,
            "args": args,
        })) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "[ext:{}] payload: {e}",
                    self.name
                )))
            }
        };
        let mut cmd = tokio::process::Command::new(&self.entry);
        cmd.current_dir(&ctx.working_dir)
            .env(crate::utils::env::YOMI_EVENT, "tool");
        crate::utils::env::inject_child_env(&mut cmd, Some(&self.data_dir), Some(&ctx.session_id));
        crate::utils::env::inject_state_dir(&mut cmd, Some(&self.state_dir));
        let captured = match crate::utils::spawn::spawn_captured(
            &mut cmd,
            Some(&payload),
            self.timeout,
            ctx.cancel_token.as_ref(),
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!(tool = %self.name, entry = %self.entry.display(), cwd = %ctx.working_dir.display(), error = %e, "ext tool: engine failed");
                return Ok(ToolOutput::error(format!("[ext:{}] {e}", self.name)));
            }
        };
        let stderr = String::from_utf8_lossy(&captured.stderr).into_owned();
        if captured.cancelled {
            debug!(tool = %self.name, "ext tool: cancelled");
            return Ok(ToolOutput::error(format!("[ext:{}] cancelled", self.name)));
        }
        if captured.timed_out {
            warn!(tool = %self.name, timeout_ms = self.timeout.as_millis(), "ext tool: timed out");
            return Ok(ToolOutput::error(timeout_text(
                &self.name,
                self.timeout,
                &stderr,
            )));
        }
        match captured.exit_code {
            Some(0) => {
                let stdout = String::from_utf8_lossy(&captured.stdout).into_owned();
                let budget = ctx.max_tool_output_length;
                let text = if stdout.len() > budget {
                    crate::tools::helper::truncate::truncate_keep_edges(
                        &stdout,
                        budget,
                        "\n... [truncated] ...\n",
                    )
                } else {
                    stdout
                };
                Ok(ToolOutput::text(text))
            }
            other => {
                warn!(tool = %self.name, exit_code = ?other, stderr = %stderr.trim(), "ext tool: failed");
                Ok(ToolOutput::error(failure_text(&self.name, other, &stderr)))
            }
        }
    }
}

/// 扫描 `<data_dir>/tools/`：有效子目录 → 代理工具。无效条目 warn/debug
/// 跳过（pets invalid-package 先例）；目录不存在视为空。
pub async fn scan(data_dir: &Path) -> Vec<Arc<dyn Tool>> {
    let base = data_dir.join(DIR_NAME);
    let mut rd = match tokio::fs::read_dir(&base).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            warn!(dir = %base.display(), error = %e, "ext tools: readdir failed");
            return Vec::new();
        }
    };
    let mut dirs = Vec::new();
    loop {
        match rd.next_entry().await {
            Ok(Some(de)) => {
                let Ok(name) = de.file_name().into_string() else {
                    continue;
                };
                if name.starts_with('.') {
                    continue;
                }
                // `DirEntry::metadata` 是 lstat 语义（不跟随符号链接），
                // 必须对完整路径用 `fs::metadata` 才真正跟随（stow/nix）。
                match tokio::fs::metadata(de.path()).await {
                    Ok(md) if md.is_dir() => dirs.push((name, de.path())),
                    Ok(_) => {}
                    Err(e) => {
                        debug!(name = %name, error = %e, "ext tools: skipping entry with unreadable metadata");
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "ext tools: readdir entry failed");
                break;
            }
        }
    }
    dirs.sort();
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    for (name, dir) in dirs {
        match load_one(data_dir, &name, &dir).await {
            Ok(tool) => out.push(Arc::new(tool)),
            Err(Reject::Fault(e)) => warn!(tool = %name, "{e}"),
            Err(Reject::Disabled(e)) => debug!(tool = %name, "{e}"),
        }
    }
    out
}

/// 拒收的两种响度：Fault=配置坏（该修）；Disabled=开关关（正常态）。
enum Reject {
    Fault(String),
    Disabled(String),
}

async fn load_one(
    data_dir: &Path,
    name: &str,
    dir: &Path,
) -> std::result::Result<SpawnTool, Reject> {
    if !valid_name(name) {
        return Err(Reject::Fault(format!(
            "invalid tool name '{name}': must start with a letter and contain only \
             letters, numbers, underscores and dashes (provider constraint)"
        )));
    }
    let manifest_path = dir.join(MANIFEST_FILE);
    let bytes = tokio::fs::read(&manifest_path)
        .await
        .map_err(|e| Reject::Fault(format!("read {}: {e}", manifest_path.display())))?;
    let manifest: ManifestFile = serde_json::from_slice(&bytes)
        .map_err(|e| Reject::Fault(format!("parse {}: {e}", manifest_path.display())))?;
    let entry = dir.join(ENTRY_FILE);
    let md = tokio::fs::metadata(&entry)
        .await
        .map_err(|e| Reject::Fault(format!("no executable entry {}: {e}", entry.display())))?;
    if !md.is_file() {
        return Err(Reject::Fault(format!("{} is not a file", entry.display())));
    }
    if !is_executable(&md) {
        // 执行位即开关：摘下是显式停用，不是故障。
        return Err(Reject::Disabled(
            "disabled (run lacks exec bit)".to_string(),
        ));
    }
    let timeout_secs = manifest
        .timeout_secs
        .map_or(DEFAULT_TIMEOUT_SECS, |s| s.clamp(1, MAX_TIMEOUT_SECS));
    Ok(SpawnTool {
        name: name.to_string(),
        entry,
        data_dir: data_dir.to_path_buf(),
        state_dir: data_dir.join("state").join(DIR_NAME).join(name),
        desc: manifest.desc,
        schema: manifest.schema,
        level: manifest.level,
        timeout: Duration::from_secs(timeout_secs),
    })
}

/// 命名约束（provider 最紧交集）：字母开头，仅 `[a-zA-Z0-9_-]`，
/// 长度 ≤ 64（OpenAI 函数名上限）。
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 文件是否可执行（unix 看任一 exec 位；其他平台视为可执行）。
fn is_executable(md: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        true
    }
}

fn timeout_text(name: &str, timeout: Duration, stderr: &str) -> String {
    let mut msg = format!("[ext:{name}] timed out after {}s", timeout.as_secs());
    let reason = stderr.trim();
    if !reason.is_empty() {
        msg.push_str(": ");
        msg.push_str(&truncate_chars(reason, MAX_ERROR_CHARS));
    }
    msg
}

fn failure_text(name: &str, code: Option<i32>, stderr: &str) -> String {
    let mut msg = match code {
        Some(c) => format!("[ext:{name}] exited with code {c}"),
        None => format!("[ext:{name}] terminated by signal"),
    };
    let reason = stderr.trim();
    if !reason.is_empty() {
        msg.push_str(": ");
        msg.push_str(&truncate_chars(reason, MAX_ERROR_CHARS));
    }
    msg
}

/// 截断到 `MAX_ERROR_CHARS`（按 char 边界）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

#[cfg(test)]
#[path = "ext_test.rs"]
mod tests;

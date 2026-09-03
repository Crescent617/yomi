//! hook/ — 文件系统 hook（一期：`pre_tool_use` 单点，Gate 端口的脚本形态）。
//!
//! 目录即注册表，零配置文件：`<data_dir>/hooks/pre_tool_use/` 下带执行位
//! 的普通文件按文件名字典序串行执行；执行位即开关，无 reload（每次事件
//! readdir，目录是真相）。**跟随符号链接**（stow/nix 式部署）；破损符号
//! 链接跳过不致命。
//!
//! stdin 契约（全项目统一 `snake_case`）：
//! ```json
//! {"session_id":"sess_...","cwd":"/work/dir","hook_event_name":"pre_tool_use",
//!  "tool_name":"shell","tool_input":{"command":"..."}}
//! ```
//! 退出码语义（对齐 Claude Code `PreToolUse`）：`0`=放行；`2`=否决，stderr
//! 即否决原因，回流为 tool error 喂回 agent；其他非零/超时/崩溃=hook 自身
//! 故障，warn 后放行（fail-open：否决必须是显式行为，坏脚本不瘫痪 agent）。
//!
//! 工具过滤下沉到脚本自身：stdin 里有 `tool_name`，不关心的工具直接
//! `exit 0`（shell 里一行 jq），内核不设 matcher 配置。
//!
//! # 与 Claude Code 的已知差异（移植 hook 前必读）
//!
//! - 无 `transcript_path` 字段（yomi 消息在 sqlite；要历史用
//!   `yomi session cat "$YOMI_SESSION_ID"`）
//! - 超时固定 30s（CC 默认 60s）：30–60s 的慢 guard 在 CC 是否决、在此被
//!   SIGKILL 后 fail-open 放行——语义反转，慢 hook 必须自行提速
//! - 不支持 CC 的 stdout JSON 高级协议（`permissionDecision`）；stdout 丢弃
//! - 非 0/2 退出码：CC 把 stderr 展示给用户，此处只进 tracing
//!
//! # 其他语义边界
//!
//! - **at-least-once**：进程在 hook 闸与结果落盘之间被杀，恢复后同一批
//!   call 会重过 hook——有副作用的 hook 须自行幂等
//! - **fail-open 的唯一观测渠道是 tracing 日志**（warn 级）：gate 失效
//!   （shebang 坏→spawn 失败→全放行）不在事件流上可见（MVP 拍板，一期
//!   不发诊断事件）
//! - 否决原因带 `[hook:<文件名>]` 前缀回流，人类可区分 hook 否决与普通
//!   工具失败（对照 permission 否决的 `Permission denied:` 前缀）

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::types::ToolCall;

// POSIX `kill(2)`：同 workflow/mod.rs 的做法，unix 上始终已链接，手动
// 声明以避免 libc/nix 依赖。setsid 已收敛到 `utils::process`。
#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
const SIGKILL: i32 = 9;

/// hook 库目录名（相对 `data_dir`）。
pub(crate) const DIR_NAME: &str = "hooks";
/// 一期唯一的 hook point 目录名（相对 hook 库目录）。
pub(crate) const POINT_PRE_TOOL_USE: &str = "pre_tool_use";

/// 子进程注入的 hook point 标识（值 = 目录名）。
pub(crate) const YOMI_HOOK_EVENT: &str = crate::env_name!("HOOK_EVENT");

/// 单 hook 执行上限：gate 在 agent 热路径上，不能给 workflow 的 5min。
const HOOK_TIMEOUT: Duration = Duration::from_secs(30);

/// 主进程退出/被杀后 drain 收尾的宽限期（同 workflow）。
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// drain 缓冲累积上限：超出继续读（防管道阻塞）但停止累积——坏脚本
/// `cat hugefile >&2` 不会在热路径上撑爆内存。
const DRAIN_CAP: usize = 64 * 1024;

/// 否决原因（hook stderr）回流给 agent 的长度上限。
const MAX_REASON_CHARS: usize = 2000;

/// stdin 负载（见模块 doc；`hook_event_name` 恒为 hook point 目录名）。
#[derive(Debug, serde::Serialize)]
pub struct PreToolUseInput {
    pub session_id: String,
    pub cwd: String,
    pub hook_event_name: &'static str,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

/// 单个 tool call 的裁决结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// 某 hook exit 2：内文即否决原因（含 `[hook:<文件名>]` 前缀），回流
    /// 为 tool error。
    Deny(String),
}

/// 一批 tool calls 的裁决结果：放行列表 + 否决列表（附原因）。
#[derive(Debug, Default)]
pub struct PreToolUseOutcome {
    pub approved: Vec<ToolCall>,
    pub denied: Vec<(ToolCall, String)>,
}

/// hook point 目录。
fn point_dir(data_dir: &Path, point: &str) -> PathBuf {
    data_dir.join(DIR_NAME).join(point)
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

/// 列出一个 hook point 的可执行脚本：普通文件（`metadata` 跟随符号链接）、
/// 带执行位、跳过隐藏文件，按文件名字典序；目录不存在视为空。
/// 单条目 metadata 失败（如破损符号链接）跳过而非整批失败——一个坏条目
/// 不该拆掉整道 gate。
async fn list_hooks(dir: &Path) -> crate::types::Result<Vec<PathBuf>> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut paths = Vec::new();
    while let Some(de) = rd.next_entry().await? {
        let Ok(name) = de.file_name().into_string() else {
            continue; // 非 UTF-8 文件名：无法排序寻址，跳过
        };
        if name.starts_with('.') {
            continue;
        }
        // 注意：`DirEntry::metadata` 是 lstat 语义（不跟随符号链接），
        // 必须对完整路径用 `fs::metadata` 才真正跟随。
        let md = match tokio::fs::metadata(de.path()).await {
            Ok(md) => md,
            Err(e) => {
                debug!(name = %name, error = %e, "pre_tool_use: skipping entry with unreadable metadata");
                continue;
            }
        };
        if !md.is_file() || !is_executable(&md) {
            continue;
        }
        paths.push(de.path());
    }
    paths.sort();
    Ok(paths)
}

/// 对一批 tool calls 跑 `pre_tool_use` hook 链。
///
/// 单 call 内按文件名序串行，首个 Deny 短路（后续 hook 不跑）；多 call
/// 间 MVP 串行循环（hook 通常毫秒级；并行是将来优化）。目录不存在/为空
/// 全部放行。`cancel` 生效时剩余 call 不再过闸、直接 flush 进 approved
/// ——下游权限检查与 `run_parallel` 持同一 token，会在执行前拦下它们。
pub async fn run_pre_tool_use(
    data_dir: &Path,
    session_id: &str,
    working_dir: &Path,
    calls: &[ToolCall],
    cancel: &CancellationToken,
) -> PreToolUseOutcome {
    let hooks = match list_hooks(&point_dir(data_dir, POINT_PRE_TOOL_USE)).await {
        Ok(h) => h,
        Err(e) => {
            // readdir 故障同 hook 故障：fail-open，但必须可见。
            warn!(error = %e, "pre_tool_use: failed to list hooks, allowing all");
            return PreToolUseOutcome {
                approved: calls.to_vec(),
                ..Default::default()
            };
        }
    };
    let mut outcome = PreToolUseOutcome::default();
    'calls: for (i, call) in calls.iter().enumerate() {
        let payload = PreToolUseInput {
            session_id: session_id.to_string(),
            cwd: working_dir.to_string_lossy().into_owned(),
            hook_event_name: POINT_PRE_TOOL_USE,
            tool_name: call.name.clone(),
            tool_input: call.arguments.clone(),
        };
        // 每 call 序列化一次，N 个 hook 复用同一份字节。
        let payload_json = match serde_json::to_vec(&payload) {
            Ok(j) => j,
            Err(e) => {
                warn!(tool = %call.name, error = %e, "pre_tool_use: payload serialize failed, allowing");
                outcome.approved.push(call.clone());
                continue 'calls;
            }
        };
        for hook in &hooks {
            let verdict = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    debug!("pre_tool_use: cancelled, flushing remaining calls ungated");
                    outcome.approved.extend_from_slice(&calls[i..]);
                    break 'calls;
                }
                v = run_one_hook(hook, &payload_json, POINT_PRE_TOOL_USE, working_dir, data_dir, session_id, HOOK_TIMEOUT) => v,
            };
            match verdict {
                Verdict::Allow => {}
                Verdict::Deny(reason) => {
                    info!(
                        hook = %hook.display(),
                        tool = %call.name,
                        "pre_tool_use: denied"
                    );
                    outcome.denied.push((call.clone(), reason));
                    continue 'calls;
                }
            }
        }
        outcome.approved.push(call.clone());
    }
    outcome
}

/// 执行单个 hook：喂 stdin JSON，捕获 stderr 作否决原因，超时强杀。
/// 故障一律 fail-open（warn + Allow）。
#[allow(clippy::too_many_arguments)]
async fn run_one_hook(
    path: &Path,
    stdin_json: &[u8],
    point: &str,
    cwd: &Path,
    data_dir: &Path,
    session_id: &str,
    timeout: Duration,
) -> Verdict {
    let mut cmd = tokio::process::Command::new(path);
    cmd.current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env(YOMI_HOOK_EVENT, point);
    crate::utils::env::inject_child_env(&mut cmd, Some(data_dir), Some(session_id));
    let mut child = {
        crate::utils::process::pre_exec_new_session(&mut cmd);
        match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                warn!(hook = %path.display(), error = %e, "pre_tool_use: spawn failed, allowing");
                return Verdict::Allow;
            }
        }
    };
    // 两管各自持续读空（管道不排空，写多的脚本会阻塞）：stdout 排空丢弃，
    // stderr 共享缓冲留存——即使 drain 宽限到期被迫 abort，已捕获的部分
    // 否决原因仍读得到（后裔持有管道不见 EOF 的场景）。
    let err_buf = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let mut drain_out = tokio::spawn(drain(
        child.stdout.take().expect("stdout piped"),
        Arc::new(tokio::sync::Mutex::new(Vec::new())),
    ));
    let mut drain_err = tokio::spawn(drain(
        child.stderr.take().expect("stderr piped"),
        Arc::clone(&err_buf),
    ));
    let mut stdin = child.stdin.take().expect("stdin piped");
    // spawn 要求 'static：字节按 hook 复制一份（memcpy 廉价；昂贵的
    // 序列化已在调用方按 call 只做一次）。
    let stdin_bytes = stdin_json.to_vec();
    let write = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt as _;
        // 脚本不读 stdin 时写会 BrokenPipe：正常场景，不算错误。
        let _ = stdin.write_all(&stdin_bytes).await;
        let _ = stdin.shutdown().await;
    });
    let wait = tokio::time::timeout(timeout, child.wait()).await;
    let (exit_code, timed_out) = match wait {
        Ok(Ok(status)) => (status.code(), false),
        Ok(Err(e)) => {
            warn!(hook = %path.display(), error = %e, "pre_tool_use: wait failed, allowing");
            write.abort();
            return Verdict::Allow;
        }
        Err(_) => {
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    // 进程组仍在（wait 未返回），按组强杀，连后裔一起。
                    unsafe { kill(-(pid as i32), SIGKILL) };
                }
                let _ = child.wait().await; // 收割僵尸
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill().await;
            }
            (None, true)
        }
    };
    write.abort();
    // 主进程已死：两管共享一段 drain 宽限收尾，到期放弃（detach 前显式
    // abort）；缓冲是共享的，abort 后已捕获内容仍在。
    let _ = tokio::join!(
        tokio::time::timeout(DRAIN_GRACE, &mut drain_out),
        tokio::time::timeout(DRAIN_GRACE, &mut drain_err),
    );
    drain_out.abort();
    drain_err.abort();

    if timed_out {
        warn!(hook = %path.display(), timeout_ms = timeout.as_millis(), "pre_tool_use: timed out, allowing");
        return Verdict::Allow;
    }
    let stderr = String::from_utf8_lossy(&err_buf.lock().await).into_owned();
    match exit_code {
        Some(0) => Verdict::Allow,
        Some(2) => {
            let name = path
                .file_name()
                .map_or_else(|| "?".to_string(), |n| n.to_string_lossy().into_owned());
            let reason = stderr.trim();
            Verdict::Deny(if reason.is_empty() {
                format!("[hook:{name}] denied without reason")
            } else {
                format!("[hook:{name}] {}", truncate_reason(reason))
            })
        }
        other => {
            warn!(
                hook = %path.display(),
                exit_code = ?other,
                stderr = %stderr.trim(),
                "pre_tool_use: hook failed, allowing"
            );
            Verdict::Allow
        }
    }
}

/// 持续读空一根管道并入共享缓冲；累积到 `DRAIN_CAP` 后继续读但停止
/// 累积（防管道阻塞的同时防内存放大）。
async fn drain<R>(mut pipe: R, buf: Arc<tokio::sync::Mutex<Vec<u8>>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut b = buf.lock().await;
                let room = DRAIN_CAP.saturating_sub(b.len());
                if room > 0 {
                    b.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
    }
}

/// 否决原因截断到 `MAX_REASON_CHARS`（按 char 边界）。
fn truncate_reason(reason: &str) -> String {
    if reason.chars().count() <= MAX_REASON_CHARS {
        return reason.to_string();
    }
    let truncated: String = reason.chars().take(MAX_REASON_CHARS).collect();
    format!("{truncated}…")
}

#[cfg(test)]
#[path = "hook_test.rs"]
mod tests;

//! hook/ — 文件系统 hook（Gate 端口 `pre_tool_use` + daemon 生命周期
//! 通知点 `daemon_up`/`daemon_down` 的脚本形态）。
//!
//! 目录即注册表，零配置文件：`<data_dir>/hooks/<point>/` 下的条目按
//! **条目名**字典序串行执行——条目为带执行位的裸文件，或含带执行位
//! `run` 的目录（与 tools 同约定，伴生文件放包里）。执行位即开关，无
//! reload（每次事件 readdir，目录是真相）。**跟随符号链接**（stow/nix
//! 式部署）；破损符号链接跳过不致命。
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
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::types::ToolCall;

/// hook 库目录名（相对 `data_dir`）。
pub(crate) const DIR_NAME: &str = "hooks";
/// 一期 gate hook point 目录名（相对 hook 库目录）。
pub(crate) const POINT_PRE_TOOL_USE: &str = "pre_tool_use";

/// daemon 生命周期 hook point（通知型，无否决语义）：
/// - `daemon_up`：服务就绪**后**触发（listeners 已绑、后台服务已起，
///   脚本可回连 CLI）；后台任务里跑，daemon 不等它——不挡开机。
/// - `daemon_down`：关停流程**中**触发（关停信号已到、kernel 拆除前；
///   此时 socket 仍在服务，脚本可回连 CLI），等它跑完再继续关停——
///   运行时退出会回收子进程，不等则清理不可靠。
///
/// 两个点共用 daemon 事件的精简契约（见 `run_daemon_point`）。
pub(crate) const POINT_DAEMON_UP: &str = "daemon_up";
pub(crate) const POINT_DAEMON_DOWN: &str = "daemon_down";

/// 子进程注入的 hook point 标识（值 = 目录名）。
pub(crate) const YOMI_HOOK_EVENT: &str = crate::env_name!("HOOK_EVENT");

/// 目录形态 hook 的入口文件名（与 tools 的 `run` 同约定）。
const RUN_FILE: &str = "run";

/// 单 hook 执行上限：gate 在 agent 热路径上，不能给 workflow 的 5min。
const HOOK_TIMEOUT: Duration = Duration::from_secs(30);

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

/// 列出一个 hook point 的可执行条目，按**条目名**字典序。条目两种形态：
/// 带执行位的裸文件，或含带执行位 `run` 的目录（目录形态与 tools 同约定
/// ——伴生文件放自己包里，`dirname "$0"` 即得）。目录不存在视为空。
/// 单条目 metadata 失败（如破损符号链接）跳过而非整批失败——一个坏条目
/// 不该拆掉整道 gate。`point` 仅用于日志归类（这是唯一的观测渠道）。
/// 返回 (条目名, 可执行路径)：state 目录/日志前缀按条目名（目录形态按
/// 目录名，不是 `run`）。
async fn list_hooks(dir: &Path, point: &str) -> crate::types::Result<Vec<(String, PathBuf)>> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut hooks = Vec::new();
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
                debug!(point, name = %name, error = %e, "hook: skipping entry with unreadable metadata");
                continue;
            }
        };
        if md.is_file() {
            if is_executable(&md) {
                hooks.push((name, de.path()));
            }
            continue;
        }
        if md.is_dir() {
            // 目录形态：<名>/run 带执行位才收；否则视为开关关/未就绪。
            let run = de.path().join(RUN_FILE);
            match tokio::fs::metadata(&run).await {
                Ok(rmd) if rmd.is_file() && is_executable(&rmd) => {
                    hooks.push((name, run));
                }
                _ => {
                    debug!(point, name = %name, "hook: skipping dir without executable {RUN_FILE}");
                }
            }
        }
    }
    hooks.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    // 同名条目（文件与目录同挂一名）：多半笔误。顺序已按路径兜底确定，
    // 但 state 目录同名共享、同一事件跑两遍——warn 留痕，值得修。
    for pair in hooks.windows(2) {
        if pair[0].0 == pair[1].0 {
            warn!(point, name = %pair[0].0, "hook: duplicate entry name (file and dir forms both present)");
        }
    }
    Ok(hooks)
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
    let hooks = match list_hooks(&point_dir(data_dir, POINT_PRE_TOOL_USE), POINT_PRE_TOOL_USE).await
    {
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
        for (name, hook) in &hooks {
            let verdict = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    debug!("pre_tool_use: cancelled, flushing remaining calls ungated");
                    outcome.approved.extend_from_slice(&calls[i..]);
                    break 'calls;
                }
                v = run_one_hook(name, hook, &payload_json, POINT_PRE_TOOL_USE, working_dir, data_dir, session_id, HOOK_TIMEOUT) => v,
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
/// 故障一律 fail-open（warn + Allow）。进程管线与 tools 共用
/// `utils::spawn` 引擎。
#[allow(clippy::too_many_arguments)]
async fn run_one_hook(
    name: &str,
    path: &Path,
    stdin_json: &[u8],
    point: &str,
    cwd: &Path,
    data_dir: &Path,
    session_id: &str,
    timeout: Duration,
) -> Verdict {
    // state 目录（state/hooks/<point>/<条目名>）惰性创建；失败不致命——
    // 脚本可自行 mkdir，缺个目录不拆 gate。
    let state_dir = data_dir.join("state").join(DIR_NAME).join(point).join(name);
    crate::utils::env::ensure_state_dir("hook", name, &state_dir).await;
    let mut cmd = tokio::process::Command::new(path);
    cmd.current_dir(cwd)
        .env(YOMI_HOOK_EVENT, point)
        .env(crate::utils::env::YOMI_EVENT, point);
    crate::utils::env::inject_child_env(&mut cmd, Some(data_dir), Some(session_id));
    crate::utils::env::inject_state_dir(&mut cmd, Some(&state_dir));
    let captured = match crate::utils::spawn::spawn_captured(
        &mut cmd,
        Some(stdin_json),
        timeout,
        None, // gate 在 agent 热路径上：30s 硬顶内跑完，不接取消（取消臂在调用方 select）
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            warn!(hook = %path.display(), error = %e, "pre_tool_use: engine failed, allowing");
            return Verdict::Allow;
        }
    };
    if captured.timed_out {
        warn!(hook = %path.display(), timeout_ms = timeout.as_millis(), "pre_tool_use: timed out, allowing");
        return Verdict::Allow;
    }
    let stderr = String::from_utf8_lossy(&captured.stderr).into_owned();
    match captured.exit_code {
        Some(0) => Verdict::Allow,
        Some(2) => {
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

/// 否决原因截断到 `MAX_REASON_CHARS`（按 char 边界）。
fn truncate_reason(reason: &str) -> String {
    if reason.chars().count() <= MAX_REASON_CHARS {
        return reason.to_string();
    }
    let truncated: String = reason.chars().take(MAX_REASON_CHARS).collect();
    format!("{truncated}…")
}

/// 跑一个 daemon 生命周期 hook point（`daemon_up`/`daemon_down`）：
/// `hooks/<point>/` 下可执行文件按文件名字典序串行，同一引擎、同一目录
/// 约定。与 gate 的差异——
///
/// - stdin 是精简契约 `{"event":"<point>","cwd":"<data_dir>"}`：无会话
///   语义，`YOMI_SESSION_ID` 显式移除（脚本 cwd 同为数据目录）；
/// - 无否决：非零/超时/spawn 故障只 warn 留痕，不中断后续脚本；
/// - 不接取消：down 钩子是关停清理，必须跑完（每条 30s 硬顶兜底）。
///
/// 调用方决定等不等（`daemon_up` 后台任务触发、`daemon_down` 关停路径
/// 上 await），本函数本身总是跑完整条链。
pub async fn run_daemon_point(data_dir: &Path, point: &str) {
    let hooks = match list_hooks(&point_dir(data_dir, point), point).await {
        Ok(h) => h,
        Err(e) => {
            warn!(point, error = %e, "daemon hook: failed to list hooks, skipping");
            return;
        }
    };
    if hooks.is_empty() {
        return;
    }
    let payload = serde_json::json!({
        "event": point,
        "cwd": data_dir.to_string_lossy(),
    });
    let payload = serde_json::to_vec(&payload).expect("daemon hook payload serializes");
    info!(point, count = hooks.len(), "daemon hook: running");
    for (name, hook) in &hooks {
        run_one_daemon_hook(name, hook, &payload, point, data_dir).await;
    }
}

/// 执行单个 daemon hook：通知语义，结果只留痕（warn/debug），不反馈给
/// 任何调用方。
async fn run_one_daemon_hook(
    name: &str,
    path: &Path,
    stdin_json: &[u8],
    point: &str,
    data_dir: &Path,
) {
    let state_dir = data_dir.join("state").join(DIR_NAME).join(point).join(name);
    crate::utils::env::ensure_state_dir("hook", name, &state_dir).await;
    let mut cmd = tokio::process::Command::new(path);
    cmd.current_dir(data_dir)
        .env(crate::utils::env::YOMI_EVENT, point)
        // 防残留：`YOMI_HOOK_EVENT` 是 pre_tool_use 的兼容变量，daemon
        // 点不注入——父进程若从 hook 环境继承，显式清掉。
        .env_remove(YOMI_HOOK_EVENT);
    crate::utils::env::inject_child_env(&mut cmd, Some(data_dir), None);
    crate::utils::env::inject_state_dir(&mut cmd, Some(&state_dir));
    let captured =
        match crate::utils::spawn::spawn_captured(&mut cmd, Some(stdin_json), HOOK_TIMEOUT, None)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!(hook = %path.display(), point, error = %e, "daemon hook: engine failed");
                return;
            }
        };
    if captured.timed_out {
        warn!(hook = %path.display(), point, timeout_ms = HOOK_TIMEOUT.as_millis(), "daemon hook: timed out (killed)");
        return;
    }
    match captured.exit_code {
        Some(0) => debug!(hook = %path.display(), point, "daemon hook: ok"),
        other => {
            let stderr = String::from_utf8_lossy(&captured.stderr);
            warn!(hook = %path.display(), point, exit_code = ?other, stderr = %stderr.trim(), "daemon hook: failed (ignored)");
        }
    }
}

#[cfg(test)]
#[path = "hook_test.rs"]
mod tests;

//! `card_trigger` —— 用户自定义飞书卡片触发器：渠道层的用户扩展点，与
//! kernel 接缝上的外挂（`hooks/` `tools/`）并列；spawn 引擎、环境变量
//! 与目录约定复用同一套。
//!
//! 注册表：`<data_dir>/channels/feishu_card_triggers/<名>`——带执行位
//! 的裸文件。执行位即开关，无 reload（每次点击实时解析），跟随符号
//! 链接，破损链接按未注册处理。
//!
//! 触发：卡片按钮 value 带 `{"action":"ext_<名>", ...}` 时由 hub 路由
//! 到此，value 全文 opaque 透传给脚本。非法名/未知名回一条拒绝提示。
//! 点击不过 channel 用户闸（`blocked_users`/`allowed_users` 不拦
//! `ext_`）——权限归脚本自管，拿 stdin 里的 `operator_open_id` 自查。
//!
//! stdin 契约（单行 JSON）：
//! ```json
//! {"event":"card_trigger","name":"publish","channel":"feishu",
//!  "operator_open_id":"ou_...","operator_union_id":null,
//!  "chat_id":"oc_...","message_id":"om_...","token":"c-...",
//!  "value":{"action":"ext_publish","id":1}}
//! ```
//! `token` 是回调 token（飞书延时更新卡片用，30 分钟有效，可能为
//! null）；窗口期外改卡用 `message_id` + im API（须发卡应用身份）。
//! 凭证不进 stdin——脚本自行经 lark-cli / `OpenAPI` 回连。
//!
//! 语义：通知型，无否决。exit 0 成功；非零/超时/spawn 故障只 warn
//! 留痕。30s 固定超时，at-least-once（副作用脚本自行幂等）。无会话
//! 语义：`YOMI_SESSION_ID` 显式移除，cwd 为数据目录；要驱动 agent
//! 走 `yomi session send`。`YOMI_STATE_DIR` =
//! `state/channels/feishu_card_triggers/<名>/`。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, warn};

use crate::channels::{CardAction, PlatformAdapter};
use crate::kernel::Kernel;

/// 注册表目录（相对 `<data_dir>/channels/`）。
const DIR_NAME: &str = "feishu_card_triggers";
/// stdin/env 的事件标识。
const EVENT_NAME: &str = "card_trigger";
/// 单次执行上限（与 hooks 同值）。
const TIMEOUT: Duration = Duration::from_secs(30);
/// 触发器名长度上限：自定值（非 provider 约束——名字只进按钮 value
/// 与文件路径；文件名硬边界 255 字节，128 留足余量兼挡垃圾超长串）。
const MAX_NAME_LEN: usize = 128;

/// `ext_<名>` 路由入口（hub 调用）：取数据目录后转 `dispatch`。
pub(crate) async fn handle_card_action(
    channel_name: &str,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: &CardAction,
) {
    let data_dir = kernel.data_dir().await;
    dispatch(&data_dir, channel_name, adapter, action).await;
}

/// 校验名 → 解析注册表 → spawn；任一步未命中回拒绝提示（best-effort）。
async fn dispatch(
    data_dir: &Path,
    channel_name: &str,
    adapter: &Arc<dyn PlatformAdapter>,
    action: &CardAction,
) {
    let raw = action.trigger_name().unwrap_or_default();
    let Some(name) = valid_name(raw) else {
        warn!(value = %action.value, "card trigger: invalid name in ext_ action");
        crate::channels::approval::send_action_denial(
            adapter,
            action,
            "Unknown card trigger.".to_string(),
        )
        .await;
        return;
    };
    let Some(path) = resolve(data_dir, name).await else {
        warn!(name, "card trigger: no executable registered");
        crate::channels::approval::send_action_denial(
            adapter,
            action,
            format!("Unknown card trigger: `{name}`."),
        )
        .await;
        return;
    };
    run(data_dir, channel_name, name, &path, action).await;
}

/// 触发器名合法性：字母开头、`[a-zA-Z0-9_-]`、≤128（字符集与 tools
/// 同交集；长度上限是自选值，见 `MAX_NAME_LEN`）。名字会拼进文件
/// 路径，必须挡掉 `.`/`/`/非 ASCII。
fn valid_name(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    ok.then_some(name)
}

/// 按名解析可执行入口：`<注册表>/<名>` 带执行位的文件。metadata 跟随
/// 符号链接；任何 IO 失败按未注册处理（注册表目录不存在是常态——
/// 用户没装触发器）。
async fn resolve(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let entry = data_dir.join("channels").join(DIR_NAME).join(name);
    let md = tokio::fs::metadata(&entry).await.ok()?;
    (md.is_file() && is_executable(&md)).then_some(entry)
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

/// stdin 负载（契约见模块 doc；Option 字段缺省序列化为 null）。
#[derive(serde::Serialize)]
struct TriggerInput<'a> {
    event: &'static str,
    name: &'a str,
    channel: &'a str,
    operator_open_id: &'a str,
    operator_union_id: Option<&'a str>,
    chat_id: Option<&'a str>,
    message_id: Option<&'a str>,
    token: Option<&'a str>,
    value: &'a serde_json::Value,
}

/// 执行触发器：通知型，结果只留痕（debug/warn）。env/cwd 语义与
/// daemon hook 相同：无会话，`YOMI_SESSION_ID` 与 `YOMI_HOOK_EVENT`
/// 显式移除防残留；state 目录惰性创建。
async fn run(data_dir: &Path, channel: &str, name: &str, path: &Path, action: &CardAction) {
    let payload = TriggerInput {
        event: EVENT_NAME,
        name,
        channel,
        operator_open_id: &action.operator_open_id,
        operator_union_id: action.operator_union_id.as_deref(),
        chat_id: action.chat_id.as_deref(),
        message_id: action.message_id.as_deref(),
        token: action.token.as_deref(),
        value: &action.value,
    };
    let stdin = match serde_json::to_vec(&payload) {
        Ok(j) => j,
        Err(e) => {
            warn!(name, error = %e, "card trigger: payload serialize failed");
            return;
        }
    };
    let state_dir = data_dir
        .join("state")
        .join("channels")
        .join(DIR_NAME)
        .join(name);
    crate::utils::env::ensure_state_dir("card_trigger", name, &state_dir).await;
    let mut cmd = tokio::process::Command::new(path);
    cmd.current_dir(data_dir)
        .env(crate::utils::env::YOMI_EVENT, EVENT_NAME)
        .env_remove(crate::hook::YOMI_HOOK_EVENT);
    crate::utils::env::inject_child_env(&mut cmd, Some(data_dir), None);
    crate::utils::env::inject_state_dir(&mut cmd, Some(&state_dir));
    let captured =
        match crate::utils::spawn::spawn_captured(&mut cmd, Some(&stdin), TIMEOUT, None).await {
            Ok(c) => c,
            Err(e) => {
                warn!(name, path = %path.display(), error = %e, "card trigger: engine failed");
                return;
            }
        };
    if captured.timed_out {
        warn!(name, path = %path.display(), timeout_ms = TIMEOUT.as_millis(), "card trigger: timed out (killed)");
        return;
    }
    match captured.exit_code {
        Some(0) => debug!(name, "card trigger: ok"),
        other => {
            let stderr = String::from_utf8_lossy(&captured.stderr);
            warn!(name, path = %path.display(), exit_code = ?other, stderr = %stderr.trim(), "card trigger: failed (ignored)");
        }
    }
}

#[cfg(all(test, unix))]
#[path = "card_trigger_test.rs"]
mod tests;

//! 入群欢迎卡：bot 被拉进群（`im.chat.member.bot.added_v1`）时发一张
//! 招呼卡。事件触发即发；投递 at-least-once，重连补投或移群再拉可能
//! 重复一张，可接受不另去重。特性开关：`disabled_events =
//! ["welcome"]`；名单外（`blocked_chats` / 非 `allowed_chats`）的群不发。
//!
//! 卡片内容两级：用户在 `<data_dir>/channels/welcome.json` 写了合法
//! 卡片 JSON 就原样发（文件即配置，与 `channels/rules/*.md` 同一
//! 思路）；没写或写坏了退回内置默认。文件里的 `{{name}}` 替换成
//! `[agent] name`（与 `system_prompt` 的 `{{name}}` 占位同源）。

use std::path::Path;
use std::sync::{Arc, Weak};

use serde_json::json;
use tracing::warn;

use crate::channels::hub_deliver::info_card_envelope;
use crate::channels::PlatformAdapter;
use crate::kernel::Kernel;

/// welcome.json 上限：卡片 JSON 本来就限长，读进来的文件 cap 一下，
/// 避免一个超大文件每次事件都被全量读进内存。
const WELCOME_JSON_MAX_BYTES: u64 = 32 * 1024;

/// 取 agent 名发欢迎卡。发送失败只记日志——欢迎卡不应影响主流程。
pub(crate) async fn send_welcome_card(
    adapter: &Arc<dyn PlatformAdapter>,
    kernel: &Weak<Kernel>,
    chat_id: &str,
) {
    let card = match kernel.upgrade() {
        Some(k) => {
            let data_dir = k.data_dir().await;
            resolve_welcome_card(&data_dir, &k.agent_name()).await
        }
        // kernel 已销毁：没有 data_dir 可查，退回内置默认。
        None => default_welcome_card("yomi"),
    };
    if let Err(e) = adapter.send_card(chat_id, &card, None).await {
        warn!(error = %e, "welcome card send failed");
    }
}

/// 决定发哪张卡：`<data_dir>/channels/welcome.json` 存在且是合法
/// JSON → 用户自定义卡（`{{name}}` 已替换）；缺失 / 读失败 / 非法
/// JSON → 内置默认。解析失败只告警不拒绝服务。
async fn resolve_welcome_card(data_dir: &Path, name: &str) -> String {
    let path = data_dir.join("channels").join("welcome.json");
    let raw = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return default_welcome_card(name);
        }
        Err(e) => {
            warn!(path = %path.display(), "welcome.json unreadable, using default: {e}");
            return default_welcome_card(name);
        }
    };
    let capped = raw
        .get(..(WELCOME_JSON_MAX_BYTES as usize).min(raw.len()))
        .unwrap_or(&[]);
    let text = String::from_utf8_lossy(capped).replace("{{name}}", name);
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(_) => text,
        Err(e) => {
            warn!(path = %path.display(), "welcome.json is not valid JSON, using default: {e}");
            default_welcome_card(name)
        }
    }
}

fn default_welcome_card(name: &str) -> String {
    let elements = vec![json!({
        "tag": "markdown",
        "content": "群里 @ 我即可，随叫随到 ✨",
    })];
    info_card_envelope(&format!("👋 大家好，我是{name}"), elements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_welcome_card_uses_configured_name() {
        let card = default_welcome_card("小嘟");
        assert!(card.contains("👋 大家好，我是小嘟"), "{card}");
        assert!(card.contains('@'), "{card}");
    }

    #[tokio::test]
    async fn custom_welcome_json_wins_and_substitutes_name() {
        let dir = tempfile::tempdir().unwrap();
        let channels = dir.path().join("channels");
        std::fs::create_dir_all(&channels).unwrap();
        std::fs::write(
            channels.join("welcome.json"),
            r#"{"header":{"title":{"tag":"plain_text","content":"嗨，{{name}}来了"}},"body":{}}"#,
        )
        .unwrap();

        let card = resolve_welcome_card(dir.path(), "小嘟").await;
        assert!(card.contains("嗨，小嘟来了"), "{card}");
        assert!(!card.contains("👋"), "{card}");
    }

    #[tokio::test]
    async fn missing_welcome_json_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let card = resolve_welcome_card(dir.path(), "小嘟").await;
        assert!(card.contains("👋 大家好，我是小嘟"), "{card}");
    }

    #[tokio::test]
    async fn invalid_welcome_json_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let channels = dir.path().join("channels");
        std::fs::create_dir_all(&channels).unwrap();
        std::fs::write(channels.join("welcome.json"), "{ not json").unwrap();

        let card = resolve_welcome_card(dir.path(), "小嘟").await;
        assert!(card.contains("👋 大家好，我是小嘟"), "{card}");
    }
}

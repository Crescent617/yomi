//! 入群欢迎卡：bot 被拉进群（`im.chat.member.bot.added_v1`）时的一
//! 张说明卡——做什么、消息表情图例、两个入口（`/settings` 面板、
//! `/help`）。事件触发即单次，无重发。

use serde_json::json;

use crate::channels::hub_deliver::info_card_envelope;

/// 欢迎卡正文。reaction 图例按平台传入（与 `/help` 同款，
/// [`crate::channels::PlatformConfig::reaction_legend`]）。
pub(crate) fn welcome_card(legend: &str) -> String {
    let elements = vec![json!({
        "tag": "markdown",
        "content": format!(
            "**@ me to ask anything** — in groups, every command needs an @.\n\
             **Reactions**: {legend}\n\
             `/settings` — panel: mention / reply-in-thread / model / context window / watch\n\
             `/help` — common commands · `/help all` — full list"
        ),
    })];
    info_card_envelope("👋 Hi, I'm yomi", elements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_card_carries_legend_and_entry_points() {
        let card = welcome_card("👀 accepted · ✅ run done");
        assert!(card.contains("👋 Hi, I'm yomi"), "{card}");
        assert!(card.contains("👀 accepted · ✅ run done"), "{card}");
        assert!(card.contains("/settings"), "{card}");
        assert!(card.contains("/help all"), "{card}");
    }
}

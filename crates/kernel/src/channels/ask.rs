//! agent 决策卡（`ask_user` 工具的飞书渲染）：问题文本 + 选项按钮，
//! 回答经 `ask_answer` 回调注入 `AskUserResponse`；工具结束（应答 /
//! 超时 / 取消）发出的 `AskUserAck` 把残留卡片 patch 成关闭态，不
//! 留僵尸按钮（daemon 重启丢注册表时，点击回落到"已关闭"提示）。
//! 回答者与 run 可见者同级（路由层 user 门限，不叠加 admin）——与
//! Stop 按钮同档。multi-select 问题 v1 按单选渲染（回答为单个
//! label，agent 侧照常可读）。

use std::sync::Arc;

use dashmap::DashMap;
use serde_json::json;
use tracing::warn;

use crate::kernel::Kernel;
use crate::tools::AskQuestion;
use crate::types::{ContentBlock, SessionId};

use super::hub_deliver::info_card_envelope;
use super::{CardAction, PlatformAdapter};

/// req_id → 已发问题卡（多问题时一题一张；关闭/回答时需要知道
/// patch 哪几张、各自属于哪一问）。
#[derive(Default)]
pub(crate) struct AskCardRegistry {
    cards: DashMap<String, Vec<AskCardRef>>,
}

struct AskCardRef {
    msg_id: String,
    header: String,
    /// 已被回答 patch（Ack 关闭时跳过，保住 ✅ 终态）。
    answered: bool,
}

/// 单问题卡：问题 → 选项说明（灰字）→ 选项按钮行（weighted 平分，
/// default small）→ 自定义回答提示。
fn question_card(sid: &SessionId, req_id: &str, q: &AskQuestion) -> String {
    let mut elements =
        vec![json!({ "tag": "markdown", "text_size": "notation", "content": q.question })];
    let desc: Vec<String> = q
        .options
        .iter()
        .map(|o| format!("· **{}** — {}", o.label, o.description))
        .collect();
    elements.push(json!({
        "tag": "markdown", "text_size": "notation",
        "content": format!("<font color='grey'>{}</font>", desc.join("\n")),
    }));
    let columns: Vec<serde_json::Value> = q
        .options
        .iter()
        .map(|o| {
            json!({
                "tag": "column", "width": "weighted", "weight": 1,
                "elements": [{
                    "tag": "button",
                    "text": { "tag": "plain_text", "content": o.label },
                    "type": "default",
                    "size": "small",
                    "behaviors": [{ "type": "callback", "value": {
                        "action": "ask_answer", "req": req_id, "sid": sid.0,
                        "q": q.question, "label": o.label,
                    } }],
                }],
            })
        })
        .collect();
    elements.push(json!({ "tag": "column_set", "columns": columns }));
    elements.push(json!({
        "tag": "markdown", "text_size": "notation",
        "content": "<font color='grey'>Buttons answer directly · times out in 2 min</font>",
    }));
    info_card_envelope(&format!("❓ {}", q.header), elements)
}

fn closed_card(header: &str, line: &str) -> String {
    info_card_envelope(
        &format!("❓ {header}"),
        vec![json!({
            "tag": "markdown", "text_size": "notation",
            "content": format!("<font color='grey'>{line}</font>"),
        })],
    )
}

impl AskCardRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 渲染并发送问题卡（每题一张），登记 req_id → 卡片引用。
    pub(crate) async fn send_question_cards(
        &self,
        adapter: &Arc<dyn PlatformAdapter>,
        chat_id: &str,
        reply_msg_id: Option<&str>,
        sid: &SessionId,
        req_id: &str,
        questions: &[AskQuestion],
    ) {
        let mut refs = Vec::new();
        for q in questions {
            let card = question_card(sid, req_id, q);
            match adapter.send_card(chat_id, &card, reply_msg_id).await {
                Ok(Some(msg_id)) => refs.push(AskCardRef {
                    msg_id,
                    header: q.header.clone(),
                    answered: false,
                }),
                Ok(None) => return, // 无卡平台，无可登记
                Err(e) => {
                    warn!(error = %e, "ask question card send failed");
                    return;
                }
            }
        }
        if !refs.is_empty() {
            self.cards.insert(req_id.to_string(), refs);
        }
    }

    /// `AskUserAck`（应答/超时/取消）：关闭残留卡片（已被回答的那张
    /// 保留 ✅ 终态）。
    pub(crate) async fn close_cards(&self, adapter: &Arc<dyn PlatformAdapter>, req_id: &str) {
        let Some((_, refs)) = self.cards.remove(req_id) else {
            return;
        };
        for r in refs.iter().filter(|r| !r.answered) {
            if let Err(e) = adapter
                .update_card(&r.msg_id, &closed_card(&r.header, "Closed."))
                .await
            {
                warn!(error = %e, "ask card close patch failed");
            }
        }
    }

    /// `ask_answer` 回调：注入 `AskUserResponse` 并把该卡 patch 成
    /// ✅ 终态；其余卡由随后的 `AskUserAck` 关闭。
    pub(crate) async fn handle_answer(
        &self,
        kernel: &Arc<Kernel>,
        adapter: &Arc<dyn PlatformAdapter>,
        action: &CardAction,
    ) {
        let value = &action.value;
        if value["action"].as_str() != Some("ask_answer") {
            warn!(value = %value, "unrecognized ask card action");
            return;
        }
        let req = value["req"].as_str().unwrap_or_default();
        let sid = SessionId::from(value["sid"].as_str().unwrap_or_default().to_string());
        let q = value["q"].as_str().unwrap_or_default();
        let label = value["label"].as_str().unwrap_or_default();
        if req.is_empty() || sid.0.is_empty() || q.is_empty() || label.is_empty() {
            warn!(value = %value, "ask answer action missing fields");
            return;
        }
        if !self.cards.contains_key(req) {
            // 已回答过 / 超时 / daemon 重启丢了注册表。
            if let Some(chat_id) = &action.chat_id {
                let _ = adapter
                    .send_message(
                        chat_id,
                        vec![ContentBlock::Text {
                            text: "⚠️ This question is already closed.".to_string(),
                        }],
                        None,
                    )
                    .await;
            }
            return;
        }
        let answers = std::collections::HashMap::from([(q.to_string(), label.to_string())]);
        if let Err(e) =
            kernel.send_ask_user_response(&sid, req, crate::tools::AskUserResponse { answers })
        {
            warn!(error = %e, "ask answer publish failed");
            return;
        }
        if let Some(msg_id) = &action.message_id {
            if let Some(mut refs) = self.cards.get_mut(req) {
                if let Some(r) = refs.iter_mut().find(|r| &r.msg_id == msg_id) {
                    r.answered = true;
                    if let Err(e) = adapter
                        .update_card(msg_id, &closed_card(&r.header, &format!("✅ {label}")))
                        .await
                    {
                        warn!(error = %e, "ask card answer patch failed");
                    }
                }
            }
        }
    }
}

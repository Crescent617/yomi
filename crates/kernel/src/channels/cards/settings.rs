//! `/settings` — chat-scope 配置面板卡：mention / reply-in-thread /
//! model / context-window 四行覆盖型 `select_static`（on/off/default(x)、
//! 模型 key 列表、25–100% 窗口档位；auto 列宽自适应）+ watch 两态行
//! （on/off，无 default——watched set 即全部状态；仅群聊渲染，on 时附
//! 一行 notation 说明 mention/rit 挂起中），底部 ♻️ Reset all（只清四个
//! 覆盖，不动 watch 模式）/ 🔄 Refresh。`cfg_*` 回调执行后原地刷新
//! （"点击即切换"的实质：执行 → 重读状态 → update_card）。
//! 配置修改限 admin（与 `/mention` `/threads` 命令同档）；路由层
//! user 门限对所有按钮生效。
//!
//! 群/私判定随卡往返：卡片无法从 `chat_id` 推回群聊/私聊，回调值一律带
//! `dm` 标志供重渲染与 `cfg_watch` 拒绝私聊翻转（`/watch` 命令在私聊
//! 同样拒绝——私聊开了 watch，连唯一的关闭入口都没了）。缺失按私聊处理
//! （保守方向：旧卡最多少一行，绝不给私聊开出 watch）。标志可被伪造，
//! 但 admin 本就有 RPC 直达路径——它是 UI 保护，不是安全边界。

use std::sync::Arc;

use serde_json::json;
use tracing::warn;

use crate::kernel::Kernel;
use crate::types::ContentBlock;
use crate::types::Result as KernelResult;

use crate::channels::hub_deliver::info_card_envelope;
use crate::channels::hub_routing::read_mention_override;
use crate::channels::{
    CardAction, ChannelConfig, ChannelMessage, ChannelStore, MappingKind, PlatformAdapter,
};

/// 面板管理的配置项的当前状态（chat scope；`None` = 跟随 channel
/// default）。
struct SettingsState {
    mention_override: Option<bool>,
    rit_override: Option<bool>,
    model_override: Option<String>,
    default_mention: bool,
    default_rit: bool,
    default_model: String,
    models: Vec<String>,
    /// 当前 chat session 的 context-window 覆盖（settings 袋）。
    ctx_override: Option<u32>,
    /// 解析到当前模型（覆盖或默认）的配置窗口，预设档位的基准。
    model_context_window: u32,
    /// 当前 chat 的 watch 模式（mapping kind 直读，两态无 default）；
    /// 是否渲染成行由卡片按 `is_group` 决定。
    watch_on: bool,
}

async fn read_state(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    store: &Arc<dyn ChannelStore>,
    chat_id: &str,
) -> KernelResult<SettingsState> {
    let mention_override = read_mention_override(store, channel_name, chat_id).await;
    let rit_override = store.get_rit_override(channel_name, chat_id).await?;
    // The chat-level session's raw model_key: `None` means "follow the
    // default" — distinct from an explicit choice that happens to equal
    // it (which would stop tracking default changes).
    let session = match store.find_mapping(channel_name, chat_id).await? {
        Some(sid) => kernel.session_store().await.get(&sid).await.ok().flatten(),
        None => None,
    };
    let model_override = session.as_ref().and_then(|info| info.model_key.clone());
    let ctx_override = session
        .and_then(|info| info.settings)
        .and_then(|s| s.context_window);
    let models_info = kernel.list_models().await?;
    // 档位基准 = 解析后模型的配置窗口（与 resolve_model 同回落：覆盖 key
    // 失效时回落默认模型）。
    let default_model = kernel.default_model_key();
    let resolved_key = model_override
        .as_deref()
        .filter(|k| models_info.iter().any(|m| &m.name == k))
        .unwrap_or(&default_model);
    let model_context_window = models_info
        .iter()
        .find(|m| m.name == resolved_key)
        .map_or(crate::compactor::DEFAULT_CONTEXT_WINDOW, |m| {
            m.context_window
        });
    let models = models_info.into_iter().map(|m| m.name).collect();
    let watch_on = matches!(
        store.find_mapping_kind(channel_name, chat_id).await?,
        Some((_, MappingKind::Watch))
    );
    Ok(SettingsState {
        mention_override,
        rit_override,
        model_override,
        default_mention: config.require_mention,
        default_rit: config.reply_in_thread,
        default_model,
        models,
        ctx_override,
        model_context_window,
        watch_on,
    })
}

fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

/// One label + select_static row: label `auto` (natural width), select
/// `weighted` (stretches over the rest — an auto select gets squeezed
/// invisible in the narrow thread side panel, verified). A STABLE
/// `element_id` is mandatory: the client tracks per-select chosen state
/// by it, and auto-assigned ids shift on every patch — without one the
/// visible selection drifts to a wrong option and the client may even
/// skip the callback ("no change"). The current value rides `value`
/// (the option value, not `initial_index`: that one is 1-based in
/// practice and displayed the wrong option — verified).
fn select_row(
    element_id: &str,
    label: &str,
    options: &[String],
    initial: usize,
    callback_value: serde_json::Value,
) -> serde_json::Value {
    let opts: Vec<serde_json::Value> = options
        .iter()
        .map(|o| {
            json!({
                "text": { "tag": "plain_text", "content": o },
                "value": o,
            })
        })
        .collect();
    json!({
        "tag": "column_set",
        "columns": [
            {
                "tag": "column", "width": "auto", "vertical_align": "center",
                "elements": [{ "tag": "markdown", "text_size": "notation", "content": label }],
            },
            {
                // weighted (not auto): in the narrow thread side panel an
                // auto column gets squeezed until the selected key is
                // invisible — a weighted column always keeps its share.
                "tag": "column", "width": "weighted", "weight": 1, "vertical_align": "center",
                "elements": [{
                    "tag": "select_static",
                    "element_id": element_id,
                    "placeholder": { "tag": "plain_text", "content": options[initial] },
                    "options": opts,
                    "value": options[initial],
                    "behaviors": [{ "type": "callback", "value": callback_value }],
                }],
            },
        ],
    })
}

fn settings_card(chat_id: &str, is_group: bool, state: &SettingsState) -> String {
    let tri_options = |default_val: bool| {
        vec![
            "on".to_string(),
            "off".to_string(),
            format!("default ({})", on_off(default_val)),
        ]
    };
    let tri_initial = |override_val: Option<bool>| match override_val {
        Some(true) => 0,
        Some(false) => 1,
        None => 2,
    };
    let mut elements = vec![
        select_row(
            "cfg_mention",
            "Mention required",
            &tri_options(state.default_mention),
            tri_initial(state.mention_override),
            json!({ "action": "cfg_set", "key": "mention", "scope": chat_id, "dm": !is_group }),
        ),
        select_row(
            "cfg_threads",
            "Reply in thread",
            &tri_options(state.default_rit),
            tri_initial(state.rit_override),
            json!({ "action": "cfg_set", "key": "threads", "scope": chat_id, "dm": !is_group }),
        ),
    ];
    // Model row: every configured key + the reset pseudo-option.
    let mut model_options = state.models.clone();
    model_options.push(format!("default ({})", state.default_model));
    let model_initial = match &state.model_override {
        Some(key) => state
            .models
            .iter()
            .position(|m| m == key)
            .unwrap_or(state.models.len()),
        None => state.models.len(),
    };
    elements.push(select_row(
        "cfg_model",
        "Model",
        &model_options,
        model_initial,
        json!({ "action": "cfg_model", "scope": chat_id, "dm": !is_group }),
    ));
    // Context window row: presets keyed to the resolved model's
    // configured window — 25/50/75/100% + the reset pseudo-option.
    // Labels carry the percentage; the callback recomputes tokens off
    // the THEN-current model default. A custom value set elsewhere
    // (TUI/GUI/CLI) shows as a `custom (Nk)` option so the visible
    // selection never lies (selecting it is a no-op).
    let fmt_k = |t: u64| {
        if t % 1000 == 0 {
            format!("{}k", t / 1000)
        } else {
            format!("{:.1}k", t as f64 / 1000.0)
        }
    };
    let mut ctx_labels: Vec<String> = [25u32, 50, 75, 100]
        .iter()
        .map(|p| {
            format!(
                "{} ({p}%)",
                fmt_k(u64::from(state.model_context_window) * u64::from(*p) / 100)
            )
        })
        .collect();
    let mut ctx_initial = usize::MAX; // unset → the default pseudo-option
    if let Some(ov) = state.ctx_override {
        match ctx_labels
            .iter()
            .position(|l| parse_ctx_label(l, state.model_context_window) == CtxOp::Set(ov))
        {
            Some(i) => ctx_initial = i,
            None => {
                ctx_labels.insert(0, format!("custom ({})", fmt_k(u64::from(ov))));
                ctx_initial = 0;
            }
        }
    }
    ctx_labels.push(format!(
        "default ({})",
        fmt_k(u64::from(state.model_context_window))
    ));
    if ctx_initial == usize::MAX {
        ctx_initial = ctx_labels.len() - 1;
    }
    elements.push(select_row(
        "cfg_ctx",
        "Context window",
        &ctx_labels,
        ctx_initial,
        json!({ "action": "cfg_ctx", "scope": chat_id, "dm": !is_group }),
    ));
    // Watch row: two-state, no `default` pseudo-option — the watched set
    // is the whole state (see `/watch`). Groups only: the command
    // refuses DMs, so the card must not open a flip there either (in a
    // DM the off switch would vanish with it). While on, a notation line
    // names the rows watch suspends: mention/rit gate conversation
    // replies, which watch replaces with the observer's own voice
    // (model/ctx stay live — the observer is a real session).
    if is_group {
        elements.push(select_row(
            "cfg_watch",
            "Watch",
            &["on".to_string(), "off".to_string()],
            // off → index 1, on → index 0 (options are ["on", "off"]).
            usize::from(!state.watch_on),
            json!({ "action": "cfg_watch", "scope": chat_id, "dm": false }),
        ));
        if state.watch_on {
            elements.push(json!({
                "tag": "markdown",
                "text_size": "notation",
                "content": "👁 Watching — non-command messages are mirrored to the observer session; **Mention required** and **Reply in thread** don't apply until watch is off.",
            }));
        }
    }
    // Footer: global actions, mailbox-style small bordered buttons.
    elements.push(json!({
        "tag": "column_set",
        "columns": [
            {
                "tag": "column", "width": "weighted", "weight": 1,
                "elements": [{
                    "tag": "button",
                    "text": { "tag": "plain_text", "content": "♻️ Reset all" },
                    "type": "default",
                    "size": "small",
                    "behaviors": [{ "type": "callback", "value": { "action": "cfg_reset_all", "scope": chat_id, "dm": !is_group } }],
                }],
            },
            {
                "tag": "column", "width": "weighted", "weight": 1,
                "elements": [{
                    "tag": "button",
                    "text": { "tag": "plain_text", "content": "🔄 Refresh" },
                    "type": "default",
                    "size": "small",
                    "behaviors": [{ "type": "callback", "value": { "action": "cfg_refresh", "scope": chat_id, "dm": !is_group } }],
                }],
            },
        ],
    }));
    info_card_envelope("⚙️ Settings · this chat", elements)
}

/// `/settings` 命令主体（admin 门槛在命令臂，此处只管执行）。
pub(crate) async fn handle_settings_command(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    reply_msg_id: Option<String>,
) -> KernelResult<Option<String>> {
    let chat_id = &msg.external_chat_id;
    let state = read_state(channel_name, config, kernel, store, chat_id).await?;
    adapter
        .send_card(
            chat_id,
            &settings_card(chat_id, msg.is_group, &state),
            reply_msg_id.as_deref(),
        )
        .await?;
    Ok(None)
}

/// `cfg_*` 按钮/下拉回调：执行变更后原地刷新这张卡片（与 mailbox 卡
/// 同一约定——不自动跟踪变更，别处改了配置点 🔄 Refresh）。
pub(crate) async fn handle_card_action(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: CardAction,
) {
    if let Err(e) =
        handle_card_action_inner(channel_name, config, kernel, store, adapter, &action).await
    {
        warn!(channel = %channel_name, error = %e, "settings card action failed");
    }
}

async fn handle_card_action_inner(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: &CardAction,
) -> KernelResult<()> {
    let value = &action.value;
    let chat_id = value["scope"].as_str().unwrap_or_default();
    if chat_id.is_empty() {
        warn!(value = %value, "settings card action missing scope");
        return Ok(());
    }
    // 群/私判定随卡往返（见模块 doc）：缺失按私聊处理（保守方向）。
    let dm = value["dm"].as_bool().unwrap_or(true);
    if let Some(deny) = crate::channels::approval::check_admin(config, &action.operator_open_id) {
        crate::channels::approval::send_action_denial(adapter, action, deny).await;
        return Ok(());
    }
    match value["action"].as_str() {
        Some("cfg_set") => {
            let key = value["key"].as_str().unwrap_or_default();
            let opt = value["option"].as_str().unwrap_or_default();
            match (key, map_cfg_set(opt)) {
                ("mention", CfgSetOp::Set(v)) => {
                    store.set_mention_override(channel_name, chat_id, v).await?
                }
                ("mention", CfgSetOp::Clear) => {
                    store.clear_mention_override(channel_name, chat_id).await?
                }
                ("threads", CfgSetOp::Set(v)) => {
                    store.set_rit_override(channel_name, chat_id, v).await?
                }
                ("threads", CfgSetOp::Clear) => {
                    store.clear_rit_override(channel_name, chat_id).await?
                }
                (key, op) => {
                    warn!(key, ?op, "unknown cfg_set key/option");
                    return Ok(());
                }
            }
        }
        Some("cfg_model") => {
            let opt = value["option"].as_str().unwrap_or_default();
            // A transient list_models failure must NOT fall through to a
            // reset — nothing is touched unless we can tell keys apart.
            let models: Vec<String> = match kernel.list_models().await {
                Ok(ms) => ms.into_iter().map(|m| m.name).collect(),
                Err(e) => {
                    warn!(error = %e, "cfg_model: list_models failed, model untouched");
                    return Ok(());
                }
            };
            match map_cfg_model(&models, opt) {
                Some(key) => {
                    crate::channels::hub_handlers::set_chat_model(
                        channel_name,
                        store,
                        kernel,
                        chat_id,
                        key,
                    )
                    .await?
                }
                None => {
                    warn!(opt, "cfg_model: unknown option, model untouched");
                    return Ok(());
                }
            }
        }
        Some("cfg_ctx") => {
            let opt = value["option"].as_str().unwrap_or_default();
            // Tokens are recomputed off the CURRENT model default (the
            // card may be stale), never parsed from the label's k-text.
            let model_default = read_state(channel_name, config, kernel, store, chat_id)
                .await?
                .model_context_window;
            match parse_ctx_label(opt, model_default) {
                CtxOp::Set(tokens) => {
                    crate::channels::hub_handlers::set_chat_context_window(
                        channel_name,
                        store,
                        kernel,
                        chat_id,
                        Some(tokens),
                    )
                    .await?
                }
                CtxOp::Clear => {
                    crate::channels::hub_handlers::set_chat_context_window(
                        channel_name,
                        store,
                        kernel,
                        chat_id,
                        None,
                    )
                    .await?
                }
                CtxOp::Noop => {}
            }
        }
        Some("cfg_watch") => {
            // 私聊卡（或 dm 标志缺失的旧卡）拒绝翻转——与 `/watch`
            // 命令的私聊拒绝同规。
            if dm {
                warn!("cfg_watch ignored: DM scope");
                return Ok(());
            }
            let opt = value["option"].as_str().unwrap_or_default();
            match map_cfg_watch(opt) {
                Some(on) => {
                    // 陈旧卡可能重发当前态：真翻转才执行、才留 ack。
                    // （残余竞态，接受：并发翻转若恰好插进预读与
                    // set_channel_watch_by_name 的路由锁之间，set 幂等
                    // 收敛、状态永不分叉，但 ack 可能重复一条——让
                    // setter 上报 flipped 得动 wire 可见的
                    // ChannelWatchStatus，为装饰性重复不值。）
                    let current = matches!(
                        store.find_mapping_kind(channel_name, chat_id).await?,
                        Some((_, MappingKind::Watch))
                    );
                    if current != on {
                        crate::channels::hub::watch::set_channel_watch_by_name(
                            store,
                            kernel,
                            channel_name,
                            chat_id,
                            on,
                        )
                        .await?;
                        // 翻转决定 bot 在本群沉默与否——必须留群里可见
                        // 的痕迹（与 `/watch` 命令同一文案）。
                        adapter
                            .send_message(
                                chat_id,
                                vec![ContentBlock::Text {
                                    text: crate::channels::hub::watch::flip_ack_text(on),
                                }],
                                None,
                            )
                            .await?;
                    }
                }
                None => {
                    warn!(opt, "unknown cfg_watch option, watch untouched");
                    return Ok(());
                }
            }
        }
        Some("cfg_reset_all") => {
            store.clear_mention_override(channel_name, chat_id).await?;
            store.clear_rit_override(channel_name, chat_id).await?;
            crate::channels::hub_handlers::set_chat_model(
                channel_name,
                store,
                kernel,
                chat_id,
                None,
            )
            .await?;
            crate::channels::hub_handlers::set_chat_context_window(
                channel_name,
                store,
                kernel,
                chat_id,
                None,
            )
            .await?;
        }
        Some("cfg_refresh") => {}
        other => {
            warn!(value = %value, "unrecognized settings card action {other:?}");
            return Ok(());
        }
    }
    if let Some(message_id) = &action.message_id {
        let state = read_state(channel_name, config, kernel, store, chat_id).await?;
        adapter
            .update_card(message_id, &settings_card(chat_id, !dm, &state))
            .await?;
    }
    Ok(())
}

/// cfg_set tri-state mapping: `on`/`off` set the override, the
/// `default (…)` pseudo-option clears it, anything else (missing or
/// malformed option) is a no-op — never an accidental reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CfgSetOp {
    Set(bool),
    Clear,
    Noop,
}

fn map_cfg_set(opt: &str) -> CfgSetOp {
    match opt {
        "on" => CfgSetOp::Set(true),
        "off" => CfgSetOp::Set(false),
        o if o.starts_with("default (") => CfgSetOp::Clear,
        _ => CfgSetOp::Noop,
    }
}

/// cfg_model mapping: `Some(Some(key))` switch, `Some(None)` reset
/// (the `default (…)` pseudo-option), `None` no-op. Callers must not
/// reach here on a `list_models` failure (handled upstream).
fn map_cfg_model<'a>(models: &[String], opt: &'a str) -> Option<Option<&'a str>> {
    if opt.starts_with("default (") {
        Some(None)
    } else if models.iter().any(|m| m == opt) {
        Some(Some(opt))
    } else {
        None
    }
}

/// `cfg_watch` mapping: `on`/`off` flip, anything else is a no-op — never
/// an accidental mode flip. Two-state by design: watch has no channel
/// default, so there is no reset pseudo-option to map.
fn map_cfg_watch(opt: &str) -> Option<bool> {
    match opt {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// cfg_ctx 档位解析的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CtxOp {
    Set(u32),
    Clear,
    Noop,
}

/// cfg_ctx label → operation：档位标签形如 `200k (25%)`，按其中的百分比
/// 乘当前模型默认窗口得 tokens（不解析 k 文本——卡片可能是旧的）；
/// `default (…)` 伪选项清除覆盖；**算出来正好等于模型默认（100% 档）也
/// 视为清除**——把等于默认的值钉成显式覆盖会断送对默认变化的跟踪；
/// `custom (…)`（别处设的精确值）与任何畸形标签一律 no-op，绝不误清。
fn parse_ctx_label(opt: &str, model_default: u32) -> CtxOp {
    if opt.starts_with("default (") {
        return CtxOp::Clear;
    }
    let pct = opt
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once('('))
        .and_then(|(_, tail)| tail.strip_suffix('%'))
        .and_then(|n| n.parse::<u32>().ok());
    match pct {
        Some(p @ 1..=100) if opt.contains("k (") => {
            let tokens = (u64::from(model_default) * u64::from(p) / 100) as u32;
            if tokens == model_default {
                CtxOp::Clear
            } else {
                CtxOp::Set(tokens)
            }
        }
        _ => CtxOp::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(mention: Option<bool>, rit: Option<bool>, model: Option<&str>) -> SettingsState {
        SettingsState {
            mention_override: mention,
            rit_override: rit,
            model_override: model.map(str::to_string),
            default_mention: true,
            default_rit: false,
            default_model: "k3-hs".to_string(),
            models: vec!["k3-hs".to_string(), "opus-4-6".to_string()],
            ctx_override: None,
            model_context_window: 800_000,
            watch_on: false,
        }
    }

    /// Collect every select_static in the card: (placeholder, options, current value, callback value).
    fn selects_of(card: &str) -> Vec<(String, Vec<String>, String, serde_json::Value)> {
        fn walk(
            v: &serde_json::Value,
            out: &mut Vec<(String, Vec<String>, String, serde_json::Value)>,
        ) {
            if v["tag"] == "select_static" {
                out.push((
                    v["placeholder"]["content"].as_str().unwrap().to_string(),
                    v["options"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|o| o["value"].as_str().unwrap().to_string())
                        .collect(),
                    v["value"].as_str().unwrap().to_string(),
                    v["behaviors"][0]["value"].clone(),
                ));
            }
            if let Some(arr) = v.as_array() {
                for e in arr {
                    walk(e, out);
                }
            }
            if let Some(obj) = v.as_object() {
                for e in obj.values() {
                    walk(e, out);
                }
            }
        }
        let v: serde_json::Value = serde_json::from_str(card).unwrap();
        let mut out = Vec::new();
        walk(&v, &mut out);
        out
    }

    /// Collect every button in the card: (text, callback value) —
    /// parsed-field assertions, never substring matching on serialized
    /// key order (`serde_json` flips to insertion order if any dep ever
    /// enables `preserve_order`).
    fn buttons_of(v: &serde_json::Value, out: &mut Vec<(String, serde_json::Value)>) {
        if v["tag"] == "button" {
            out.push((
                v["text"]["content"].as_str().unwrap().to_string(),
                v["behaviors"][0]["value"].clone(),
            ));
        }
        if let Some(arr) = v.as_array() {
            for e in arr {
                buttons_of(e, out);
            }
        }
        if let Some(obj) = v.as_object() {
            for e in obj.values() {
                buttons_of(e, out);
            }
        }
    }

    #[test]
    fn card_renders_tri_state_and_model_selection() {
        let card = settings_card("oc_1", true, &state(Some(false), None, Some("opus-4-6")));
        let s = selects_of(&card);
        assert_eq!(s.len(), 5, "{card}");

        // Mention overridden off → selects "off".
        assert_eq!(s[0].0, "off");
        assert_eq!(
            s[0].1,
            ["on", "off", "default (on)"],
            "default label carries the channel default"
        );
        assert_eq!(s[0].2, "off", "current value rides `value`");
        assert_eq!(s[0].3["key"], "mention");
        assert_eq!(s[0].3["scope"], "oc_1");

        // Rit unset → the default pseudo-option, labeled with the default.
        assert_eq!(s[1].0, "default (off)");
        assert_eq!(s[1].2, "default (off)");

        // Model override → that key; options carry the default tail.
        assert_eq!(s[2].0, "opus-4-6");
        assert_eq!(s[2].1, ["k3-hs", "opus-4-6", "default (k3-hs)"]);
        assert_eq!(s[2].2, "opus-4-6");

        // Ctx unset → presets keyed to the model window + default pseudo.
        assert_eq!(s[3].0, "default (800k)");
        assert_eq!(
            s[3].1,
            [
                "200k (25%)",
                "400k (50%)",
                "600k (75%)",
                "800k (100%)",
                "default (800k)"
            ]
        );
        assert_eq!(s[3].2, "default (800k)");
        assert_eq!(s[3].3["action"], "cfg_ctx");

        // Label carries no baked "now" — the selection (preset / custom /
        // default pseudo) already states the effective value, and the
        // card is stale-tolerant by contract (🔄 Refresh).
        assert!(card.contains("Context window"), "{card}");
        assert!(!card.contains("(now"), "{card}");

        // Watch row: two-state, no default pseudo-option, dm flag rides.
        assert_eq!(s[4].0, "off");
        assert_eq!(s[4].1, ["on", "off"]);
        assert_eq!(s[4].2, "off");
        assert_eq!(s[4].3["action"], "cfg_watch");
        assert_eq!(s[4].3["dm"], false);
        assert!(!card.contains("👁 Watching"), "{card}");
    }

    #[test]
    fn card_ctx_row_marks_preset_override_and_custom_value() {
        // Override exactly on a preset → that preset is the selection.
        let mut st = state(None, None, None);
        st.ctx_override = Some(400_000);
        let card = settings_card("oc_1", true, &st);
        let s = selects_of(&card);
        assert_eq!(s[3].0, "400k (50%)");
        assert_eq!(s[3].2, "400k (50%)");
        assert!(!s[3].1.iter().any(|l| l.starts_with("custom (")));

        // Off-preset override (set via TUI/GUI/CLI) → honest custom option.
        st.ctx_override = Some(320_000);
        let card = settings_card("oc_1", true, &st);
        let s = selects_of(&card);
        assert_eq!(s[3].0, "custom (320k)");
        assert_eq!(s[3].1[0], "custom (320k)");
        assert_eq!(s[3].2, "custom (320k)");
    }

    #[test]
    fn card_watch_row_marks_on_and_explains_suspension() {
        let mut st = state(None, None, None);
        st.watch_on = true;
        let card = settings_card("oc_1", true, &st);
        let s = selects_of(&card);
        assert_eq!(s[4].0, "on");
        assert_eq!(s[4].2, "on");
        // The mutex note appears only while watching, naming the two
        // rows watch suspends by their label.
        assert!(card.contains("👁 Watching"), "{card}");
        assert!(card.contains("**Mention required**"), "{card}");
        assert!(card.contains("**Reply in thread**"), "{card}");
    }

    #[test]
    fn dm_card_hides_watch_row_and_flags_callbacks() {
        // Even a watched chat (reachable via RPC) renders no watch row
        // in DM scope — the off switch must not be offered there.
        let mut st = state(None, None, None);
        st.watch_on = true;
        let card = settings_card("oc_1", false, &st);
        let s = selects_of(&card);
        assert_eq!(s.len(), 4, "{card}");
        assert!(!card.contains("cfg_watch"), "{card}");
        assert!(!card.contains("👁 Watching"), "{card}");
        assert!(
            s.iter().all(|sel| sel.3["dm"] == true),
            "every callback value carries dm:true for re-render, {card}"
        );
    }

    #[test]
    fn parse_ctx_label_maps_presets_pseudo_and_garbage() {
        assert_eq!(parse_ctx_label("200k (25%)", 800_000), CtxOp::Set(200_000));
        // 100% 档 == 默认值 → 清除而非钉死（保留对默认变化的跟踪）。
        assert_eq!(parse_ctx_label("800k (100%)", 800_000), CtxOp::Clear);
        // 百分比乘的是当前模型默认，不是标签里的 k 文本。
        assert_eq!(parse_ctx_label("200k (25%)", 128_000), CtxOp::Set(32_000));
        assert_eq!(parse_ctx_label("default (800k)", 800_000), CtxOp::Clear);
        // custom/畸形/0% 一律 no-op，绝不误清。
        assert_eq!(parse_ctx_label("custom (320k)", 800_000), CtxOp::Noop);
        assert_eq!(parse_ctx_label("0k (0%)", 800_000), CtxOp::Noop);
        assert_eq!(parse_ctx_label("", 800_000), CtxOp::Noop);
        assert_eq!(parse_ctx_label("200k", 800_000), CtxOp::Noop);
    }

    #[test]
    fn card_defaults_point_at_reset_pseudo_option() {
        let card = settings_card("oc_1", true, &state(None, None, None));
        let s = selects_of(&card);
        assert_eq!(s[0].0, "default (on)");
        assert_eq!(s[1].0, "default (off)");
        // No override: the default pseudo-option is the current value.
        assert_eq!(s[2].0, "default (k3-hs)");
        assert_eq!(s[2].2, "default (k3-hs)");
    }

    #[test]
    fn footer_buttons_carry_scope_and_small_size() {
        let card = settings_card("oc_1", true, &state(None, None, None));
        let v: serde_json::Value = serde_json::from_str(&card).unwrap();
        let mut buttons = Vec::new();
        buttons_of(&v, &mut buttons);
        assert_eq!(buttons.len(), 2, "{card}");
        assert_eq!(buttons[0].0, "♻️ Reset all");
        assert_eq!(
            buttons[0].1,
            serde_json::json!({"action": "cfg_reset_all", "scope": "oc_1", "dm": false})
        );
        assert_eq!(buttons[1].0, "🔄 Refresh");
        assert_eq!(
            buttons[1].1,
            serde_json::json!({"action": "cfg_refresh", "scope": "oc_1", "dm": false})
        );
        let json = v.to_string();
        assert_eq!(json.matches("\"size\":\"small\"").count(), 2, "{json}");
    }

    #[test]
    fn cfg_set_mapping_only_resets_on_default_pseudo_label() {
        assert_eq!(map_cfg_set("on"), CfgSetOp::Set(true));
        assert_eq!(map_cfg_set("off"), CfgSetOp::Set(false));
        assert_eq!(map_cfg_set("default (on)"), CfgSetOp::Clear);
        assert_eq!(map_cfg_set("default (off)"), CfgSetOp::Clear);
        // Missing/malformed values must never fall through to a reset.
        assert_eq!(map_cfg_set(""), CfgSetOp::Noop);
        assert_eq!(map_cfg_set("k3"), CfgSetOp::Noop);
        assert_eq!(map_cfg_set("default"), CfgSetOp::Noop);
    }

    #[test]
    fn cfg_model_mapping_distinguishes_keys_reset_and_noop() {
        let models = vec!["k3-hs".to_string(), "opus-4-6".to_string()];
        assert_eq!(map_cfg_model(&models, "k3-hs"), Some(Some("k3-hs")));
        assert_eq!(
            map_cfg_model(&models, "default (k3-hs)"),
            Some(None),
            "pseudo-label resets"
        );
        assert_eq!(map_cfg_model(&models, "no-such-model"), None);
        assert_eq!(map_cfg_model(&models, ""), None);
    }

    #[test]
    fn cfg_watch_mapping_only_flips_on_exact_on_off() {
        assert_eq!(map_cfg_watch("on"), Some(true));
        assert_eq!(map_cfg_watch("off"), Some(false));
        // 任何其他值（包括伪造的 default 伪选项）一律 no-op，绝不误翻转。
        assert_eq!(map_cfg_watch(""), None);
        assert_eq!(map_cfg_watch("default (off)"), None);
        assert_eq!(map_cfg_watch("ON"), None);
    }
}

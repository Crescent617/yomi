//! `/settings` — 配置面板卡，作用域跟随命令落点（与 `/model` `/mailbox`
//! 等 session-addressing 命令同规）：chat 顶层调用 = chat 作用域（标题
//! "this chat"），thread 内调用 = thread 作用域（标题 "this thread"）。
//! 标题只写真实作用域——凡在标题里标作用域的卡都遵此约。
//!
//! 行集合按作用域粒度裁剪（chat 卡五行 + footer，thread 卡三行 +
//! footer）：
//! - **Mention required**（chat+thread）：container 粒度——thread 有
//!   自己的覆盖（按 `thread_id` 键，回落 chat 覆盖 → 频道默认，见
//!   `resolve_require_mention`；`/mention` 命令在 thread 里写的就是
//!   这个键）。`default (x)` 的 x 是回落生效值，不是裸频道默认。
//! - **Reply in thread**（chat only）：chat 级覆盖，thread 无此维度
//!   （"threads carry no own override"，见 `handle_threads_command`）。
//! - **Model / Context window**（chat+thread）：session 粒度。chat 卡
//!   写 chat session 并 fan-out 本 chat 全部 thread session（未来
//!   thread 建行时继承）；thread 卡只写本 thread session——无
//!   session 时读显示继承生效值（chat session 的覆盖，即首条消息实
//!   际会用到的值，选中项恒表达生效值），写先 materialize（继承先
//!   应用、显式选择赢），clear/Reset 落在纯 default 状态上是 no-op
//!   不白建行——与 `/model` 在 thread 下无 session 时的建行同一条
//!   规则。
//! - **Watch**（chat 群聊 only）：chat 级开关，两态无 default（watched
//!   set 即全部状态）；thread/私聊不可开（`/watch` 命令同规拒绝）；
//!   on 时附一行 notation 说明 mention/rit 挂起中。
//! - footer：♻️ Reset all（只清本作用域的覆盖，不动 watch）/ 🔄 Refresh。
//!
//! `cfg_*` 回调执行后原地刷新（"点击即切换"的实质：执行 → 重读状态 →
//! `update_card`）。配置修改限 admin（与 `/mention` `/threads` 命令同
//! 档）；路由层 user 门限对所有按钮生效。
//!
//! 作用域的唯一载体是 [`Scope`]：渲染时构造一次，随每个回调值往返，
//! 回调入口解析一次，读状态/渲染/写路径都以它为单位。三个键在
//! thread 卡上各不相同（chat 卡三者同为 `chat_id`），`dm`/`thread` 是
//! UI 保护标志而非安全边界（可被伪造，但 admin 本就有 RPC 直达路
//! 径）。旧卡只有扁平 `scope`+`dm`——缺失字段一律向保守方向回落
//! （`container`/`chat` 回落 `session`、`th` 回落 chat、`dm` 回落私
//! 聊），旧卡语义不变。

use std::sync::Arc;

use serde_json::json;
use tracing::warn;

use crate::kernel::Kernel;
use crate::types::Result as KernelResult;

use crate::channels::hub_deliver::info_card_envelope;
use crate::channels::hub_routing::{
    effective_mapping_key, get_or_create_session, read_mention_override, resolve_reply_in_thread,
};
use crate::channels::{
    CardAction, ChannelConfig, ChannelMessage, ChannelStore, MappingKind, PlatformAdapter,
};

/// 卡片的作用域：渲染时构造一次，随每个回调值往返，回调入口解析一
/// 次——读状态、渲染、写路径都以它为单位（见模块 doc）。thread 卡
/// 上三个键各不相同：
/// - `chat`：真实 chat id——thread 无自身 session 时继承生效值的读
///   取对象、materialize 时继承的来源；
/// - `session`：session mapping key（thread root id）——model/ctx 的
///   读写目标；
/// - `container`：mention 容器键（`thread_id`）——mention 行的读写目
///   标（与 `history_container` 同取法）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scope {
    chat: String,
    session: String,
    container: String,
    thread: bool,
    dm: bool,
}

impl Scope {
    fn chat_scope(chat_id: &str, dm: bool) -> Self {
        Self {
            chat: chat_id.to_string(),
            session: chat_id.to_string(),
            container: chat_id.to_string(),
            thread: false,
            dm,
        }
    }

    fn thread_scope(chat_id: &str, session: &str, container: &str, dm: bool) -> Self {
        Self {
            chat: chat_id.to_string(),
            session: session.to_string(),
            container: container.to_string(),
            thread: true,
            dm,
        }
    }

    /// 随卡往返的线格式解析。缺失字段向保守方向回落（见模块 doc）；
    /// 事件自带的 `chat_id` 是 `chat` 的第二来源（`chat` 字段不在卡上
    /// 时的旧卡/异常卡）。
    fn from_value(value: &serde_json::Value, event_chat_id: Option<&str>) -> Option<Self> {
        let non_empty = |key: &str| value[key].as_str().filter(|s| !s.is_empty());
        let session = non_empty("scope")?;
        let container = non_empty("container").unwrap_or(session);
        let chat = non_empty("chat")
            .or(event_chat_id.filter(|s| !s.is_empty()))
            .unwrap_or(session);
        Some(Self {
            chat: chat.to_string(),
            session: session.to_string(),
            container: container.to_string(),
            thread: value["th"].as_bool().unwrap_or(false),
            dm: value["dm"].as_bool().unwrap_or(true),
        })
    }

    /// 每行回调值的公共部分；行专属字段由调用方补上（如 `cfg_set` 的
    /// `key`）。
    fn callback(&self, action: &str) -> serde_json::Value {
        json!({
            "action": action,
            "scope": self.session,
            "container": self.container,
            "chat": self.chat,
            "dm": self.dm,
            "th": self.thread,
        })
    }
}

/// 面板管理的配置项的当前状态（`None` = 跟随 default）。thread scope
/// 下 mention 读自 thread 容器（`default_mention` 取回落生效值：chat
/// 覆盖 ?? 频道默认），model/ctx 读自本 thread session（无 session 时
/// 回落 chat session 的覆盖——继承生效值）；rit/watch 是 chat-only，
/// 不读不渲染。
struct SettingsState {
    mention_override: Option<bool>,
    rit_override: Option<bool>,
    model_override: Option<String>,
    default_mention: bool,
    default_rit: bool,
    default_model: String,
    models: Vec<String>,
    /// 作用域 session 的 context-window 覆盖（settings 袋）。
    ctx_override: Option<u32>,
    /// 解析到当前模型（覆盖或默认）的配置窗口，预设档位的基准。
    model_context_window: u32,
    /// chat 的 watch 模式（mapping kind 直读，两态无 default）；是否渲
    /// 染成行由卡片按 scope 决定。
    watch_on: bool,
}

async fn read_state(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    store: &Arc<dyn ChannelStore>,
    scope: &Scope,
) -> KernelResult<SettingsState> {
    // mention 是 container 粒度（thread 有自己的覆盖），两种卡都读。
    let mention_override = read_mention_override(store, channel_name, &scope.container).await;
    // default 伪选项标签里的回落生效值：chat 卡 = 频道默认；thread 卡
    // = chat 覆盖 ?? 频道默认（与 resolve_require_mention 的回落链一致）。
    let default_mention = if scope.thread {
        read_mention_override(store, channel_name, &scope.chat)
            .await
            .unwrap_or(config.require_mention)
    } else {
        config.require_mention
    };
    // rit 是 chat-only，thread 卡不读不渲染。
    let rit_override = if scope.thread {
        None
    } else {
        store.get_rit_override(channel_name, &scope.session).await?
    };
    // The scoped session's raw model_key: `None` means "follow the
    // default" — distinct from an explicit choice that happens to equal
    // it (which would stop tracking default changes).
    let mut session = match store.find_mapping(channel_name, &scope.session).await? {
        Some(sid) => kernel.session_store().await.get(&sid).await.ok().flatten(),
        None => None,
    };
    // Thread 卡无自身 session：读 chat session 的覆盖——首条消息经
    // overrides_for_new_channel_session 继承到的就是它，选中项恒表达
    // 生效值。已有自身 session（即使无覆盖）不回落：继承只在建行时
    // 发生，现存 session 的 None 就是跟随配置默认。
    if session.is_none() && scope.thread {
        session = match store.find_mapping(channel_name, &scope.chat).await? {
            Some(sid) => kernel.session_store().await.get(&sid).await.ok().flatten(),
            None => None,
        };
    }
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
    // watch 是 chat 级开关（thread 上不允许开），thread 卡不读不渲染。
    let watch_on = !scope.thread
        && matches!(
            store
                .find_mapping_kind(channel_name, &scope.session)
                .await?,
            Some((_, MappingKind::Watch))
        );
    Ok(SettingsState {
        mention_override,
        rit_override,
        model_override,
        default_mention,
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

/// One label + `select_static` row: label `auto` (natural width), select
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

/// `cfg_set` 行专属字段：回调公共部分（[`Scope::callback`]）+ 覆盖项名。
fn set_row(scope: &Scope, key: &str) -> serde_json::Value {
    let mut v = scope.callback("cfg_set");
    v["key"] = json!(key);
    v
}

fn settings_card(scope: &Scope, state: &SettingsState) -> String {
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
    // mention 是 container 粒度（thread 有自己的覆盖），两种卡都渲染。
    let mut elements = vec![select_row(
        "cfg_mention",
        "Mention required",
        &tri_options(state.default_mention),
        tri_initial(state.mention_override),
        set_row(scope, "mention"),
    )];
    // rit 是 chat-only，thread 卡不渲染。
    if !scope.thread {
        elements.push(select_row(
            "cfg_threads",
            "Reply in thread",
            &tri_options(state.default_rit),
            tri_initial(state.rit_override),
            set_row(scope, "threads"),
        ));
    }
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
        scope.callback("cfg_model"),
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
        scope.callback("cfg_ctx"),
    ));
    // Watch row: two-state, no `default` pseudo-option — the watched set
    // is the whole state (see `/watch`). Chat-scope groups only: the
    // command refuses DMs and threads alike, so the card must not open
    // a flip there either (in a DM the off switch would vanish with it;
    // from a thread the flip would silently swallow the whole chat).
    // While on, a notation line names the rows watch suspends: mention/
    // rit gate conversation replies, which watch replaces with the
    // observer's own voice (model/ctx stay live — the observer is a
    // real session).
    if !scope.dm && !scope.thread {
        elements.push(select_row(
            "cfg_watch",
            "Watch",
            &["on".to_string(), "off".to_string()],
            // off → index 1, on → index 0 (options are ["on", "off"]).
            usize::from(!state.watch_on),
            scope.callback("cfg_watch"),
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
                    "behaviors": [{ "type": "callback", "value": scope.callback("cfg_reset_all") }],
                }],
            },
            {
                "tag": "column", "width": "weighted", "weight": 1,
                "elements": [{
                    "tag": "button",
                    "text": { "tag": "plain_text", "content": "🔄 Refresh" },
                    "type": "default",
                    "size": "small",
                    "behaviors": [{ "type": "callback", "value": scope.callback("cfg_refresh") }],
                }],
            },
        ],
    }));
    // 标题只写真实作用域（见模块 doc）。
    let title = if scope.thread {
        "⚙️ Settings · this thread"
    } else {
        "⚙️ Settings · this chat"
    };
    info_card_envelope(title, elements)
}

/// `/settings` 命令主体（admin 门槛在命令臂，此处只管执行）。作用域
/// 跟随命令落点（见模块 doc）：thread 内调用 = thread 作用域。
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
    let dm = !msg.is_group;
    let scope = if msg.thread_id.is_some() {
        let rit = resolve_reply_in_thread(store, config, chat_id).await;
        // mention 容器与 history_container 同取法（thread_id）；session
        // 键与 /subscribe 同取法（effective_mapping_key，thread root）。
        Scope::thread_scope(
            chat_id,
            &effective_mapping_key(store, adapter, channel_name, msg, chat_id, rit).await?,
            msg.thread_id.as_deref().unwrap_or_default(),
            dm,
        )
    } else {
        Scope::chat_scope(chat_id, dm)
    };
    let state = read_state(channel_name, config, kernel, store, &scope).await?;
    adapter
        .send_card(
            chat_id,
            &settings_card(&scope, &state),
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
    let Some(scope) = Scope::from_value(value, action.chat_id.as_deref()) else {
        warn!(value = %value, "settings card action missing scope");
        return Ok(());
    };
    if let Some(deny) = crate::channels::approval::check_admin(config, &action.operator_open_id) {
        crate::channels::approval::send_action_denial(adapter, action, deny).await;
        return Ok(());
    }
    match value["action"].as_str() {
        Some("cfg_set") => {
            let key = value["key"].as_str().unwrap_or_default();
            // rit 是 chat-only——thread 卡不渲染这行，回调臂同样拒绝
            // （伪造/陈旧值不写出垃圾键）。mention 是 container 粒度，
            // thread 卡写 thread 容器自己的覆盖。
            if scope.thread && key != "mention" {
                warn!(value = %value, "cfg_set ignored at thread scope");
                return Ok(());
            }
            let opt = value["option"].as_str().unwrap_or_default();
            match (key, map_cfg_set(opt)) {
                ("mention", CfgSetOp::Set(v)) => {
                    store
                        .set_mention_override(channel_name, &scope.container, v)
                        .await?;
                }
                ("mention", CfgSetOp::Clear) => {
                    store
                        .clear_mention_override(channel_name, &scope.container)
                        .await?;
                }
                ("threads", CfgSetOp::Set(v)) => {
                    store
                        .set_rit_override(channel_name, &scope.session, v)
                        .await?;
                }
                ("threads", CfgSetOp::Clear) => {
                    store
                        .clear_rit_override(channel_name, &scope.session)
                        .await?;
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
                Some(key) => write_model(channel_name, config, kernel, store, &scope, key).await?,
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
            let model_default = read_state(channel_name, config, kernel, store, &scope)
                .await?
                .model_context_window;
            match parse_ctx_label(opt, model_default) {
                CtxOp::Set(tokens) => {
                    write_ctx(channel_name, config, kernel, store, &scope, Some(tokens)).await?;
                }
                CtxOp::Clear => {
                    write_ctx(channel_name, config, kernel, store, &scope, None).await?;
                }
                CtxOp::Noop => {}
            }
        }
        Some("cfg_watch") => {
            // 私聊卡（dm 标志缺失的旧卡同）与 thread 卡拒绝翻转——与
            // `/watch` 命令的私聊/thread 拒绝同规。
            if scope.dm || scope.thread {
                warn!(value = %value, "cfg_watch ignored: DM/thread scope");
                return Ok(());
            }
            let opt = value["option"].as_str().unwrap_or_default();
            match map_cfg_watch(opt) {
                // set_channel_watch_by_name 幂等收敛（路由锁 + no-change
                // 纯 no-op），陈旧卡重发当前态无需预读。翻转结果由卡片
                // 原地刷新表达（on 时说明行出现）——卡片改动从不在群里
                // 另发消息（与 mention/rit/model/ctx 同行惯例）；群里可
                // 见的解释由 `/watch` 命令的 ack 承担（命令回复是命令的
                // 惯例），面板自身由说明行承担。
                Some(on) => {
                    crate::channels::hub::watch::set_channel_watch_by_name(
                        store,
                        kernel,
                        channel_name,
                        &scope.session,
                        on,
                    )
                    .await?;
                }
                None => {
                    warn!(opt, "unknown cfg_watch option, watch untouched");
                    return Ok(());
                }
            }
        }
        Some("cfg_reset_all") => reset_all(channel_name, config, kernel, store, &scope).await?,
        Some("cfg_refresh") => {}
        other => {
            warn!(value = %value, "unrecognized settings card action {other:?}");
            return Ok(());
        }
    }
    if let Some(message_id) = &action.message_id {
        let state = read_state(channel_name, config, kernel, store, &scope).await?;
        adapter
            .update_card(message_id, &settings_card(&scope, &state))
            .await?;
    }
    Ok(())
}

/// thread scope 的写入锚点：无 session 的 thread 在写入时建行——继承
/// （`overrides_for_new_channel_session`）在建行时先应用，随后的显式
/// set/clear 覆盖之。读路径（渲染/Refresh）永不调用本函数。
async fn materialize_scope_session(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    scope: &Scope,
) -> KernelResult<crate::types::SessionId> {
    Ok(get_or_create_session(
        channel_name,
        store,
        kernel,
        &scope.chat,
        &scope.session,
        None,
        MappingKind::Normal,
    )
    .await?
    .0)
}

/// 一次 model 覆盖写入。chat scope：`set_chat_model` 扇出本 chat 全部
/// thread session；thread scope：只写本 thread session——无 session
/// 时 materialize 再落；clear 落在纯 default 状态上是 no-op，不白建
/// 行（见模块 doc）。
async fn write_model(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    store: &Arc<dyn ChannelStore>,
    scope: &Scope,
    key: Option<&str>,
) -> KernelResult<()> {
    if !scope.thread {
        return crate::channels::hub_handlers::set_chat_model(
            channel_name,
            store,
            kernel,
            &scope.session,
            key,
        )
        .await;
    }
    let dirty = match key {
        Some(_) => true,
        None => read_state(channel_name, config, kernel, store, scope)
            .await?
            .model_override
            .is_some(),
    };
    if dirty {
        let sid = materialize_scope_session(channel_name, store, kernel, scope).await?;
        match key {
            Some(k) => kernel.set_session_model(&sid, k).await?,
            None => kernel.clear_session_model(&sid).await?,
        }
    }
    Ok(())
}

/// 一次 ctx 覆盖写入（作用域语义同 [`write_model`]）。
async fn write_ctx(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    store: &Arc<dyn ChannelStore>,
    scope: &Scope,
    tokens: Option<u32>,
) -> KernelResult<()> {
    if !scope.thread {
        return crate::channels::hub_handlers::set_chat_context_window(
            channel_name,
            store,
            kernel,
            &scope.session,
            tokens,
        )
        .await;
    }
    let dirty = match tokens {
        Some(_) => true,
        None => read_state(channel_name, config, kernel, store, scope)
            .await?
            .ctx_override
            .is_some(),
    };
    if dirty {
        let sid = materialize_scope_session(channel_name, store, kernel, scope).await?;
        kernel.set_session_context_window(&sid, tokens).await?;
    }
    Ok(())
}

/// ♻️ Reset all：清本作用域的全部覆盖（不动 watch）——chat scope 清
/// mention/rit/model/ctx 四项；thread scope 清 mention（thread 容器
/// 键）与本 thread session 的 model/ctx（复用 write_* 的 no-op 豁
/// 免，纯 default 状态下不白建行）。
async fn reset_all(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    store: &Arc<dyn ChannelStore>,
    scope: &Scope,
) -> KernelResult<()> {
    if read_mention_override(store, channel_name, &scope.container)
        .await
        .is_some()
    {
        store
            .clear_mention_override(channel_name, &scope.container)
            .await?;
    }
    if !scope.thread {
        store
            .clear_rit_override(channel_name, &scope.session)
            .await?;
    }
    write_model(channel_name, config, kernel, store, scope, None).await?;
    write_ctx(channel_name, config, kernel, store, scope, None).await
}

/// `cfg_set` tri-state mapping: `on`/`off` set the override, the
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

/// `cfg_model` mapping: `Some(Some(key))` switch, `Some(None)` reset
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

/// `cfg_ctx` 档位解析的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CtxOp {
    Set(u32),
    Clear,
    Noop,
}

/// `cfg_ctx` label → operation：档位标签形如 `200k (25%)`，按其中的百分比
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

    fn chat_scope() -> Scope {
        Scope::chat_scope("oc_1", false)
    }

    fn dm_scope() -> Scope {
        Scope::chat_scope("oc_1", true)
    }

    fn thread_scope() -> Scope {
        Scope::thread_scope("oc_1", "om_root", "omt_9", false)
    }

    /// Collect every `select_static` in the card: (placeholder, options, current value, callback value).
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
        let card = settings_card(&chat_scope(), &state(Some(false), None, Some("opus-4-6")));
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
        assert_eq!(s[0].3["container"], "oc_1");
        assert_eq!(s[0].3["chat"], "oc_1");
        assert_eq!(s[0].3["th"], false, "chat card callbacks carry th:false");

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
        assert!(card.contains("⚙️ Settings · this chat"), "{card}");

        // Watch row: two-state, no default pseudo-option, scope fields ride.
        assert_eq!(s[4].0, "off");
        assert_eq!(s[4].1, ["on", "off"]);
        assert_eq!(s[4].2, "off");
        assert_eq!(s[4].3["action"], "cfg_watch");
        assert_eq!(s[4].3["dm"], false);
        assert_eq!(s[4].3["th"], false);
        assert!(!card.contains("👁 Watching"), "{card}");
    }

    #[test]
    fn card_ctx_row_marks_preset_override_and_custom_value() {
        // Override exactly on a preset → that preset is the selection.
        let mut st = state(None, None, None);
        st.ctx_override = Some(400_000);
        let card = settings_card(&chat_scope(), &st);
        let s = selects_of(&card);
        assert_eq!(s[3].0, "400k (50%)");
        assert_eq!(s[3].2, "400k (50%)");
        assert!(!s[3].1.iter().any(|l| l.starts_with("custom (")));

        // Off-preset override (set via TUI/GUI/CLI) → honest custom option.
        st.ctx_override = Some(320_000);
        let card = settings_card(&chat_scope(), &st);
        let s = selects_of(&card);
        assert_eq!(s[3].0, "custom (320k)");
        assert_eq!(s[3].1[0], "custom (320k)");
        assert_eq!(s[3].2, "custom (320k)");
    }

    #[test]
    fn card_watch_row_marks_on_and_explains_suspension() {
        let mut st = state(None, None, None);
        st.watch_on = true;
        let card = settings_card(&chat_scope(), &st);
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
        let card = settings_card(&dm_scope(), &st);
        let s = selects_of(&card);
        assert_eq!(s.len(), 4, "{card}");
        assert!(!card.contains("cfg_watch"), "{card}");
        assert!(!card.contains("👁 Watching"), "{card}");
        assert!(
            s.iter()
                .all(|sel| sel.3["dm"] == true && sel.3["th"] == false),
            "every callback value carries dm:true and th:false, {card}"
        );
    }

    #[test]
    fn thread_card_is_thread_scoped_and_hides_chat_level_rows() {
        // rit/watch 即使有值也不渲染（chat-only）；mention 是
        // container 粒度，thread 卡保留（写 thread 容器键）。
        let mut st = state(Some(false), Some(true), Some("opus-4-6"));
        st.watch_on = true;
        st.ctx_override = Some(400_000);
        let card = settings_card(&thread_scope(), &st);
        assert!(card.contains("⚙️ Settings · this thread"), "{card}");
        assert!(!card.contains("this chat"), "{card}");

        let s = selects_of(&card);
        assert_eq!(s.len(), 3, "{card}");
        assert_eq!(s[0].3["action"], "cfg_set");
        assert_eq!(s[0].3["key"], "mention");
        assert_eq!(s[0].3["container"], "omt_9");
        assert_eq!(s[0].0, "off");
        assert_eq!(s[1].3["action"], "cfg_model");
        assert_eq!(s[1].0, "opus-4-6");
        assert_eq!(s[2].3["action"], "cfg_ctx");
        assert_eq!(s[2].0, "400k (50%)");
        assert!(
            s.iter().all(|sel| sel.3["th"] == true
                && sel.3["scope"] == "om_root"
                && sel.3["chat"] == "oc_1"),
            "every callback value carries th:true and both scope keys, {card}"
        );
        assert!(!card.contains("cfg_watch"), "{card}");
        assert!(!card.contains("cfg_threads"), "{card}");
        assert!(!card.contains("👁 Watching"), "{card}");

        let v: serde_json::Value = serde_json::from_str(&card).unwrap();
        let mut buttons = Vec::new();
        buttons_of(&v, &mut buttons);
        assert_eq!(buttons.len(), 2, "{card}");
        assert!(
            buttons.iter().all(|b| b.1["th"] == true
                && b.1["scope"] == "om_root"
                && b.1["container"] == "omt_9"
                && b.1["chat"] == "oc_1"),
            "footer buttons carry th:true and all scope keys, {card}"
        );
    }

    #[test]
    fn scope_from_value_falls_back_conservatively() {
        // 旧卡（扁平 scope+dm）：container/chat 回落 session，th 回落
        // chat——语义与渲染它的旧代码一致。
        let old = json!({"action": "cfg_model", "scope": "oc_1", "dm": false, "option": "k3-hs"});
        let scope = Scope::from_value(&old, Some("oc_1")).unwrap();
        assert_eq!(scope, Scope::chat_scope("oc_1", false));

        // dm 缺失按私聊（保守）；事件 chat_id 是 chat 的第二来源。
        let old = json!({"action": "cfg_refresh", "scope": "oc_1"});
        let scope = Scope::from_value(&old, Some("oc_9")).unwrap();
        assert_eq!(scope.chat, "oc_9");
        assert!(scope.dm);
        assert!(!scope.thread);

        // 新 thread 卡：三键各自就位。
        let new = json!({
            "action": "cfg_ctx", "scope": "om_root", "container": "omt_9",
            "chat": "oc_1", "dm": false, "th": true, "option": "200k (25%)"
        });
        let scope = Scope::from_value(&new, Some("oc_1")).unwrap();
        assert_eq!(scope, thread_scope());

        // scope 缺失/为空 → 拒绝。
        assert!(Scope::from_value(&json!({"action": "cfg_refresh"}), None).is_none());
        assert!(Scope::from_value(&json!({"scope": ""}), None).is_none());
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
        let card = settings_card(&chat_scope(), &state(None, None, None));
        let s = selects_of(&card);
        assert_eq!(s[0].0, "default (on)");
        assert_eq!(s[1].0, "default (off)");
        // No override: the default pseudo-option is the current value.
        assert_eq!(s[2].0, "default (k3-hs)");
        assert_eq!(s[2].2, "default (k3-hs)");
    }

    #[test]
    fn footer_buttons_carry_scope_and_small_size() {
        let card = settings_card(&chat_scope(), &state(None, None, None));
        let v: serde_json::Value = serde_json::from_str(&card).unwrap();
        let mut buttons = Vec::new();
        buttons_of(&v, &mut buttons);
        assert_eq!(buttons.len(), 2, "{card}");
        assert_eq!(buttons[0].0, "♻️ Reset all");
        assert_eq!(
            buttons[0].1,
            serde_json::json!({"action": "cfg_reset_all", "scope": "oc_1", "container": "oc_1", "chat": "oc_1", "dm": false, "th": false})
        );
        assert_eq!(buttons[1].0, "🔄 Refresh");
        assert_eq!(
            buttons[1].1,
            serde_json::json!({"action": "cfg_refresh", "scope": "oc_1", "container": "oc_1", "chat": "oc_1", "dm": false, "th": false})
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

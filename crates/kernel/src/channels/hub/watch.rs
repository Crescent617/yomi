//! Watch mode (`/watch`): the chat's own session as sole-listener
//! observer — one session per chat, the mapping's `kind` is the mode.
//!
//! `/watch on` flips the chat mapping's `kind` to `watch` (creating the
//! session if absent): from then on every plain message passing access
//! control — mention or not — is *mirrored*: steered verbatim into the
//! chat's session, while the gate suspends the conversation-trigger
//! path (see `gate.rs`). The session is the chat's only message
//! consumer and decides for itself when a reply is warranted. While
//! `kind='watch'` the channel delivers NOTHING for it: no status card,
//! no streaming consumption, no reply delivery, no reactions, no
//! subscriber notify (suppressed at the event-forwarder single point).
//! Its only voice is the platform skill from its own skill list (e.g.
//! `lark` for feishu) — with no matching skill it is a pure read-only
//! observer. `/watch off` flips the kind back to `normal`: the SAME
//! session answers mentions again, its watch-period memory intact. The
//! watch contract lives in the system prompt
//! ([`crate::prompt::watch_section`], appended by the conductor at
//! spawn while kind is `watch`), so it survives context compaction.
//!
//! Mirror bounds (observer is a long-lived session; bounded by design,
//! not by hope — both confined to this module, other modes untouched):
//! - **Per-block truncation** ([`MIRROR_TEXT_CAP`]): an oversized text
//!   block is cut with an omission marker — a giant paste must not eat
//!   the observer's context; the full text stays one `msg_id` away.
//! - **Batching** ([`BATCH_WINDOW`] / [`BATCH_CAP`]): mirrors collect
//!   into a per-chat pending batch flushed once per window (or early
//!   at the cap) as a single steer. Mid-run arrivals already merged in
//!   the mailbox; batching extends the same idea to the idle→run wake,
//!   so a chatty burst becomes one turn — the observer judges
//!   "worth interrupting" per turn, and deserves the conversational
//!   beat rather than fragments. Any real kind flip drains the pending
//!   batch (companion of the flip's mailbox drain), so tee-time never
//!   smears across a flip: a batch mirrors only when the kind stayed
//!   `Watch` from gate to flush.

use std::sync::Arc;

use dashmap::DashMap;
use tracing::{info, warn};

use crate::kernel::Kernel;
use crate::types::ContentBlock;

use crate::channels::{ChannelMessage, ChannelStore, MappingKind};

use crate::channels::hub_routing::get_or_create_session_locked;

/// 单条镜像文本块的截断阈值（字符数）：巨型粘贴不该吃掉观察者的上下
/// 文——观察者要的是梗概，需要全文可凭头里的 `msg_id` 经 skill 自取。
const MIRROR_TEXT_CAP: usize = 2048;

/// 攒批窗口：首条消息到达后的等待时长。watch 是被动观察者，这点延迟
/// 无关痛痒（对话路径不经此模块，零影响）。
const BATCH_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

/// pending 批上限：真洪峰时提前 flush（观察者更该早看），防超长窗口
/// 攒出巨型批。
pub(crate) const BATCH_CAP: usize = 50;

/// Per-chat pending mirror batch.
#[derive(Default)]
struct PendingBatch {
    /// Batched messages (each already `mirror_content`-assembled), in
    /// arrival order (the receive loop is sequential).
    items: Vec<Vec<ContentBlock>>,
    /// The sleeping window task, if any. The task self-clears this
    /// handle at take time — a task running its flush is NOT a future
    /// flusher, so an enqueue during the flush schedules a fresh task
    /// (a message can never be stranded behind a running flush). A
    /// finished handle (completed normally, or its runtime died — test
    /// worlds) self-heals the same way: treated as no task.
    flush_task: Option<tokio::task::JoinHandle<()>>,
    /// Flip-drain counter: a batch taken before a drain carries a stale
    /// epoch and is dropped at flush (off→on double-flip race — see
    /// [`flush_batch`]).
    epoch: u64,
}

/// 按 chat 攒批的镜像队列，键 = (kernel, channel, chat)。测试在同一
/// 进程里并存多个独立 kernel 世界，键必须含 kernel 身份，否则别处的
/// watch 翻转 drain 会跨界清空本世界的 pending（生产单 kernel 无此
/// 问题，行为与按 channel:chat 完全一致）。条目常驻（空 Vec + 句柄 +
/// 计数，按 watch chat 数有界）；flush 任务持有 `Arc<Kernel>`，存活
/// 任务钉住自己的世界；任务句柄在入队时经 `is_finished` 自愈，地址
/// 复用残留的完成态句柄不会挡住新世界的首次 flush。
static PENDING: std::sync::LazyLock<DashMap<String, Arc<tokio::sync::Mutex<PendingBatch>>>> =
    std::sync::LazyLock::new(DashMap::new);

/// pending 队列键：kernel 指针身份 + channel + chat（见 [`PENDING`]）。
fn pending_key(kernel: &Kernel, channel_name: &str, chat_id: &str) -> String {
    format!(
        "{:x}:{channel_name}:{chat_id}",
        std::ptr::from_ref(kernel) as usize
    )
}

/// Mirror one message into the watched chat's session: assemble the
/// content, batch it per chat ([`PENDING`]), and let the window flush
/// steer it in. Fire-and-forget semantics for the caller — a run starts
/// when the session is idle, a mid-run mailbox post when it is already
/// thinking. Images are NOT downloaded (unlike post-gate triggers): the
/// session pulls them via skill only if it cares. Failures are logged,
/// never propagated — the tee must not break the serial dispatch of the
/// conversation path it shadows.
///
/// The tee fires on the gate-time snapshot; the flush (see
/// [`flush_batch`]) then re-reads the live row and steers under a
/// single route lock — the same lock the kind flip holds across its
/// read-flip-reset (see [`set_channel_watch_by_name`]), so an off/gc
/// can never interleave (which would make a back-to-`normal` session
/// answer publicly). A missing row means watch is off — drop, never
/// resurrect. A dangling row (alive, session gone) is healed by the
/// locked get-or-create in the same critical section. (Residual
/// micro-window, accepted: delete_session/gc takes no route lock, so a
/// row+session delete can still land between the re-read and the
/// locked create — the create then resurrects a just-ended watch row.
/// Same window existed pre-refactor; locking delete_session isn't
/// worth it.)
pub(crate) async fn mirror_message(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    msg: &ChannelMessage,
) {
    mirror_enqueue(channel_name, store, kernel, msg, BATCH_WINDOW).await;
}

/// Drain a chat's pending mirror batch — the kind-flip companion of the
/// mailbox drain in [`set_channel_watch_by_name`]: a real flip is a
/// hard boundary, pending I/O from the previous mode must not leak into
/// the new one. Clearing items AND bumping the epoch through the shared
/// entry (never removing it) lets the sleeping window task wake to an
/// empty batch and exit cleanly, and marks any already-taken batch
/// stale (see [`flush_batch`]); producers after the drain ride the same
/// task's wake.
pub(crate) async fn drain_pending(kernel: &Kernel, channel_name: &str, chat_id: &str) {
    // Clone the Arc out before awaiting — never hold a DashMap shard
    // guard across `.await`.
    let entry = PENDING
        .get(&pending_key(kernel, channel_name, chat_id))
        .map(|r| Arc::clone(r.value()));
    if let Some(entry) = entry {
        let mut state = entry.lock().await;
        state.items.clear();
        state.epoch += 1;
    }
}

/// Enqueue one message into the chat's pending batch and schedule the
/// window flush — or flush inline when the batch cap trips (a real
/// flood means the observer should see it sooner, and the cap bounds
/// the batch). `window` is a test seam; the production entry is
/// [`mirror_message`] (fixed [`BATCH_WINDOW`]).
pub(crate) async fn mirror_enqueue(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    msg: &ChannelMessage,
    window: std::time::Duration,
) {
    let chat_id = msg.external_chat_id.clone();
    let key = pending_key(kernel, channel_name, &chat_id);
    let entry = PENDING.entry(key).or_default().clone();
    // 截断+克隆在锁外完成（两遍 O(n) 字符扫描，别占着 entry 锁——
    // 翻转 drain 就在等它）。
    let content = mirror_content(msg);
    let mut state = entry.lock().await;
    state.items.push(content);
    let task_alive = state.flush_task.as_ref().is_some_and(|h| !h.is_finished());
    if task_alive {
        if state.items.len() < BATCH_CAP {
            return; // 窗口任务已在睡，攒着。
        }
        // 洪峰：提前 flush（睡着的任务随后醒来看到空队列直接退出）。
        let batch = std::mem::take(&mut state.items);
        let epoch = state.epoch;
        let task_entry = Arc::clone(&entry);
        drop(state);
        flush_batch(
            channel_name,
            store,
            kernel,
            &chat_id,
            batch,
            epoch,
            task_entry,
        )
        .await;
        return;
    }
    let kernel = Arc::clone(kernel);
    let store = Arc::clone(store);
    let channel_name = channel_name.to_string();
    let task_entry = Arc::clone(&entry);
    state.flush_task = Some(tokio::spawn(async move {
        tokio::time::sleep(window).await;
        let (batch, epoch) = {
            let mut state = task_entry.lock().await;
            let batch = std::mem::take(&mut state.items);
            // 取出即自清句柄：本任务接下来的 flush 不再为未来消息负
            // 责——flush 期间到达的消息必须另起新任务，否则会搁浅到下
            // 一条消息（闲聊收尾的那一条就永远丢了）。
            state.flush_task = None;
            (batch, state.epoch)
        };
        if batch.is_empty() {
            return; // 被 drain 或提前 flush 抢空。
        }
        flush_batch(
            &channel_name,
            &store,
            &kernel,
            &chat_id,
            batch,
            epoch,
            task_entry,
        )
        .await;
    }));
    drop(state);
}

/// Flush one pending batch through the tee's unchanged semantics: the
/// route lock is held across the live kind re-read and the locked
/// get-or-create — mutually exclusive with kind flips (see
/// [`set_channel_watch_by_name`]). A chat no longer watched at flush
/// time drops the batch (the off already drained its mailbox); a
/// missing row is never resurrected; a dangling row is healed by the
/// same locked get-or-create as before. One lock, one steer per batch.
async fn flush_batch(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    chat_id: &str,
    batch: Vec<Vec<ContentBlock>>,
    epoch: u64,
    entry: Arc<tokio::sync::Mutex<PendingBatch>>,
) {
    let _guard =
        crate::utils::g_lock::g_lock(format!("channel_route:{channel_name}:{chat_id}")).await;
    // epoch 守卫：批被 take 之后若发生翻转 drain（epoch 前进——off→on
    // 双翻转竞态），它是旧模式的遗物，整批丢弃。drain 与 flush 都经
    // 路由锁串行，且锁序同为 route→entry（与 [`drain_pending`] 一
    // 致），检查是精确的。
    let current_epoch = entry.lock().await.epoch;
    if current_epoch != epoch {
        info!(channel = %channel_name, chat_id = %chat_id, "stale mirror batch dropped (watch flipped mid-flight)");
        return;
    }
    let watched = match store.find_mapping_kind(channel_name, chat_id).await {
        Ok(row) => matches!(row, Some((_, MappingKind::Watch))),
        Err(e) => {
            warn!(channel = %channel_name, chat_id = %chat_id, error = %e, "watch mapping lookup failed; mirror batch dropped");
            return;
        }
    };
    if !watched {
        info!(channel = %channel_name, chat_id = %chat_id, "not watched at flush; mirror batch dropped");
        return;
    }
    match get_or_create_session_locked(
        channel_name,
        store,
        kernel,
        chat_id,
        chat_id,
        None,
        MappingKind::Watch,
    )
    .await
    {
        Ok((sid, _reused)) => {
            let count = batch.len();
            // 每条消息的头已在各自首个 text block 里，扁平化即保序保
            // 归属；空批（理论防御）不 steer。
            let content: Vec<ContentBlock> = batch.into_iter().flatten().collect();
            if content.is_empty() {
                return;
            }
            kernel.send_steer(&sid, content).await;
            info!(channel = %channel_name, chat_id = %chat_id, session_id = %sid.0, count, "mirrored batch to watch session");
        }
        Err(e) => {
            warn!(channel = %channel_name, chat_id = %chat_id, error = %e, "watch session resolution failed");
        }
    }
}

/// The mirrored content: the message's own blocks verbatim **modulo
/// the per-block cap** (see [`MIRROR_TEXT_CAP`]) — the adapter header
/// already carries `[ts][from][chat][msg_id][thread][root]` — plus
/// image references as text: opaque platform keys the session can
/// resolve on demand via its skill.
fn mirror_content(msg: &ChannelMessage) -> Vec<ContentBlock> {
    let mut content = msg.content.clone();
    for block in &mut content {
        if let ContentBlock::Text { text } = block {
            truncate_chars(text, MIRROR_TEXT_CAP);
        }
    }
    if !msg.image_keys.is_empty() {
        let refs = msg
            .image_keys
            .iter()
            .map(|key| format!("[image: {key}]"))
            .collect::<Vec<_>>()
            .join(" ");
        content.push(ContentBlock::Text { text: refs });
    }
    content
}

/// Truncate `text` to at most `cap` chars, appending an omission marker
/// when anything was cut. Char-boundary safe (multibyte never split).
fn truncate_chars(text: &mut String, cap: usize) {
    if text.chars().count() <= cap {
        return;
    }
    let kept: String = text.chars().take(cap).collect();
    text.clear();
    text.push_str(&kept);
    text.push_str("…(已截断)");
}

#[cfg(test)]
#[path = "watch_test.rs"]
mod tests;

// ── Query / switch (shared by the `/watch` command and the RPC) ─────────

/// The chat-visible ack posted after a watch flip — used by the
/// `/watch` command only. The flip decides whether the bot speaks in
/// this chat at all, so the command path leaves a visible trace. (The
/// settings card refreshes in place instead: cards never message, and
/// its watch-on notation line carries the explanation.)
pub(crate) fn flip_ack_text(on: bool) -> String {
    if on {
        "👁 Watch on — every non-command message here goes to this chat's session as its observer. \
         It decides for itself when to speak (via skill) or stay silent; \
         @-mentions no longer trigger conversation replies while watch is on. \
         In groups commands always need an @: `@bot /watch off` to stop."
            .to_string()
    } else {
        "⏹ Watch off — the same session answers @-mentions here again, \
         its watch-period memory intact. `@bot /watch on` to resume watching."
            .to_string()
    }
}

/// Query a chat's watch mode by channel name.
pub(crate) async fn get_channel_watch_by_name(
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    chat_id: &str,
) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
    let row = store.find_mapping_kind(channel_name, chat_id).await?;
    Ok(crate::channels::ChannelWatchStatus {
        on: matches!(row, Some((_, MappingKind::Watch))),
        session_id: row.map(|(sid, _)| sid.0.to_string()),
    })
}

/// Switch a chat's watch mode by channel name. Same core as `/watch
/// on|off`: on ensures the chat session exists and flips its kind to
/// `Watch`; off flips back to `Normal`. Both directions hold the route
/// lock across read-flip-reset — mutually exclusive with the tee's
/// re-read+steer (see [`flush_batch`]).
///
/// A flip cancels the in-flight run and drains the mailbox **and the
/// pending mirror batch** ([`drain_pending`]): pending I/O from the
/// previous mode must not leak into the new one (a queued conversation
/// request must not be answered invisibly while watched, nor a mirrored
/// message wake the session after off). No state change (idempotent on,
/// or off while not watched) is a pure no-op — in particular off must
/// never kill an ordinary session's run.
pub(crate) async fn set_channel_watch_by_name(
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    channel_name: &str,
    chat_id: &str,
    on: bool,
) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
    let lock_key = format!("channel_route:{channel_name}:{chat_id}");
    if on {
        let _guard = crate::utils::g_lock::g_lock(lock_key).await;
        // Idempotent: already watched (and alive) — nothing to do. The
        // liveness check is not redundant: without it an idempotent on
        // over a dangling row would no-op and never heal.
        if let Some((sid, MappingKind::Watch)) =
            store.find_mapping_kind(channel_name, chat_id).await?
        {
            if kernel.session_store().await.get(&sid).await?.is_some() {
                return Ok(crate::channels::ChannelWatchStatus {
                    on: true,
                    session_id: Some(sid.0.to_string()),
                });
            }
        }
        // Ensure a live row, then flip explicitly — get_or_create only
        // writes `kind` on create, so the flip is needed exactly when an
        // existing row was reused.
        let (sid, reused) = get_or_create_session_locked(
            channel_name,
            store,
            kernel,
            chat_id,
            chat_id,
            None,
            MappingKind::Watch,
        )
        .await?;
        if reused {
            store
                .update_mapping(channel_name, chat_id, None, Some(MappingKind::Watch))
                .await?;
        }
        kernel.cancel(&sid);
        kernel
            .clear_mailbox(&sid, crate::comms::MailboxScope::All)
            .await;
        drain_pending(kernel, channel_name, chat_id).await;
        Ok(crate::channels::ChannelWatchStatus {
            on: true,
            session_id: Some(sid.0.to_string()),
        })
    } else {
        let _guard = crate::utils::g_lock::g_lock(lock_key).await;
        let Some((sid, MappingKind::Watch)) =
            store.find_mapping_kind(channel_name, chat_id).await?
        else {
            return Ok(crate::channels::ChannelWatchStatus {
                on: false,
                session_id: None,
            });
        };
        store
            .update_mapping(channel_name, chat_id, None, Some(MappingKind::Normal))
            .await?;
        kernel.cancel(&sid);
        kernel
            .clear_mailbox(&sid, crate::comms::MailboxScope::All)
            .await;
        drain_pending(kernel, channel_name, chat_id).await;
        Ok(crate::channels::ChannelWatchStatus {
            on: false,
            session_id: Some(sid.0.to_string()),
        })
    }
}

impl crate::channels::hub::ChannelHub {
    /// Query a chat's watch mode (channel resolved by name or platform).
    pub async fn get_channel_watch(
        &self,
        channel: Option<&str>,
        platform: &str,
        chat_id: &str,
    ) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
        let (name, ..) = self.resolve_channel(channel, platform)?;
        get_channel_watch_by_name(&self.store(), &name, chat_id).await
    }

    /// Switch a chat's watch mode (channel resolved by name or platform).
    pub async fn set_channel_watch(
        &self,
        kernel: &Kernel,
        channel: Option<&str>,
        platform: &str,
        chat_id: &str,
        on: bool,
    ) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
        let (name, ..) = self.resolve_channel(channel, platform)?;
        set_channel_watch_by_name(&self.store(), kernel, &name, chat_id, on).await
    }

    /// The `set_channel_watch` RPC: `on` absent = query (Vim `:set` style).
    pub async fn rpc_set_channel_watch(
        &self,
        kernel: &Kernel,
        channel: Option<&str>,
        platform: &str,
        chat_id: &str,
        on: Option<bool>,
    ) -> crate::types::Result<crate::channels::ChannelWatchStatus> {
        match on {
            Some(on) => {
                self.set_channel_watch(kernel, channel, platform, chat_id, on)
                    .await
            }
            None => self.get_channel_watch(channel, platform, chat_id).await,
        }
    }
}

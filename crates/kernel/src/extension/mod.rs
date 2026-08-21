//! extension/ — wire 外部扩展（一期：custom tool 注册 + source 路由）。
//!
//! 一个扩展 = 一条 wire 连接 + 一本副作用账本（本模块的 DashMap 表）。
//! 注册即记账；连接断开 → `sweep` 逆序回收（tool 摘出、pending 全部
//! 报错）——teardown 只有断开一条路（RAII），状态只存内存，daemon
//! 重启后扩展重连重注册。设计文档：docs/design/extension-phase1.md。

pub mod supervisor;

#[cfg(test)]
#[path = "extension_test.rs"]
mod tests;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::{oneshot, Notify};
use tracing::{debug, info};

use crate::types::SessionId;

/// wire 连接标识（accept 时分配的 ULID）。
pub(crate) type ConnId = str;

/// ext_pull 的空转心跳（无单时的挂起上限）。
const PULL_TIMEOUT: Duration = Duration::from_secs(55);
/// 工作项从派单到结果的最长等待（超时对调用方报错）。
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// 外部工具登记定义（desc/schema 原样进模型工具表）。
#[derive(Debug, Clone)]
pub struct ExtToolDef {
    pub name: String,
    pub desc: String,
    pub schema: Value,
    pub level: crate::permission::Level,
}

/// ext_pull 领到的工作项（result_tx 留在 in-flight 表，不随单走）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PulledWork {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}

/// 工作项结果（provider 回执）。
#[derive(Debug)]
pub struct ExtCallOutcome {
    pub output: String,
    pub is_error: bool,
}

struct Registration {
    conn_id: String,
    def: ExtToolDef,
    queue: Mutex<VecDeque<PulledWork>>,
    notify: Notify,
    /// 单 worker 约束：同一时刻只允许一条挂起 pull。
    pull_pending: AtomicBool,
}

/// in-flight 工作项的 result 通道（call_id → 回执口 + 归属连接）。
struct InFlight {
    conn_id: String,
    result_tx: oneshot::Sender<ExtCallOutcome>,
}

#[derive(Default)]
pub struct ExtensionRegistry {
    regs: DashMap<String, Arc<Registration>>,
    by_name: DashMap<String, String>, // tool name → registration id
    inflight: DashMap<String, InFlight>,
    /// source 路由的内存回退（无 channel store 时）：(source, key) → session。
    routes: DashMap<(String, String), SessionId>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// `ext_register`（kind=tool）：ext 命名空间内查重；与内建工具撞名
    /// 在 spawn 合并时让位（conductor 记 warn 跳过）。
    /// 命名约束取各 provider 的最紧交集（OpenAI 函数名只允许
    /// `[a-zA-Z0-9_-]` 且字母开头）——点分命名（stock.quote）过不了 OpenAI。
    pub fn register_tool(&self, conn_id: &ConnId, def: ExtToolDef) -> Result<String, String> {
        let valid = !def.name.is_empty()
            && def
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && def
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !valid {
            return Err(format!(
                "invalid tool name '{}': must start with a letter and contain only \
                 letters, numbers, underscores and dashes (provider constraint)",
                def.name
            ));
        }
        // 原子查插：check-then-insert 的 TOCTOU 会让两个连接同时注册
        // 同名成功。entry 占坑即所有权重音。
        let id = format!("ext_{}", ulid::Ulid::new().to_string().to_lowercase());
        let dashmap::mapref::entry::Entry::Vacant(slot) = self.by_name.entry(def.name.clone())
        else {
            return Err(format!("extension tool '{}' already registered", def.name));
        };
        let reg = Arc::new(Registration {
            conn_id: conn_id.to_string(),
            def,
            queue: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            pull_pending: AtomicBool::new(false),
        });
        slot.insert(id.clone());
        self.regs.insert(id.clone(), reg);
        info!(registration = %id, tool = %self.regs.get(&id).unwrap().def.name, "extension tool registered");
        Ok(id)
    }

    /// 代理 Tool 的调用入口：派单并等待回执（60s 超时）。
    pub async fn dispatch(
        &self,
        registration: &str,
        args: Value,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExtCallOutcome, String> {
        let Some(reg) = self.regs.get(registration).map(|r| Arc::clone(&r)) else {
            return Err("extension tool provider disconnected".to_string());
        };
        let call_id = format!("c_{}", ulid::Ulid::new().to_string().to_lowercase());
        let (tx, rx) = oneshot::channel();
        self.inflight.insert(
            call_id.clone(),
            InFlight {
                conn_id: reg.conn_id.clone(),
                result_tx: tx,
            },
        );
        reg.queue.lock().unwrap().push_back(PulledWork {
            call_id: call_id.clone(),
            name: reg.def.name.clone(),
            args,
        });
        reg.notify.notify_one();

        // 超时 / 取消 / 回执三选一；无论哪条路径都清掉 in-flight，
        // 迟到的 result 找不到登记即被丢弃（记 debug）。
        let wait = async {
            match rx.await {
                Ok(outcome) => Ok(outcome),
                Err(_) => Err("extension tool provider disconnected".to_string()),
            }
        };
        let result = match cancel {
            Some(ct) => {
                tokio::select! {
                    r = wait => r,
                    () = ct.cancelled() => Err("cancelled".to_string()),
                    () = tokio::time::sleep(CALL_TIMEOUT) =>
                        Err(format!("extension tool '{}' timed out (60s)", reg.def.name)),
                }
            }
            None => match tokio::time::timeout(CALL_TIMEOUT, wait).await {
                Ok(r) => r,
                Err(_) => Err(format!("extension tool '{}' timed out (60s)", reg.def.name)),
            },
        };
        self.inflight.remove(&call_id);
        result
    }

    /// `ext_pull`：单 worker 长轮询（55s 空转心跳）。
    pub async fn pull(
        &self,
        conn_id: &ConnId,
        registration: &str,
    ) -> Result<Option<PulledWork>, String> {
        let Some(reg) = self.regs.get(registration).map(|r| Arc::clone(&r)) else {
            return Err(format!("unknown registration '{registration}'"));
        };
        if reg.conn_id != conn_id {
            return Err("registration belongs to another connection".to_string());
        }
        if reg.pull_pending.swap(true, Ordering::SeqCst) {
            return Err("a pull is already pending on this registration".to_string());
        }
        let _reset = ResetOnDrop(&reg.pull_pending);

        // Notify 的 permit 语义覆盖了"查队列→挂起"之间的竞争：
        // 先查一次，没单就挂起等 notify（期间来单会被 permit 立即唤醒）。
        if let Some(item) = reg.queue.lock().unwrap().pop_front() {
            return Ok(Some(item));
        }
        match tokio::time::timeout(PULL_TIMEOUT, reg.notify.notified()).await {
            Ok(()) => Ok(reg.queue.lock().unwrap().pop_front()),
            Err(_) => Ok(None),
        }
    }

    /// `ext_result`：交付回执。归属校验（call_id 属于本连接）；
    /// 过期/取消后迟到的 result 找不到登记，丢弃记 debug（独立失败域）。
    pub fn submit_result(
        &self,
        conn_id: &ConnId,
        call_id: &str,
        output: String,
        is_error: bool,
    ) -> Result<(), String> {
        let Some((_, inflight)) = self.inflight.remove(call_id) else {
            debug!(call_id, "ext_result for unknown/expired call, discarded");
            return Ok(());
        };
        if inflight.conn_id != conn_id {
            // 放回不了（所有权已取出）——直接报错并唤醒调用方，防串线。
            let _ = inflight.result_tx.send(ExtCallOutcome {
                output: "ext_result from wrong connection".to_string(),
                is_error: true,
            });
            return Err("call_id belongs to another connection".to_string());
        }
        let _ = inflight.result_tx.send(ExtCallOutcome { output, is_error });
        Ok(())
    }

    /// 连接断开的 RAII 回收：该连接的全部 registration 摘除（tool 立
    /// 即从后续 spawn 的工具表消失），其 in-flight 与队列中的工作项
    /// 全部以 disconnected 报错。
    pub fn sweep(&self, conn_id: &ConnId) {
        let reg_ids: Vec<String> = self
            .regs
            .iter()
            .filter(|e| e.value().conn_id == conn_id)
            .map(|e| e.key().clone())
            .collect();
        for id in &reg_ids {
            if let Some((_, reg)) = self.regs.remove(id) {
                self.by_name.remove(&reg.def.name);
                info!(registration = %id, tool = %reg.def.name, "extension tool swept");
                // 唤醒挂起的 pull（连接已死，响应写不出，只是让 future 收尾）。
                reg.notify.notify_one();
            }
        }
        let dead_calls: Vec<String> = self
            .inflight
            .iter()
            .filter(|e| e.value().conn_id == conn_id)
            .map(|e| e.key().clone())
            .collect();
        for call_id in dead_calls {
            if let Some((_, inflight)) = self.inflight.remove(&call_id) {
                let _ = inflight.result_tx.send(ExtCallOutcome {
                    output: "extension tool provider disconnected".to_string(),
                    is_error: true,
                });
            }
        }
    }

    /// conductor spawn 用：当前全部 ext 工具的代理（撞名与 blocklist
    /// 检查在 Agent::new 合并时做——注册表才是唯一真相源）。
    pub fn tool_proxies(self: &Arc<Self>) -> Vec<ExtTool> {
        self.regs
            .iter()
            .map(|e| {
                let reg = Arc::clone(e.value());
                ExtTool {
                    registry: Arc::clone(self),
                    registration: e.key().clone(),
                    def: reg.def.clone(),
                }
            })
            .collect()
    }

    /// source 路由（内存回退路径）。
    pub fn route_get(&self, source: &str, key: &str) -> Option<SessionId> {
        self.routes
            .get(&(source.to_string(), key.to_string()))
            .map(|r| r.clone())
    }

    pub fn route_set(&self, source: &str, key: &str, sid: SessionId) {
        self.routes
            .insert((source.to_string(), key.to_string()), sid);
    }
}

/// pull_pending 的作用域复位（pull 返回/报错/超时都解开）。
struct ResetOnDrop<'a>(&'a AtomicBool);
impl Drop for ResetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 注册进 session ToolRegistry 的代理 Tool：desc/schema 用登记的，
/// exec 派给登记连接的队列。
pub struct ExtTool {
    registry: Arc<ExtensionRegistry>,
    registration: String,
    def: ExtToolDef,
}

#[async_trait::async_trait]
impl crate::tools::Tool for ExtTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn desc(&self) -> &str {
        &self.def.desc
    }

    fn schema(&self) -> Value {
        self.def.schema.clone()
    }

    async fn exec(
        &self,
        args: Value,
        ctx: crate::tools::ToolExecCtx<'_>,
    ) -> crate::types::Result<crate::types::ToolOutput> {
        let outcome = self
            .registry
            .dispatch(&self.registration, args, ctx.cancel_token.as_ref())
            .await;
        match outcome {
            Ok(o) if o.is_error => Ok(crate::types::ToolOutput::error(o.output)),
            Ok(o) => Ok(crate::types::ToolOutput::text(o.output)),
            Err(e) => Ok(crate::types::ToolOutput::error(e)),
        }
    }
}

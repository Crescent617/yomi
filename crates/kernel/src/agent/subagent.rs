use crate::agent::{Agent, AgentHandle, AgentShared, SubAgentMode};
use crate::types::AgentId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 子代理管理器
#[derive(Clone)]
pub struct SubAgentManager {
    sub_agents: Arc<RwLock<HashMap<AgentId, SubAgentHandle>>>,
    parent_id: AgentId,
    agent_shared: Arc<AgentShared>,
}

impl std::fmt::Debug for SubAgentManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAgentManager")
            .field("parent_id", &self.parent_id)
            .finish_non_exhaustive()
    }
}

struct SubAgentHandle {
    handle: AgentHandle,
    #[allow(dead_code)]
    mode: SubAgentMode,
}

impl SubAgentManager {
    pub fn new(parent_id: AgentId, agent_shared: Arc<AgentShared>) -> Self {
        Self {
            sub_agents: Arc::new(RwLock::new(HashMap::new())),
            parent_id,
            agent_shared,
        }
    }

    /// 启动子代理
    pub async fn spawn(&self, mode: SubAgentMode, task: String) -> AgentId {
        let system_prompt = format!(
            "You are a sub-agent working on a specific task. \
             Parent agent: {}. Task: {}",
            self.parent_id.0, task
        );

        let (handle, mut event_rx) = Agent::spawn(
            AgentId::new(),
            Arc::clone(&self.agent_shared),
            &system_prompt,
            None, // Sub-agents don't persist to storage
            None,
            10,    // Sub-agents get fewer iterations
            false, // Sub-agents don't spawn more sub-agents
            SubAgentMode::Async,
        );

        let id = handle.id.clone();

        // Spawn a task to drain events from subagent (prevents channel from filling)
        tokio::spawn(async move {
            while let Some(_event) = event_rx.recv().await {
                // Subagent events are currently discarded
                // Could forward to parent agent if needed
            }
        });

        // 发送子代理任务
        handle.send_text(task).await.ok();

        self.sub_agents
            .write()
            .await
            .insert(id.clone(), SubAgentHandle { handle, mode });

        id
    }

    /// 获取子代理句柄
    pub async fn get(&self, id: &AgentId) -> Option<AgentHandle> {
        self.sub_agents
            .read()
            .await
            .get(id)
            .map(|h| h.handle.clone())
    }

    /// 等待所有子代理完成
    pub async fn wait_for_all(&self) -> Vec<AgentId> {
        let ids: Vec<_> = self.sub_agents.read().await.keys().cloned().collect();

        for id in &ids {
            if let Some(handle) = self.get(id).await {
                // 简单轮询等待完成
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    let state = handle.state();
                    if state.is_terminal() {
                        break;
                    }
                }
            }
        }

        ids
    }

    /// 取消所有子代理
    pub async fn cancel_all(&self) {
        for (_, handle) in self.sub_agents.read().await.iter() {
            handle.handle.cancel();
        }
    }
}

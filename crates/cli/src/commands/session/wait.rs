//! `yomi session wait` —— 轮询会话直至完全静默：
//! `phase == idle`、无 running 子 agent、无后台 shell 任务。
//!
//! 退出码：0 静默；2 首个探测失败（会话不存在或 daemon 不在）；
//! 3 `--timeout` 到期。mailbox pending 不在静默判定内（队列可经
//! `yomi session mailbox` 查看，但不视为忙碌）。

use crate::args::GlobalArgs;
use anyhow::Result;
use kernel::client::KernelApi;
use kernel::types::SessionId;
use std::time::{Duration, Instant};

struct Probe {
    phase: String,
    running_subagents: usize,
    background_shells: usize,
}

impl Probe {
    fn quiescent(&self) -> bool {
        self.phase == "idle" && self.running_subagents == 0 && self.background_shells == 0
    }
}

async fn probe(kernel: &kernel::client::RemoteKernel, sid: &SessionId) -> Result<Probe> {
    let session = kernel.get_session(sid).await?;
    // 子查询失败按"忙"处理（宁等勿放），与历史 session-wait 脚本语义一致。
    let running_subagents = kernel
        .list_subagents(sid)
        .await
        .map_or(1, |subs| subs.iter().filter(|s| s.is_running).count());
    let background_shells = kernel.list_running_sessions().await.map_or(1, |sessions| {
        sessions
            .iter()
            .find(|s| s.id == *sid)
            .map_or(0, |s| s.background_shells.len())
    });
    Ok(Probe {
        phase: session.phase,
        running_subagents,
        background_shells,
    })
}

pub async fn run(
    global: &GlobalArgs,
    session: Option<String>,
    interval: u64,
    timeout: Option<u64>,
) -> Result<()> {
    // 会话解析失败（当前目录无记录）属用法错误，与脚本语义一致 exit 2。
    let session_id = match super::resolve_session_id(global, session).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("session-wait: {e}");
            std::process::exit(2);
        }
    };
    let sid = SessionId::from(session_id.clone());
    let kernel = match crate::daemon::connect_strict().await {
        Ok(k) => k,
        Err(e) => {
            eprintln!("session-wait: cannot reach daemon: {e}");
            std::process::exit(2);
        }
    };

    let start = Instant::now();
    let mut reachable_once = false;
    let mut last_line = String::new();

    loop {
        let mut state = match probe(&kernel, &sid).await {
            Ok(p) => {
                reachable_once = true;
                p
            }
            Err(e) if reachable_once => {
                // 曾经连通后的失败按瞬时处理（daemon 重启中？）——继续等。
                tracing::warn!("session-wait probe failed (treated as busy): {e}");
                Probe {
                    phase: "unreachable".to_string(),
                    running_subagents: 1,
                    background_shells: 1,
                }
            }
            Err(e) => {
                eprintln!(
                    "session-wait: cannot query session {session_id} (bad session id or daemon down): {e}"
                );
                std::process::exit(2);
            }
        };

        if state.quiescent() {
            // turn 边界可能读出一瞬 idle（agent 取下一条 mailbox 的间隙）：
            // 1s 后复检确认。
            tokio::time::sleep(Duration::from_secs(1)).await;
            match probe(&kernel, &sid).await {
                Ok(p) if p.quiescent() => {
                    println!(
                        "session-wait: {session_id} quiescent (idle, no running subagents, no shell tasks) after {}s",
                        start.elapsed().as_secs()
                    );
                    return Ok(());
                }
                Ok(p) => state = p, // 假静默：用复检值继续
                Err(e) => {
                    // 复检失败同主探测——按忙处理，不留旧的 idle 状态行。
                    tracing::warn!("session-wait re-probe failed (treated as busy): {e}");
                    state = Probe {
                        phase: "unreachable".to_string(),
                        running_subagents: 1,
                        background_shells: 1,
                    };
                }
            }
        }

        let line = format!(
            "phase={} subagents_running={} bg_shells={}",
            state.phase, state.running_subagents, state.background_shells
        );
        if let Some(t) = timeout {
            if start.elapsed().as_secs() >= t {
                eprintln!("session-wait: timeout after {t}s ({line})");
                std::process::exit(3);
            }
        }
        if line != last_line {
            println!("session-wait: [{}s] {line}", start.elapsed().as_secs());
            last_line = line;
        }

        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

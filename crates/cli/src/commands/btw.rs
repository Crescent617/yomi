//! `yomi btw` — ask an ephemeral side question against a session's live context.
//!
//! A single tool-free completion off a frozen snapshot of the session's
//! history (kernel `agent::btw`): the answer streams to stdout and leaves
//! no trace in the session. Works while the agent is mid-run.

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use kernel::client::KernelApi;
use kernel::event::{BtwEndReason, BtwEvent, Event};
use kernel::types::SessionId;
use std::io::{IsTerminal, Read as _, Write as _};

pub async fn run(
    global: &GlobalArgs,
    question: Vec<String>,
    session: Option<String>,
) -> Result<()> {
    let session_id = super::session::resolve_session_id(global, session).await?;
    let sid = SessionId::from(session_id.clone());
    let kernel = crate::daemon::connect_strict().await?;

    // 与 `session send` 同款前置校验：拼错的 session id 会被 daemon 当成
    // 空历史会话 spawn——fail fast。
    kernel
        .get_session(&sid)
        .await
        .with_context(|| format!("Session {session_id} not found"))?;

    let stdin = if question.is_empty() && !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Some(buf)
    } else {
        None
    };
    let text = if question.is_empty() {
        stdin.unwrap_or_default()
    } else {
        question.join(" ")
    };
    let text = text.trim();
    if text.is_empty() {
        anyhow::bail!("No question provided. Pass it as an argument or pipe it via stdin.");
    }

    // 先订阅再发问：Start → Delta* → Done 一条不漏。
    let mut subscriber = kernel
        .subscribe_session_events(&sid, None)
        .await
        .context("Failed to subscribe to session events")?;
    let request_id = kernel
        .btw(&sid, text.to_string(), None)
        .await
        .with_context(|| format!("Failed to start btw on session {session_id}"))?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut saw_text = false;
    // 与 `yomi events` 同款存活探针：daemon 失联时订阅可能只是静默。
    let mut watchdog = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => break,
            item = subscriber.recv() => {
                let Some((_sid, envelope)) = item else { break };
                let Event::Btw(event) = envelope.event else { continue };
                match event {
                    BtwEvent::Delta { request_id: rid, text } if rid == request_id => {
                        saw_text = true;
                        print!("{text}");
                        out.flush()?;
                    }
                    BtwEvent::Done { request_id: rid, reason } if rid == request_id => {
                        match reason {
                            // 模型不守规矩只发 tool_use 时，兜底文案由这里补
                            //（守规矩时它自己会在文本里说明，见下面的合并臂）。
                            BtwEndReason::ToolUse if !saw_text => {
                                println!("（这需要正式提问——旁问只能基于已有上下文回答）");
                            }
                            BtwEndReason::Stop | BtwEndReason::ToolUse => {}
                            BtwEndReason::Replaced => {
                                println!("\n（已被更新的旁问替换）");
                            }
                            BtwEndReason::Cancelled => {
                                println!("\n（已取消）");
                            }
                            BtwEndReason::Error(e) => {
                                anyhow::bail!("btw failed: {e}");
                            }
                        }
                        break;
                    }
                    _ => {}
                }
            }
            _ = watchdog.tick() => {
                if !kernel.is_connected().await {
                    out.flush()?;
                    anyhow::bail!("Lost connection to daemon");
                }
            }
        }
    }
    println!();
    out.flush()?;
    Ok(())
}

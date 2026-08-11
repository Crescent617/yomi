//! `yomi session send` — send a message to a session via the daemon.
//!
//! Requires a running daemon: the message goes over IPC and the conductor
//! spawns/restores the agent loop on demand. When the agent is busy the
//! message queues in its mailbox; `--steer` takes the priority lane and is
//! injected between turns instead of queueing behind normal messages.

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use kernel::client::KernelApi;
use kernel::types::{ContentBlock, SessionId};
use std::io::{IsTerminal, Read as _};

pub async fn run(
    global: &GlobalArgs,
    message: Vec<String>,
    session: Option<String>,
    steer: bool,
) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;
    let sid = SessionId::from(session_id.clone());

    let kernel = crate::daemon::connect_strict().await?;

    // send_message would happily spawn an agent for a typo'd session id
    // (empty history, fallback working dir) — fail fast instead.
    kernel
        .get_session(&sid)
        .await
        .with_context(|| format!("Session {session_id} not found"))?;

    // Resolve the message only after the session checks out, so a failure
    // above leaves piped stdin untouched for a retry.
    let stdin = if message.is_empty() && !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Some(buf)
    } else {
        None
    };
    let text = resolve_message(&message, stdin)?;

    let blocks = vec![ContentBlock::Text { text }];
    if steer {
        kernel
            .send_steer(&sid, blocks)
            .await
            .with_context(|| format!("Failed to steer session {session_id}"))?;
        println!("Steer sent to session {session_id}.");
    } else {
        kernel
            .send_message(&sid, blocks)
            .await
            .with_context(|| format!("Failed to send message to session {session_id}"))?;
        println!("Message sent to session {session_id}.");
    }
    Ok(())
}

/// Message text comes from positional args (joined with spaces) or, when
/// omitted, from piped stdin.
fn resolve_message(args: &[String], stdin: Option<String>) -> Result<String> {
    let text = if args.is_empty() {
        stdin.unwrap_or_default()
    } else {
        args.join(" ")
    };
    let text = text.trim();
    if text.is_empty() {
        anyhow::bail!("No message provided. Pass it as an argument or pipe it via stdin.");
    }
    Ok(text.to_string())
}

#[cfg(test)]
#[path = "send_test.rs"]
mod tests;

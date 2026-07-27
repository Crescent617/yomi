//! `yomi events` — stream session events from the daemon as NDJSON.
//!
//! One JSON envelope per line (`session_id` / `event_id` / `event`), so the
//! output pipes straight into `jq`. Single-session mode replays the daemon's
//! event buffer first (optionally resuming after `--after-event-id`), then
//! follows live; `--all` is real-time only across every session.

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use kernel::client::{KernelApi, RemoteKernel};
use kernel::types::{EventId, SessionId};
use std::io::Write as _;

pub async fn run(
    global: &GlobalArgs,
    session: Option<String>,
    all: bool,
    after_event_id: Option<String>,
) -> Result<()> {
    let addr = crate::daemon::socket_addr();
    let kernel = RemoteKernel::connect(&addr)
        .await
        .context("Failed to connect to daemon. Is it running?")?;

    let mut subscriber = if all {
        if after_event_id.is_some() {
            anyhow::bail!("--after-event-id only works with a single session, not --all");
        }
        kernel
            .subscribe_all_events()
            .await
            .context("Failed to subscribe to all sessions")?
    } else {
        let session_id = super::session::resolve_session_id(global, session).await?;
        kernel
            .subscribe_session_events(
                &SessionId::from(session_id),
                after_event_id.map(EventId::from),
            )
            .await
            .context("Failed to subscribe to session events")?
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    // A lost daemon also kills the server-side subscription, and the bridge
    // receiver may just go quiet — poll liveness so we don't hang forever.
    let mut watchdog = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => break,
            item = subscriber.recv() => {
                let Some((_sid, envelope)) = item else { break };
                writeln!(out, "{}", serde_json::to_string(&envelope)?)?;
            }
            _ = watchdog.tick() => {
                if !kernel.is_connected().await {
                    out.flush()?;
                    anyhow::bail!("Lost connection to daemon");
                }
            }
        }
    }
    out.flush()?;
    Ok(())
}

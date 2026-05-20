use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use kernel::client::{CoordinatorApi, RemoteCoordinator};
use kernel::types::SessionId;

pub async fn run(global: &GlobalArgs, session: Option<String>) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;

    let socket = crate::daemon::socket_path();
    let coordinator = RemoteCoordinator::connect(&socket)
        .await
        .context("Failed to connect to daemon. Is it running?")?;

    coordinator
        .shutdown_session(&SessionId(session_id.clone()))
        .await
        .with_context(|| format!("Failed to shutdown session {session_id}"))?;

    println!("Session {session_id} shutdown.");
    Ok(())
}

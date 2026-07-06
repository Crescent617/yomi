use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use kernel::client::{KernelApi, RemoteKernel};
use kernel::types::SessionId;

pub async fn run(global: &GlobalArgs, session: Option<String>) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;

    let addr = crate::daemon::socket_addr();
    let kernel = RemoteKernel::connect(&addr)
        .await
        .context("Failed to connect to daemon. Is it running?")?;

    let _ = kernel.cancel(&SessionId::from(session_id.clone())).await;

    println!("Session {session_id} cancelled.");
    Ok(())
}

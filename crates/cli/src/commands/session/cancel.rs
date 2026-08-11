use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use kernel::client::KernelApi;
use kernel::types::SessionId;

pub async fn run(global: &GlobalArgs, session: Option<String>) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;

    let kernel = crate::daemon::connect_strict().await?;

    kernel
        .cancel(&SessionId::from(session_id.clone()))
        .await
        .with_context(|| format!("Failed to cancel session {session_id}"))?;

    println!("Session {session_id} cancelled.");
    Ok(())
}

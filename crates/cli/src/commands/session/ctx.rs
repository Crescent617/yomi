//! `yomi session ctx` — 查询/设置/清除 session 的 context-window 覆盖
//! （settings 袋；设计见 docs/design/session-context-window.md）。

use crate::args::GlobalArgs;
use anyhow::{anyhow, Result};
use kernel::client::KernelApi;
use kernel::types::SessionId;

fn fmt_k(t: u32) -> String {
    if t % 1000 == 0 {
        format!("{}k", t / 1000)
    } else {
        format!("{:.1}k", t as f64 / 1000.0)
    }
}

pub async fn run(
    global: &GlobalArgs,
    session: Option<String>,
    value: Option<String>,
) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;
    let kernel = crate::daemon::connect_strict().await?;
    let sid = SessionId::from(session_id);

    match value.as_deref() {
        None => {}
        Some(v) if v.eq_ignore_ascii_case("reset") => {
            kernel.set_session_context_window(&sid, None).await?;
        }
        Some(v) => {
            let tokens = kernel::utils::env::parse_number_with_unit(v)
                .filter(|t| *t > 0)
                .ok_or_else(|| {
                    anyhow!("invalid context window `{v}` (try 512k, 1m, or `reset`)")
                })?;
            kernel
                .set_session_context_window(&sid, Some(tokens))
                .await?;
        }
    }

    let info = kernel.get_session_context_window(&sid).await?;
    let source = match info.override_ {
        Some(t) => format!("override {}", fmt_k(t)),
        None => format!("model default ({})", fmt_k(info.model_default)),
    };
    println!(
        "ctx: {} [{source}; model `{}`]",
        fmt_k(info.effective),
        info.model_key
    );
    Ok(())
}

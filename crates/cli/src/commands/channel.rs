//! `yomi channel` — drive platform channels from the CLI.

use crate::args::GlobalArgs;
use anyhow::Result;
use clap::{Parser, Subcommand};
use kernel::client::KernelApi;

#[derive(Parser)]
pub struct ChannelArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    command: ChannelCommands,
}

#[derive(Subcommand)]
enum ChannelCommands {
    /// Open a new thread in a chat and run a task in a fresh session there
    /// (a `/thread` trigger without a human message; threads are Feishu-only)
    NewThread(NewThreadArgs),
}

#[derive(Parser)]
struct NewThreadArgs {
    /// Chat to open the thread in (e.g. oc_...)
    #[arg(long)]
    chat: String,
    /// Task text to run in the new session
    #[arg(long)]
    text: String,
    /// Short title for the thread root (default: the task text); the task
    /// text is then posted as the thread's first reply
    #[arg(long)]
    title: Option<String>,
    /// Channel name from the config (default: the sole channel of --platform)
    #[arg(long)]
    channel: Option<String>,
    /// Platform used to resolve the channel when --channel is absent
    #[arg(long, default_value = "feishu")]
    platform: String,
}

pub async fn run(args: ChannelArgs) -> Result<()> {
    match args.command {
        ChannelCommands::NewThread(a) => {
            let client = crate::daemon::connect_strict().await?;
            let result = client
                .channel_new_thread(a.channel, Some(a.platform), a.chat, a.title, a.text)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
    }
}

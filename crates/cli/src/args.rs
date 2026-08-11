use clap::Parser;
use std::path::PathBuf;

/// Global arguments shared across all commands
#[derive(Parser, Default)]
pub struct GlobalArgs {
    /// Config file path
    #[arg(short, long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Working directory
    #[arg(short, long, global = true, value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

/// Kernel mode flags shared by `run` and `tui` (the only commands that
/// create a kernel). NOT global: other subcommands reject them loudly
/// instead of silently ignoring them.
#[derive(clap::Parser, Default)]
pub struct KernelModeArgs {
    /// Use the background daemon (spawning it when needed); the connection
    /// must pass the hello handshake, with no fallback
    #[arg(long, conflicts_with = "fg")]
    pub bg: bool,

    /// Foreground mode: force a local in-process kernel and ignore any
    /// running daemon (default: use a healthy daemon when one runs)
    #[arg(long)]
    pub fg: bool,
}

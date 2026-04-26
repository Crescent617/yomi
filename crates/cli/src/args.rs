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

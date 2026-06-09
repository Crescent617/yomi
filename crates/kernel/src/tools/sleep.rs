//! Sleep tool for synchronous waiting.
//!
//! This tool allows the agent to pause execution for a specified duration.
//! Unlike the reminder tool, this blocks synchronously and returns when the
//! delay has elapsed.

use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;

use crate::tools::{Tool, ToolExecCtx};
use crate::types::{Result, ToolOutput};

pub const SLEEP_TOOL_NAME: &str = "sleep";

/// Tool for synchronous sleeping / waiting.
#[derive(Default)]
pub struct SleepTool;

impl SleepTool {
    /// Create a new sleep tool.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SleepTool {
    fn name(&self) -> &'static str {
        SLEEP_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Pause execution for a specified number of seconds. Use when an external process needs time to settle or a rate-limit requires waiting. Do NOT use to wait for the result of a previous tool — those notify you automatically when finished."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                    "seconds": {
                        "type": "integer",
                        "description": "Number of seconds to sleep. Keep reasonable (1 - 3600).",
                        "minimum": 1,
                        "maximum": 3600
                    }
            },
            "required": ["seconds"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let delay = args["seconds"]
            .as_u64()
            .ok_or_else(|| crate::types::KernelError::tool("seconds must be a positive integer"))?;

        let start = tokio::time::Instant::now();
        tokio::select! {
            () = sleep(Duration::from_secs(delay)) => {
                Ok(ToolOutput::text(format!("Slept for {delay} seconds")))
            }
            () = ctx.cancelled() => {
                let elapsed = start.elapsed().as_secs();
                Ok(ToolOutput::text(format!(
                    "Sleep cancelled after {elapsed} seconds (planned {delay} seconds, not completed)"
                )))
            }
        }
    }
}

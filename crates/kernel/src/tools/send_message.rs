use crate::channels::hub::ChannelHub;
use crate::channels::utils::resolve_safe_path;
use crate::tools::{Tool, ToolExecCtx, ToolOutput};
use crate::types::{ContentBlock, Result, SessionId};
use serde_json::Value;
use std::sync::Arc;

pub const SEND_MESSAGE_TOOL_NAME: &str = "send_message";

/// Send a message to the current external chat platform.
///
/// Supports sending text content and/or attaching files.
/// At least one of `content` or `files` must be provided.
pub struct SendMessageTool {
    channel_manager: Arc<ChannelHub>,
}

impl SendMessageTool {
    pub fn new(channel_manager: Arc<ChannelHub>) -> Self {
        Self { channel_manager }
    }
}

#[async_trait::async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        SEND_MESSAGE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Send a message to the current external chat platform (Telegram, Feishu, etc.). \
         Supports text content and/or file attachments."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Text content to send."
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of relative paths to files under the current workspace to attach."
                }
            }
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let content = args.get("content").and_then(Value::as_str);
        let files = args.get("files").and_then(Value::as_array);

        if content.is_none() && files.is_none() {
            return Err(crate::types::KernelError::tool(
                "Either 'content' or 'files' must be provided",
            ));
        }

        let session_id = SessionId::from(ctx.session_id.clone());
        let (routing, adapter) = self
            .channel_manager
            .get_routing_for_session(&session_id)
            .await
            .map_err(|e| crate::types::KernelError::tool(format!("Failed to find channel: {e}")))?
            .ok_or_else(|| {
                crate::types::KernelError::tool(
                    "This session is not connected to an external chat platform",
                )
            })?;

        // Resolve and validate all file paths first (atomic check).
        let mut resolved_paths: Vec<std::path::PathBuf> = Vec::new();
        if let Some(files) = files {
            for file in files {
                let path = file
                    .as_str()
                    .ok_or_else(|| crate::types::KernelError::tool("Invalid file path"))?;

                let full_path =
                    resolve_safe_path(&ctx.working_dir, path)
                        .await
                        .ok_or_else(|| {
                            crate::types::KernelError::tool(format!(
                                "Unsafe or invalid path: {path}"
                            ))
                        })?;

                if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                    return Err(crate::types::KernelError::tool(format!(
                        "File not found: {path}"
                    )));
                }

                resolved_paths.push(full_path);
            }
        }

        let chat_id = &routing.external_chat_id;
        let reply_msg_id = routing.reply_msg_id.as_deref();

        // Send files first, then text (so text appears after files in chat).
        if !resolved_paths.is_empty() {
            let refs: Vec<(&std::path::Path, Option<&str>)> =
                resolved_paths.iter().map(|p| (p.as_path(), None)).collect();
            adapter
                .send_files(chat_id, &refs, reply_msg_id)
                .await
                .map_err(|e| {
                    crate::types::KernelError::tool(format!("Failed to send files: {e}"))
                })?;
        }

        if let Some(text) = content {
            let blocks = vec![ContentBlock::Text {
                text: text.to_string(),
            }];
            let _ = adapter
                .send_message(chat_id, blocks, reply_msg_id)
                .await
                .map_err(|e| {
                    crate::types::KernelError::tool(format!("Failed to send message: {e}"))
                })?;
        }

        let parts: Vec<String> = [
            (!resolved_paths.is_empty()).then(|| format!("files: {}", resolved_paths.len())),
            content.is_some().then(|| "message".to_string()),
        ]
        .into_iter()
        .flatten()
        .collect();

        Ok(ToolOutput::text(parts.join(", ")))
    }
}

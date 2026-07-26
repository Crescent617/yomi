//! JSON Lines implementation of `MessageStore`

use super::{storage_err, MessageStore};
use crate::types::{Message, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// File-based message store using JSON Lines format
#[derive(Debug, Clone)]
pub struct JsonlMessageStore {
    base_dir: PathBuf,
    data_dir: PathBuf,
}

impl JsonlMessageStore {
    /// Create new store with the given sessions directory
    pub fn new(base_dir: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            data_dir: data_dir.into(),
        }
    }

    fn file_path(&self, session_id: &str) -> PathBuf {
        let safe_id = session_id.replace(['/', '\\'], "_");
        self.base_dir.join(format!("{safe_id}.jsonl"))
    }

    async fn read_lines(&self, path: &Path, inline_assets: bool) -> Result<Vec<Message>> {
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut messages = Vec::new();

        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| storage_err(e.to_string()))?
        {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(mut msg) = serde_json::from_str::<Message>(&line) {
                if inline_assets {
                    crate::utils::asset::inline_assets_in_message(&mut msg, &self.data_dir).await;
                }
                messages.push(msg);
            }
        }

        Ok(messages)
    }

    /// Serialize a message (extracting inline images) and write it to the file.
    async fn write_message(&self, file: &mut File, msg: &mut Message) -> Result<()> {
        crate::utils::asset::extract_inline_image(msg, &self.data_dir).await;
        let line = serde_json::to_string(msg).map_err(|e| storage_err(e.to_string()))?;
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        file.write_all(b"\n")
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl MessageStore for JsonlMessageStore {
    async fn append(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        let path = self.file_path(session_id);

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| storage_err(e.to_string()))?;
        }

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;

        for msg in messages {
            let mut msg = msg.clone();
            self.write_message(&mut file, &mut msg).await?;
        }

        file.flush().await.map_err(|e| storage_err(e.to_string()))?;
        Ok(())
    }

    async fn get(&self, session_id: &str) -> Result<Vec<Message>> {
        self.read_lines(&self.file_path(session_id), false).await
    }

    async fn get_inlined(&self, session_id: &str) -> Result<Vec<Message>> {
        self.read_lines(&self.file_path(session_id), true).await
    }

    async fn replace(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        let path = self.file_path(session_id);
        let temp_path = path.with_extension("tmp");

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| storage_err(e.to_string()))?;
        }

        // Write to temp file
        let mut file = File::create(&temp_path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;

        for msg in messages {
            let mut msg = msg.clone();
            self.write_message(&mut file, &mut msg).await?;
        }

        file.flush().await.map_err(|e| storage_err(e.to_string()))?;
        drop(file);

        // Atomic rename
        fs::rename(&temp_path, &path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "jsonl_test.rs"]
mod tests;

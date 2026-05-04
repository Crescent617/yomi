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
}

impl JsonlMessageStore {
    /// Create new store with the given sessions directory
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn file_path(&self, session_id: &str) -> PathBuf {
        self.base_dir.join(format!("{session_id}.jsonl"))
    }

    async fn read_lines(&self, path: &Path) -> Result<Vec<Message>> {
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
            if let Ok(msg) = serde_json::from_str::<Message>(&line) {
                messages.push(msg);
            }
        }

        Ok(messages)
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
            let line = serde_json::to_string(msg).map_err(|e| storage_err(e.to_string()))?;
            file.write_all(line.as_bytes())
                .await
                .map_err(|e| storage_err(e.to_string()))?;
            file.write_all(b"\n")
                .await
                .map_err(|e| storage_err(e.to_string()))?;
        }

        file.flush().await.map_err(|e| storage_err(e.to_string()))?;
        Ok(())
    }

    async fn get(&self, session_id: &str) -> Result<Vec<Message>> {
        self.read_lines(&self.file_path(session_id)).await
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
            let line = serde_json::to_string(msg).map_err(|e| storage_err(e.to_string()))?;
            file.write_all(line.as_bytes())
                .await
                .map_err(|e| storage_err(e.to_string()))?;
            file.write_all(b"\n")
                .await
                .map_err(|e| storage_err(e.to_string()))?;
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
mod tests {
    use super::*;
    use crate::types::Message;
    use tempfile::TempDir;

    fn create_test_store() -> (JsonlMessageStore, TempDir) {
        let temp = TempDir::new().unwrap();
        let store = JsonlMessageStore::new(temp.path());
        (store, temp)
    }

    #[tokio::test]
    async fn test_append_and_get() {
        let (store, _temp) = create_test_store();

        store
            .append(
                "session-1",
                &[Message::user("hello"), Message::assistant("hi")],
            )
            .await
            .unwrap();

        let messages = store.get("session-1").await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text_content(), "hello");
        assert_eq!(messages[1].text_content(), "hi");
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let (store, _temp) = create_test_store();

        let messages = store.get("nonexistent").await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_replace() {
        let (store, _temp) = create_test_store();

        store
            .append(
                "session-1",
                &[Message::user("old"), Message::assistant("data")],
            )
            .await
            .unwrap();

        store
            .replace("session-1", &[Message::user("compacted")])
            .await
            .unwrap();

        let messages = store.get("session-1").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text_content(), "compacted");
    }
}

//! File state tracking - track file modification times for stale read detection

use crate::types::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// File modification state for a single file
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileState {
    pub path: PathBuf,
    pub mtime: u64,
}

impl FileState {
    /// Create a new file state
    pub fn new(path: PathBuf, mtime: u64) -> Self {
        Self { path, mtime }
    }
}

/// Storage for file modification states
#[async_trait]
pub trait FileStateStore: Send + Sync {
    /// Record a file state
    async fn record(&self, path: PathBuf, mtime: u64) -> Result<()>;

    /// Record multiple file states efficiently
    ///
    /// Default implementation records one by one. Implementations should
    /// override this for better performance when batching is supported.
    async fn record_batch(&self, states: Vec<FileState>) -> Result<()> {
        for state in states {
            self.record(state.path, state.mtime).await?;
        }
        Ok(())
    }

    /// Get all recorded file states (deduplicated by path, last wins)
    async fn read_all(&self) -> Result<Vec<FileState>>;

    /// Clear all file states
    async fn truncate(&self) -> Result<()>;
}

pub mod jsonl;
pub use jsonl::JsonlFileStateStore;

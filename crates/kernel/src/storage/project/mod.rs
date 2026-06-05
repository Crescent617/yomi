//! Project management - project lifecycle and metadata storage

use crate::types::{KernelError, Project, ProjectId, Result};
use async_trait::async_trait;

/// Storage for project lifecycle and metadata
#[async_trait]
pub trait ProjectStore: Send + Sync {
    /// Create a new project
    async fn create(&self, id: &ProjectId, name: &str, dir: &str) -> Result<()>;

    /// Get project by ID
    async fn get(&self, id: &ProjectId) -> Result<Option<Project>>;

    /// Get project by directory path
    async fn get_by_dir(&self, dir: &str) -> Result<Option<Project>>;

    /// List all projects, ordered by `updated_at` DESC
    async fn list(&self) -> Result<Vec<Project>>;

    /// Update project name
    async fn update_name(&self, id: &ProjectId, name: &str) -> Result<()>;

    /// Touch project (update `updated_at`)
    async fn touch(&self, id: &ProjectId) -> Result<()>;

    /// Delete a project
    async fn delete(&self, id: &ProjectId) -> Result<()>;
}

/// Helper for storage errors
fn storage_err(msg: impl Into<String>) -> KernelError {
    KernelError::Storage(msg.into())
}

pub mod sqlite;
pub use sqlite::SqliteProjectStore;

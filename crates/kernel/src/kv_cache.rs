//! Generic persistent KV cache (`<data_dir>/cache.db`).
//!
//! A disposable sidecar store for data that is cheap to rebuild but nice
//! to keep across restarts (e.g. the text of cards the bot sent, whose
//! content the platform will not serve back). It lives in its own
//! database file, separate from the authoritative `yomi.db`: deleting it
//! loses nothing but cache warmth.
//!
//! Namespaced, stringly-typed, no TTL machinery — callers prune by age
//! (`prune_older_than`) when they write.

use crate::types::{KernelError, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use std::path::Path;

pub struct KvCache {
    pool: SqlitePool,
}

impl KvCache {
    /// Open (creating if needed) the cache database at `path`.
    pub async fn open(path: &Path) -> Result<Self> {
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .pragma("busy_timeout", "5000")
                .pragma("journal_mode", "WAL"),
        )
        .await
        .map_err(|e| KernelError::storage(format!("failed to connect cache db: {e}")))?;
        sqlx::query(
            r"CREATE TABLE IF NOT EXISTS kv (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (namespace, key)
            );",
        )
        .execute(&pool)
        .await
        .map_err(|e| KernelError::storage(format!("failed to init kv table: {e}")))?;
        Ok(Self { pool })
    }

    /// Read one entry (`None` when absent).
    pub async fn get(&self, namespace: &str, key: &str) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT value FROM kv WHERE namespace = ? AND key = ?")
            .bind(namespace)
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| KernelError::storage(format!("kv get failed: {e}")))
    }

    /// Insert or replace one entry, refreshing its timestamp.
    pub async fn put(&self, namespace: &str, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            r"INSERT INTO kv (namespace, key, value, created_at) VALUES (?, ?, ?, ?)
              ON CONFLICT(namespace, key) DO UPDATE SET
              value = excluded.value, created_at = excluded.created_at",
        )
        .bind(namespace)
        .bind(key)
        .bind(value)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await
        .map_err(|e| KernelError::storage(format!("kv put failed: {e}")))?;
        Ok(())
    }

    /// Delete a namespace's entries older than `cutoff_ms` (unix ms).
    /// Returns the number of rows deleted.
    pub async fn prune_older_than(&self, namespace: &str, cutoff_ms: i64) -> Result<u64> {
        let res = sqlx::query("DELETE FROM kv WHERE namespace = ? AND created_at < ?")
            .bind(namespace)
            .bind(cutoff_ms)
            .execute(&self.pool)
            .await
            .map_err(|e| KernelError::storage(format!("kv prune failed: {e}")))?;
        Ok(res.rows_affected())
    }

    /// Count entries older than `cutoff_ms` across all namespaces (gc dry-run).
    pub async fn count_older_than(&self, cutoff_ms: i64) -> Result<u64> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM kv WHERE created_at < ?")
            .bind(cutoff_ms)
            .fetch_one(&self.pool)
            .await
            .map(|n| n as u64)
            .map_err(|e| KernelError::storage(format!("kv count failed: {e}")))
    }

    /// Delete entries older than `cutoff_ms` across all namespaces (gc).
    /// Returns the number of rows deleted.
    pub async fn sweep_older_than(&self, cutoff_ms: i64) -> Result<u64> {
        let res = sqlx::query("DELETE FROM kv WHERE created_at < ?")
            .bind(cutoff_ms)
            .execute(&self.pool)
            .await
            .map_err(|e| KernelError::storage(format!("kv sweep failed: {e}")))?;
        Ok(res.rows_affected())
    }

    /// Reclaim disk space (deletions leave free pages behind).
    pub async fn vacuum(&self) -> Result<()> {
        sqlx::query("VACUUM")
            .execute(&self.pool)
            .await
            .map_err(|e| KernelError::storage(format!("kv vacuum failed: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "kv_cache_test.rs"]
mod tests;

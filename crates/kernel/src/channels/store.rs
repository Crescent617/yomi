use crate::channels::{ChannelStore, SessionRouting};
use crate::storage::storage_err;
use crate::types::{Result, SessionId};
use async_trait::async_trait;
use sqlx::sqlite::SqlitePool;

pub struct SqliteChannelStore {
    pool: SqlitePool,
}

impl SqliteChannelStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ChannelStore for SqliteChannelStore {
    async fn save_mapping(
        &self,
        channel_name: &str,
        mapping_key: &str,
        session_id: &SessionId,
        actual_chat_id: &str,
        reply_msg_id: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r"INSERT INTO channel_session_mappings
               (channel_name, external_chat_id, session_id, actual_chat_id, reply_msg_id, created_at)
               VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
               ON CONFLICT(channel_name, external_chat_id) DO UPDATE SET
               session_id = excluded.session_id,
               actual_chat_id = excluded.actual_chat_id,
               reply_msg_id = excluded.reply_msg_id,
               created_at = CURRENT_TIMESTAMP",
        )
        .bind(channel_name)
        .bind(mapping_key)
        .bind(session_id.as_str())
        .bind(actual_chat_id)
        .bind(reply_msg_id)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to save channel mapping: {e}")))?;

        Ok(())
    }

    async fn find_mapping(
        &self,
        channel_name: &str,
        mapping_key: &str,
    ) -> Result<Option<SessionId>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT session_id FROM channel_session_mappings
             WHERE channel_name = ? AND external_chat_id = ?",
        )
        .bind(channel_name)
        .bind(mapping_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to find channel mapping: {e}")))?;

        Ok(row.map(|r| SessionId::from(r.0)))
    }

    async fn list_mappings(&self, channel_name: &str) -> Result<Vec<(String, SessionId)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT external_chat_id, session_id FROM channel_session_mappings
             WHERE channel_name = ?",
        )
        .bind(channel_name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to list channel mappings: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|(chat_id, sid)| (chat_id, SessionId::from(sid)))
            .collect())
    }

    async fn find_routing_by_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionRouting>> {
        let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT channel_name, COALESCE(actual_chat_id, external_chat_id) AS actual_chat_id, reply_msg_id
             FROM channel_session_mappings
             WHERE session_id = ?",
        )
        .bind(session_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to find routing by session: {e}")))?;

        Ok(row.map(
            |(channel_name, actual_chat_id, reply_msg_id)| SessionRouting {
                channel_name,
                external_chat_id: actual_chat_id.unwrap_or_default(),
                reply_msg_id,
            },
        ))
    }

    async fn delete_mapping(&self, channel_name: &str, mapping_key: &str) -> Result<()> {
        sqlx::query(
            "DELETE FROM channel_session_mappings
             WHERE channel_name = ? AND external_chat_id = ?",
        )
        .bind(channel_name)
        .bind(mapping_key)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to delete channel mapping: {e}")))?;

        Ok(())
    }

    async fn delete_by_sessions(&self, session_ids: &[SessionId]) -> Result<u64> {
        const CHUNK_SIZE: usize = 100;

        let mut deleted = 0u64;
        for chunk in session_ids.chunks(CHUNK_SIZE) {
            let mut builder = sqlx::QueryBuilder::new(
                "DELETE FROM channel_session_mappings WHERE session_id IN (",
            );
            let mut separated = builder.separated(", ");
            for id in chunk {
                separated.push_bind(id.as_str());
            }
            separated.push_unseparated(")");

            let result = builder.build().execute(&self.pool).await.map_err(|e| {
                storage_err(format!("Failed to delete channel mappings by session: {e}"))
            })?;
            deleted += result.rows_affected();
        }
        Ok(deleted)
    }

    async fn get_history_cursor(
        &self,
        channel_name: &str,
        container_id: &str,
    ) -> Result<Option<i64>> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT cursor_ts FROM channel_history_cursors
             WHERE channel_name = ? AND container_id = ?",
        )
        .bind(channel_name)
        .bind(container_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to get history cursor: {e}")))?;

        Ok(row.map(|r| r.0))
    }

    async fn set_history_cursor(
        &self,
        channel_name: &str,
        container_id: &str,
        cursor_ts: i64,
    ) -> Result<()> {
        sqlx::query(
            r"INSERT INTO channel_history_cursors (channel_name, container_id, cursor_ts, updated_at)
               VALUES (?, ?, ?, CURRENT_TIMESTAMP)
               ON CONFLICT(channel_name, container_id) DO UPDATE SET
               cursor_ts = excluded.cursor_ts,
               updated_at = CURRENT_TIMESTAMP",
        )
        .bind(channel_name)
        .bind(container_id)
        .bind(cursor_ts)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to set history cursor: {e}")))?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "store_test.rs"]
mod tests;

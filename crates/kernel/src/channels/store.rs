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
}

#[cfg(test)]
#[path = "store_test.rs"]
mod tests;

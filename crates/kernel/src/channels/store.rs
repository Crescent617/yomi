use crate::storage::storage_err;
use crate::types::{Result, SessionId};
use async_trait::async_trait;
use sqlx::sqlite::SqlitePool;

use super::ChannelStore;

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
        external_chat_id: &str,
        session_id: &SessionId,
    ) -> Result<()> {
        sqlx::query(
            r"INSERT INTO channel_session_mappings (channel_name, external_chat_id, session_id, created_at)
               VALUES (?, ?, ?, CURRENT_TIMESTAMP)
               ON CONFLICT(channel_name, external_chat_id) DO UPDATE SET
               session_id = excluded.session_id, created_at = CURRENT_TIMESTAMP",
        )
        .bind(channel_name)
        .bind(external_chat_id)
        .bind(session_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to save channel mapping: {e}")))?;

        Ok(())
    }

    async fn find_mapping(
        &self,
        channel_name: &str,
        external_chat_id: &str,
    ) -> Result<Option<SessionId>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT session_id FROM channel_session_mappings
             WHERE channel_name = ? AND external_chat_id = ?",
        )
        .bind(channel_name)
        .bind(external_chat_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to find channel mapping: {e}")))?;

        Ok(row.map(|r| SessionId::from_string(r.0)))
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
            .map(|(chat_id, sid)| (chat_id, SessionId::from_string(sid)))
            .collect())
    }

    async fn find_by_session_id(&self, session_id: &SessionId) -> Result<Option<(String, String)>> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT channel_name, external_chat_id FROM channel_session_mappings
             WHERE session_id = ?",
        )
        .bind(session_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to find channel mapping by session: {e}")))?;

        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn create_test_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    async fn run_migrations(pool: &SqlitePool) {
        sqlx::query(
            r"CREATE TABLE channel_session_mappings (
                channel_name TEXT NOT NULL,
                external_chat_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (channel_name, external_chat_id)
            );
            CREATE INDEX idx_channel_mapping_session ON channel_session_mappings(session_id);",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_save_and_find_mapping() {
        let pool = create_test_pool().await;
        run_migrations(&pool).await;
        let store = SqliteChannelStore::new(pool);

        let sid = SessionId::new();
        store.save_mapping("tg_bot", "12345", &sid).await.unwrap();

        let found = store.find_mapping("tg_bot", "12345").await.unwrap();
        assert_eq!(found, Some(sid));

        let not_found = store.find_mapping("tg_bot", "99999").await.unwrap();
        assert_eq!(not_found, None);
    }

    #[tokio::test]
    async fn test_update_mapping() {
        let pool = create_test_pool().await;
        run_migrations(&pool).await;
        let store = SqliteChannelStore::new(pool);

        let sid1 = SessionId::new();
        let sid2 = SessionId::new();
        store.save_mapping("tg_bot", "12345", &sid1).await.unwrap();
        store.save_mapping("tg_bot", "12345", &sid2).await.unwrap();

        let found = store.find_mapping("tg_bot", "12345").await.unwrap();
        assert_eq!(found, Some(sid2));
    }

    #[tokio::test]
    async fn test_list_mappings() {
        let pool = create_test_pool().await;
        run_migrations(&pool).await;
        let store = SqliteChannelStore::new(pool);

        let sid1 = SessionId::new();
        let sid2 = SessionId::new();
        store.save_mapping("tg_bot", "111", &sid1).await.unwrap();
        store.save_mapping("tg_bot", "222", &sid2).await.unwrap();
        store
            .save_mapping("other", "333", &SessionId::new())
            .await
            .unwrap();

        let mappings = store.list_mappings("tg_bot").await.unwrap();
        assert_eq!(mappings.len(), 2);
    }

    #[tokio::test]
    async fn test_find_by_session_id() {
        let pool = create_test_pool().await;
        run_migrations(&pool).await;
        let store = SqliteChannelStore::new(pool);

        let sid = SessionId::new();
        store.save_mapping("tg_bot", "12345", &sid).await.unwrap();

        let found = store.find_by_session_id(&sid).await.unwrap();
        assert_eq!(found, Some(("tg_bot".to_string(), "12345".to_string())));

        let not_found = store.find_by_session_id(&SessionId::new()).await.unwrap();
        assert_eq!(not_found, None);
    }
}

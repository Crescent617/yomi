use super::{storage_err, AddFavoriteInput, FavoriteAnswer, FavoriteStore};
use crate::types::{MessageId, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

/// SQLite-backed favorite answer store
#[derive(Debug, Clone)]
pub struct SqliteFavoriteStore {
    pool: SqlitePool,
}

impl SqliteFavoriteStore {
    /// Create a new store
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct FavoriteRow {
    id: String,
    session_id: String,
    message_id: String,
    session_title: Option<String>,
    content: String,
    note: Option<String>,
    favorited_at: DateTime<Utc>,
    message_created_at: Option<DateTime<Utc>>,
}

impl From<FavoriteRow> for FavoriteAnswer {
    fn from(row: FavoriteRow) -> Self {
        Self {
            id: row.id,
            session_id: SessionId::from(row.session_id),
            message_id: MessageId::from(row.message_id),
            session_title: row.session_title,
            content: row.content,
            note: row.note,
            favorited_at: row.favorited_at,
            message_created_at: row.message_created_at,
        }
    }
}

#[async_trait]
impl FavoriteStore for SqliteFavoriteStore {
    async fn add(&self, input: AddFavoriteInput) -> Result<FavoriteAnswer> {
        let id = format!("fav_{}", ulid::Ulid::new());
        sqlx::query(
            "INSERT INTO favorite_answers
                 (id, session_id, message_id, session_title, content, note,
                  favorited_at, message_created_at)
             VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, ?)
             ON CONFLICT(session_id, message_id) DO UPDATE SET
                 content = excluded.content,
                 session_title = excluded.session_title,
                 note = COALESCE(excluded.note, favorite_answers.note),
                 favorited_at = excluded.favorited_at,
                 message_created_at = COALESCE(
                     excluded.message_created_at,
                     favorite_answers.message_created_at
                 )",
        )
        .bind(&id)
        .bind(input.session_id.as_str())
        .bind(input.message_id.as_str())
        .bind(&input.session_title)
        .bind(&input.content)
        .bind(&input.note)
        .bind(input.message_created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to add favorite: {e}")))?;

        self.get_by_message(&input.session_id, &input.message_id)
            .await?
            .ok_or_else(|| storage_err("failed to read back favorite after add"))
    }

    async fn remove(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM favorite_answers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to remove favorite: {e}")))?;
        Ok(())
    }

    async fn remove_by_message(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<()> {
        sqlx::query("DELETE FROM favorite_answers WHERE session_id = ? AND message_id = ?")
            .bind(session_id.as_str())
            .bind(message_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to remove favorite by message: {e}")))?;
        Ok(())
    }

    async fn get_by_message(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<Option<FavoriteAnswer>> {
        let row = sqlx::query_as::<_, FavoriteRow>(
            "SELECT id, session_id, message_id, session_title, content, note,
                    favorited_at, message_created_at
             FROM favorite_answers
             WHERE session_id = ? AND message_id = ?",
        )
        .bind(session_id.as_str())
        .bind(message_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to get favorite: {e}")))?;
        Ok(row.map(Into::into))
    }

    async fn list(
        &self,
        query: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<FavoriteAnswer>> {
        // Escape LIKE wildcards in user input so search is literal.
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(|q| {
            let escaped = q
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        });
        let rows = sqlx::query_as::<_, FavoriteRow>(
            "SELECT id, session_id, message_id, session_title, content, note,
                    favorited_at, message_created_at
             FROM favorite_answers
             WHERE ?1 IS NULL
                OR content LIKE ?1 ESCAPE '\\'
                OR note LIKE ?1 ESCAPE '\\'
                OR session_title LIKE ?1 ESCAPE '\\'
             ORDER BY favorited_at DESC
             LIMIT ?2 OFFSET ?3",
        )
        .bind(&pattern)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to list favorites: {e}")))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn update_note(&self, id: &str, note: Option<&str>) -> Result<()> {
        let result = sqlx::query("UPDATE favorite_answers SET note = ? WHERE id = ?")
            .bind(note)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to update favorite note: {e}")))?;
        if result.rows_affected() == 0 {
            return Err(storage_err(format!("favorite not found: {id}")));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "sqlite_test.rs"]
mod tests;

use crate::channels::{
    ChannelStore, DocPermissionRequest, PermRequestRow, RunSubscriptionRow, SessionRouting,
};
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

#[derive(sqlx::FromRow)]
struct PermRequestDbRow {
    id: i64,
    channel_name: String,
    file_token: String,
    file_type: String,
    permission: String,
    remark: Option<String>,
    applicant_users: String,
    applicant_chats: String,
    applicant_departments: String,
    status: String,
    notify_msg_ids: String,
    resolved_by: Option<String>,
    resolved_perm: Option<String>,
    created_at: String,
}

impl PermRequestDbRow {
    fn into_row(self) -> PermRequestRow {
        let parse_list = |raw: &str| {
            serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|e| {
                tracing::warn!(raw, error = %e, "corrupt JSON list in perm request row");
                Vec::new()
            })
        };
        PermRequestRow {
            id: self.id,
            channel_name: self.channel_name,
            file_token: self.file_token,
            file_type: self.file_type,
            permission: self.permission,
            remark: self.remark,
            applicant_users: parse_list(&self.applicant_users),
            applicant_chats: parse_list(&self.applicant_chats),
            applicant_departments: parse_list(&self.applicant_departments),
            status: self.status,
            notify_msg_ids: parse_list(&self.notify_msg_ids),
            resolved_by: self.resolved_by,
            resolved_perm: self.resolved_perm,
            created_at: self.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct RunSubscriptionDbRow {
    id: i64,
    channel_name: String,
    scope_key: String,
    chat_id: String,
    recursive: bool,
    subscriber_open_id: String,
    target_chat_id: Option<String>,
    created_at: String,
}

impl RunSubscriptionDbRow {
    fn into_row(self) -> RunSubscriptionRow {
        RunSubscriptionRow {
            id: self.id,
            channel_name: self.channel_name,
            scope_key: self.scope_key,
            chat_id: self.chat_id,
            recursive: self.recursive,
            subscriber_open_id: self.subscriber_open_id,
            target_chat_id: self.target_chat_id,
            created_at: self.created_at,
        }
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
        let row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT channel_name, COALESCE(actual_chat_id, external_chat_id) AS actual_chat_id, reply_msg_id, external_chat_id
             FROM channel_session_mappings
             WHERE session_id = ?",
        )
        .bind(session_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to find routing by session: {e}")))?;

        Ok(row.map(
            |(channel_name, actual_chat_id, reply_msg_id, mapping_key)| SessionRouting {
                channel_name,
                external_chat_id: actual_chat_id.unwrap_or_default(),
                reply_msg_id,
                mapping_key,
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

    async fn save_perm_request(
        &self,
        channel_name: &str,
        req: &DocPermissionRequest,
    ) -> Result<Option<i64>> {
        let users = serde_json::to_string(&req.applicant_users)
            .map_err(|e| storage_err(format!("Failed to encode applicants: {e}")))?;
        let chats = serde_json::to_string(&req.applicant_chats)
            .map_err(|e| storage_err(format!("Failed to encode applicants: {e}")))?;
        let departments = serde_json::to_string(&req.applicant_departments)
            .map_err(|e| storage_err(format!("Failed to encode applicants: {e}")))?;

        // ws redelivery dedup: the same application still pending → skip.
        let duplicate: Option<(i64,)> = sqlx::query_as(
            r"SELECT id FROM channel_doc_permission_requests
             WHERE channel_name = ? AND file_token = ? AND permission = ?
               AND applicant_users = ? AND applicant_chats = ? AND applicant_departments = ?
               AND status = 'pending'",
        )
        .bind(channel_name)
        .bind(&req.file_token)
        .bind(&req.permission)
        .bind(&users)
        .bind(&chats)
        .bind(&departments)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to check duplicate perm request: {e}")))?;
        if duplicate.is_some() {
            return Ok(None);
        }

        let result = sqlx::query(
            r"INSERT INTO channel_doc_permission_requests
               (channel_name, file_token, file_type, permission, remark,
                applicant_users, applicant_chats, applicant_departments)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(channel_name)
        .bind(&req.file_token)
        .bind(&req.file_type)
        .bind(&req.permission)
        .bind(&req.remark)
        .bind(&users)
        .bind(&chats)
        .bind(&departments)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to save perm request: {e}")))?;

        Ok(Some(result.last_insert_rowid()))
    }

    async fn set_perm_notify_msgs(&self, id: i64, msg_ids: &[String]) -> Result<()> {
        let ids = serde_json::to_string(msg_ids)
            .map_err(|e| storage_err(format!("Failed to encode notify msg ids: {e}")))?;
        sqlx::query("UPDATE channel_doc_permission_requests SET notify_msg_ids = ? WHERE id = ?")
            .bind(ids)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("Failed to set perm notify msgs: {e}")))?;
        Ok(())
    }

    async fn list_pending_perm_requests(&self, channel_name: &str) -> Result<Vec<PermRequestRow>> {
        let rows = sqlx::query_as::<_, PermRequestDbRow>(
            r"SELECT id, channel_name, file_token, file_type, permission, remark,
                     applicant_users, applicant_chats, applicant_departments,
                     status, notify_msg_ids, resolved_by, resolved_perm, created_at
              FROM channel_doc_permission_requests
              WHERE channel_name = ? AND status = 'pending' ORDER BY id",
        )
        .bind(channel_name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to list pending perm requests: {e}")))?;

        Ok(rows.into_iter().map(PermRequestDbRow::into_row).collect())
    }

    async fn resolve_perm_request(
        &self,
        id: i64,
        status: &str,
        resolved_by: &str,
        resolved_perm: Option<&str>,
    ) -> Result<Option<PermRequestRow>> {
        let result = sqlx::query(
            r"UPDATE channel_doc_permission_requests
               SET status = ?, resolved_by = ?, resolved_perm = ?, resolved_at = CURRENT_TIMESTAMP
               WHERE id = ? AND status = 'pending'",
        )
        .bind(status)
        .bind(resolved_by)
        .bind(resolved_perm)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to resolve perm request: {e}")))?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }

        self.get_perm_request(id).await.map(Some)
    }

    async fn reopen_perm_request(&self, id: i64) -> Result<()> {
        // Only an approved row can be reopened (grant failed after winning
        // the race) — the guard keeps future misuses from silently
        // resurrecting denied or foreign-state rows.
        let result = sqlx::query(
            r"UPDATE channel_doc_permission_requests
               SET status = 'pending', resolved_by = NULL, resolved_perm = NULL, resolved_at = NULL
               WHERE id = ? AND status = 'approved'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to reopen perm request: {e}")))?;
        if result.rows_affected() == 0 {
            tracing::warn!(id, "reopen_perm_request matched no approved row");
        }
        Ok(())
    }

    async fn save_run_subscription(
        &self,
        channel_name: &str,
        scope_key: &str,
        chat_id: &str,
        recursive: bool,
        subscriber_open_id: &str,
        target_chat_id: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r"INSERT INTO channel_run_subscriptions
               (channel_name, scope_key, chat_id, recursive, subscriber_open_id, target_chat_id)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(channel_name, scope_key, subscriber_open_id) DO UPDATE SET
               recursive = excluded.recursive,
               target_chat_id = excluded.target_chat_id",
        )
        .bind(channel_name)
        .bind(scope_key)
        .bind(chat_id)
        .bind(recursive)
        .bind(subscriber_open_id)
        .bind(target_chat_id)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to save run subscription: {e}")))?;
        Ok(())
    }

    async fn remove_run_subscription(
        &self,
        channel_name: &str,
        scope_key: &str,
        subscriber_open_id: &str,
    ) -> Result<u64> {
        let result = sqlx::query(
            r"DELETE FROM channel_run_subscriptions
             WHERE channel_name = ? AND scope_key = ? AND subscriber_open_id = ?",
        )
        .bind(channel_name)
        .bind(scope_key)
        .bind(subscriber_open_id)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to remove run subscription: {e}")))?;
        Ok(result.rows_affected())
    }

    async fn list_matching_run_subscriptions(
        &self,
        channel_name: &str,
        scope_key: &str,
        chat_id: &str,
    ) -> Result<Vec<RunSubscriptionRow>> {
        let rows = sqlx::query_as::<_, RunSubscriptionDbRow>(
            r"SELECT id, channel_name, scope_key, chat_id, recursive, subscriber_open_id,
                     target_chat_id, created_at
              FROM channel_run_subscriptions
              WHERE channel_name = ?
                AND (scope_key = ? OR (recursive != 0 AND chat_id = ?))",
        )
        .bind(channel_name)
        .bind(scope_key)
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to list run subscriptions: {e}")))?;
        Ok(rows
            .into_iter()
            .map(RunSubscriptionDbRow::into_row)
            .collect())
    }
}

impl SqliteChannelStore {
    async fn get_perm_request(&self, id: i64) -> Result<PermRequestRow> {
        let row = sqlx::query_as::<_, PermRequestDbRow>(
            r"SELECT id, channel_name, file_token, file_type, permission, remark,
                     applicant_users, applicant_chats, applicant_departments,
                     status, notify_msg_ids, resolved_by, resolved_perm, created_at
              FROM channel_doc_permission_requests WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| storage_err(format!("Failed to get perm request: {e}")))?;
        Ok(row.into_row())
    }
}

#[cfg(test)]
#[path = "store_test.rs"]
mod tests;

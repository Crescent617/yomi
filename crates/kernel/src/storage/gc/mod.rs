//! Garbage collection for expired session resources
//!
//! A session's resources live in several places:
//! - sqlite `sessions` row (`pinned_sessions` cascades via FK)
//! - sqlite `channel_session_mappings` rows (no FK)
//! - `sessions/{id}.jsonl` (message history)
//! - `sessions/todos/{id}.json`
//! - `sessions/goals/{id}.json`
//! - `sessions/file_states/{id}.jsonl`
//! - `checkpoints/{id}/` (self-contained directory)
//!
//! The `token_usage` table is deliberately **never touched**: usage data is a
//! cross-session statistics asset (used by `yomi usage`) and has no FK, so
//! keeping rows for deleted sessions produces no dangling references.
//!
//! Execution order matters: DB rows are deleted before files, so that a crash
//! mid-gc leaves orphan files (recoverable by the orphan sweep on the next
//! run) rather than dangling DB rows pointing at missing files.

use crate::types::{Result, SessionId};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::StorageSet;

/// Stale `.tmp` files older than this are removed during the orphan sweep.
const TMP_STALE_SECS: u64 = 3600;

/// Options controlling a gc run
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct GcOptions {
    /// Sessions with `updated_at` older than `now - days` are collected (min 1)
    pub days: i64,
    /// Skip pinned sessions (default true)
    pub keep_pinned: bool,
    /// Sweep orphan files whose session no longer exists in the DB (default true)
    pub sweep_orphans: bool,
    /// Run `VACUUM` + WAL truncate after deletion (default false)
    pub vacuum: bool,
    /// Only report, delete nothing
    pub dry_run: bool,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            days: 90,
            keep_pinned: true,
            sweep_orphans: true,
            vacuum: false,
            dry_run: true,
        }
    }
}

/// Report of what a gc run deleted (or would delete, in dry-run mode)
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GcReport {
    /// Collected session IDs (includes subagent sessions)
    pub sessions: Vec<SessionId>,
    /// Number of collected subagent sessions (subset of `sessions`)
    pub subagent_sessions: u64,
    /// Data files deleted (`messages/todos/goals/file_states`)
    pub files_deleted: u64,
    /// Checkpoint directories deleted
    pub checkpoint_dirs_deleted: u64,
    /// Channel mapping rows deleted
    pub channel_mappings_deleted: u64,
    /// Orphan files deleted during the sweep (incl. stale `.tmp`)
    pub orphan_files_deleted: u64,
    /// Bytes reclaimed (estimated from file sizes before deletion)
    pub bytes_reclaimed: u64,
    /// Non-fatal errors encountered (gc continues past individual failures)
    pub errors: Vec<String>,
    /// Whether this was a dry run
    pub dry_run: bool,
}

/// Garbage collector for session resources.
///
/// Obtain via [`StorageSet::gc`].
pub struct GarbageCollector {
    storage: StorageSet,
}

impl GarbageCollector {
    pub(super) fn new(storage: StorageSet) -> Self {
        Self { storage }
    }

    /// Run garbage collection according to `opts`.
    pub async fn run(&self, opts: &GcOptions) -> Result<GcReport> {
        let days = opts.days.max(1);
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days);

        let mut report = GcReport {
            dry_run: opts.dry_run,
            ..GcReport::default()
        };

        // ── Phase 1: find victims ────────────────────────────────────
        let victims = self
            .storage
            .session_store()
            .list_expired(cutoff, opts.keep_pinned)
            .await?;
        report.subagent_sessions = victims.iter().filter(|id| id.is_subagent()).count() as u64;

        if opts.dry_run {
            // Estimate sizes without deleting
            for id in &victims {
                for path in self.session_files(id.as_str()) {
                    if let Ok(meta) = tokio::fs::metadata(&path).await {
                        report.bytes_reclaimed += meta.len();
                        report.files_deleted += 1;
                    }
                }
                let cp_dir = self.checkpoints_dir().join(id.as_str());
                if cp_dir.is_dir() {
                    report.bytes_reclaimed += dir_size(&cp_dir).await;
                    report.checkpoint_dirs_deleted += 1;
                }
            }
            if opts.sweep_orphans {
                self.sweep_orphans(&mut report, true).await;
            }
            report.sessions = victims;
            return Ok(report);
        }

        // ── Phase 2+3: delete DB rows then files ─────────────────────
        self.purge_into(&victims, &mut report).await?;

        // ── Phase 4: orphan sweep ────────────────────────────────────
        if opts.sweep_orphans {
            self.sweep_orphans(&mut report, false).await;
        }

        // ── Phase 5: vacuum ──────────────────────────────────────────
        if opts.vacuum {
            if let Err(e) = self.vacuum().await {
                report.errors.push(format!("vacuum: {e}"));
            }
        }

        report.sessions = victims;
        Ok(report)
    }

    /// Purge the given sessions and all their resources (DB rows, data files,
    /// checkpoint directories, channel mappings). Unlike [`Self::run`], this
    /// ignores age/pin status — the caller decides *what* to delete.
    ///
    /// Used by project cascade deletion. `token_usage` rows are never touched.
    pub async fn purge_sessions(&self, ids: &[SessionId]) -> Result<GcReport> {
        let mut report = GcReport {
            subagent_sessions: ids.iter().filter(|id| id.is_subagent()).count() as u64,
            ..GcReport::default()
        };
        self.purge_into(ids, &mut report).await?;
        report.sessions = ids.to_vec();
        Ok(report)
    }

    /// Delete DB rows (before files) and then per-session files for `victims`.
    async fn purge_into(&self, victims: &[SessionId], report: &mut GcReport) -> Result<()> {
        // DB rows first: a crash mid-way leaves orphan files (recoverable by
        // the orphan sweep) rather than dangling DB rows.
        if !victims.is_empty() {
            self.storage.session_store().delete_batch(victims).await?;
            report.channel_mappings_deleted = self
                .storage
                .channel_store()
                .delete_by_sessions(victims)
                .await?;
        }

        for id in victims {
            for path in self.session_files(id.as_str()) {
                match remove_file_sized(&path).await {
                    Ok(Some(size)) => {
                        report.files_deleted += 1;
                        report.bytes_reclaimed += size;
                    }
                    Ok(None) => {}
                    Err(e) => report.errors.push(format!("{}: {e}", path.display())),
                }
            }

            let cp_dir = self.checkpoints_dir().join(id.as_str());
            if cp_dir.is_dir() {
                let size = dir_size(&cp_dir).await;
                match tokio::fs::remove_dir_all(&cp_dir).await {
                    Ok(()) => {
                        report.checkpoint_dirs_deleted += 1;
                        report.bytes_reclaimed += size;
                    }
                    Err(e) => report.errors.push(format!("{}: {e}", cp_dir.display())),
                }
            }
        }
        Ok(())
    }

    /// Per-session data files (messages, todos, goals, file states)
    fn session_files(&self, id: &str) -> Vec<PathBuf> {
        let sessions_dir = self.sessions_dir();
        vec![
            sessions_dir.join(format!("{id}.jsonl")),
            sessions_dir.join("todos").join(format!("{id}.json")),
            sessions_dir.join("goals").join(format!("{id}.json")),
            sessions_dir.join("file_states").join(format!("{id}.jsonl")),
        ]
    }

    fn sessions_dir(&self) -> PathBuf {
        self.storage.data_dir().join("sessions")
    }

    fn checkpoints_dir(&self) -> PathBuf {
        self.storage.data_dir().join("checkpoints")
    }

    /// Sweep files/directories whose session no longer exists in the DB.
    /// Also removes stale `.tmp` files (atomic-write leftovers).
    async fn sweep_orphans(&self, report: &mut GcReport, dry_run: bool) {
        let live = match self.live_session_ids().await {
            Ok(set) => set,
            Err(e) => {
                report.errors.push(format!("orphan sweep skipped: {e}"));
                return;
            }
        };

        let sessions_dir = self.sessions_dir();
        // (dir, extension) pairs holding per-session files
        let file_dirs = [
            (sessions_dir.clone(), "jsonl"),
            (sessions_dir.join("todos"), "json"),
            (sessions_dir.join("goals"), "json"),
            (sessions_dir.join("file_states"), "jsonl"),
        ];

        for (dir, ext) in &file_dirs {
            let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                // Stale .tmp files: judged by mtime, not by session liveness
                if path.extension().is_some_and(|e| e == "tmp") {
                    if is_stale(&path, TMP_STALE_SECS).await {
                        self.sweep_file(&path, report, dry_run).await;
                    }
                    continue;
                }

                if path.extension().is_none_or(|e| e != *ext) {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !live.contains(stem) {
                    self.sweep_file(&path, report, dry_run).await;
                }
            }
        }

        // Orphan checkpoint directories
        if let Ok(mut entries) = tokio::fs::read_dir(self.checkpoints_dir()).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if live.contains(name) {
                    continue;
                }
                let size = dir_size(&path).await;
                if dry_run {
                    report.orphan_files_deleted += 1;
                    report.bytes_reclaimed += size;
                } else {
                    match tokio::fs::remove_dir_all(&path).await {
                        Ok(()) => {
                            report.orphan_files_deleted += 1;
                            report.bytes_reclaimed += size;
                        }
                        Err(e) => report.errors.push(format!("{}: {e}", path.display())),
                    }
                }
            }
        }
    }

    async fn sweep_file(&self, path: &Path, report: &mut GcReport, dry_run: bool) {
        if dry_run {
            if let Ok(meta) = tokio::fs::metadata(path).await {
                report.orphan_files_deleted += 1;
                report.bytes_reclaimed += meta.len();
            }
            return;
        }
        match remove_file_sized(path).await {
            Ok(Some(size)) => {
                report.orphan_files_deleted += 1;
                report.bytes_reclaimed += size;
            }
            Ok(None) => {}
            Err(e) => report.errors.push(format!("{}: {e}", path.display())),
        }
    }

    /// All session IDs currently present in the DB (including subagents)
    async fn live_session_ids(&self) -> Result<HashSet<String>> {
        use sqlx::Row;
        let rows = sqlx::query("SELECT id FROM sessions")
            .fetch_all(self.storage.pool())
            .await
            .map_err(|e| super::storage_err(format!("failed to list live sessions: {e}")))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.try_get::<String, _>("id").ok())
            .collect())
    }

    async fn vacuum(&self) -> Result<()> {
        sqlx::query("VACUUM")
            .execute(self.storage.pool())
            .await
            .map_err(|e| super::storage_err(format!("vacuum failed: {e}")))?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(self.storage.pool())
            .await
            .map_err(|e| super::storage_err(format!("wal checkpoint failed: {e}")))?;
        Ok(())
    }
}

/// Remove a file, returning its size if it existed.
async fn remove_file_sized(path: &Path) -> std::io::Result<Option<u64>> {
    match tokio::fs::metadata(path).await {
        Ok(meta) => {
            let size = meta.len();
            tokio::fs::remove_file(path).await?;
            Ok(Some(size))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Recursively compute directory size (best-effort; errors count as 0)
async fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// Whether the file's mtime is older than `secs` seconds ago
async fn is_stale(path: &Path, secs: u64) -> bool {
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    mtime.elapsed().is_ok_and(|age| age.as_secs() > secs)
}

#[cfg(test)]
#[path = "gc_test.rs"]
mod tests;

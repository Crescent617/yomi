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
//! - `assets/{hash}.{ext}` (content-addressed files shared by message histories)
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
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::StorageSet;

/// Stale `.tmp` files older than this are removed during the orphan sweep.
const TMP_STALE_SECS: u64 = 3600;
/// Fresh assets may not have been persisted into message history yet.
const ASSET_STALE_SECS: u64 = 3600;

/// Per-session data files as `(subdirectory, extension)` pairs under
/// `sessions/`. Both victim purging ([`GarbageCollector::session_files`]) and
/// the orphan sweep derive their paths from this single list, so adding a new
/// per-session file kind here covers purging and sweeping at once.
const SESSION_FILE_KINDS: &[(&str, &str)] = &[
    ("", "jsonl"), // {id}.jsonl — message history
    ("todos", "json"),
    // legacy: goal mode was removed (2026-08); keep this entry only to
    // clean up pre-existing sessions/goals/*.json files.
    ("goals", "json"),
    ("file_states", "jsonl"),
    ("rules", "md"), // {id}.md — per-session RULE.md (spawn-injected)
];

/// Options controlling a gc run
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct GcOptions {
    /// Sessions with `updated_at` older than `now - retention_days` are collected (min 1)
    pub retention_days: i64,
    /// Skip pinned sessions (default true)
    pub keep_pinned: bool,
    /// Sweep orphan files whose session no longer exists in the DB (default true)
    pub sweep_orphans: bool,
    /// Run `VACUUM` + WAL truncate after deletion (default false)
    pub vacuum: bool,
    /// Only report, delete nothing
    pub dry_run: bool,
    /// Session IDs to exclude from collection (e.g. sessions with a live
    /// in-memory agent). Runtime state, not policy — always empty when built
    /// via [`GcOptions::from_config`].
    pub exclude_sessions: Vec<SessionId>,
}

impl Default for GcOptions {
    fn default() -> Self {
        // Policy defaults live in `crate::config::GcConfig`; dry-run by default.
        Self::from_config(&crate::config::GcConfig::default(), true)
    }
}

impl GcOptions {
    /// Build run options from the configured gc policy. `dry_run` is a
    /// per-invocation flag and deliberately not part of the configuration:
    /// manual runs stay dry by default, the daemon's auto gc passes `false`.
    pub fn from_config(config: &crate::config::GcConfig, dry_run: bool) -> Self {
        Self {
            retention_days: config.retention_days,
            keep_pinned: config.keep_pinned,
            sweep_orphans: config.sweep_orphans,
            vacuum: config.vacuum,
            dry_run,
            exclude_sessions: Vec::new(),
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
    /// Unreferenced asset files deleted during the orphan sweep
    pub assets_deleted: u64,
    /// Cache entries (cache.db) deleted during the cache sweep
    #[serde(default)]
    pub cache_entries_deleted: u64,
    /// Bytes reclaimed (estimated from file sizes before deletion)
    pub bytes_reclaimed: u64,
    /// Non-fatal errors encountered (gc continues past individual failures)
    pub errors: Vec<String>,
    /// Whether this was a dry run
    pub dry_run: bool,
}

/// What a swept file or directory counts as in the report.
#[derive(Debug, Clone, Copy)]
enum CountAs {
    /// A victim session's data file (`files_deleted`).
    SessionFile,
    /// A victim session's checkpoint directory (`checkpoint_dirs_deleted`).
    CheckpointDir,
    /// An orphan file or checkpoint directory (`orphan_files_deleted`).
    Orphan,
    /// An unreferenced asset file (`assets_deleted`).
    Asset,
}

/// One unit of gc work discovered during a traversal. The same actions
/// feed both outcomes — a dry run only counts, a real run deletes — so
/// the two paths can never drift apart.
enum SweepAction {
    File {
        path: PathBuf,
        count_as: CountAs,
    },
    Dir {
        path: PathBuf,
        count_as: CountAs,
    },
    /// Expired cache entries (cache.db) older than this cutoff (unix ms).
    CacheRows {
        cutoff_ms: i64,
    },
}

fn counter_mut(report: &mut GcReport, count_as: CountAs) -> &mut u64 {
    match count_as {
        CountAs::SessionFile => &mut report.files_deleted,
        CountAs::CheckpointDir => &mut report.checkpoint_dirs_deleted,
        CountAs::Orphan => &mut report.orphan_files_deleted,
        CountAs::Asset => &mut report.assets_deleted,
    }
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
        let retention_days = opts.retention_days.max(1);
        let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days);

        let mut report = GcReport {
            dry_run: opts.dry_run,
            ..GcReport::default()
        };

        // ── Phase 1: find victims ────────────────────────────────────
        let mut victims = self
            .storage
            .session_store()
            .list_expired(cutoff, opts.keep_pinned)
            .await?;
        if !opts.exclude_sessions.is_empty() {
            victims.retain(|id| !opts.exclude_sessions.contains(id));
        }
        report.subagent_sessions = victims.iter().filter(|id| id.is_subagent()).count() as u64;

        if opts.dry_run {
            self.purge_files(&victims, &mut report).await;
            if opts.sweep_orphans {
                self.sweep(&victims, &mut report).await;
            }
            self.execute(
                SweepAction::CacheRows {
                    cutoff_ms: cutoff.timestamp_millis(),
                },
                &mut report,
            )
            .await;
            report.sessions = victims;
            return Ok(report);
        }

        // ── Phase 2+3: delete DB rows then files ─────────────────────
        self.purge_into(&victims, &mut report).await?;

        // ── Phase 4: orphan sweep ────────────────────────────────────
        if opts.sweep_orphans {
            self.sweep(&[], &mut report).await;
        }

        // ── Phase 4.5: cache sweep ───────────────────────────────────
        self.execute(
            SweepAction::CacheRows {
                cutoff_ms: cutoff.timestamp_millis(),
            },
            &mut report,
        )
        .await;

        // ── Phase 5: vacuum ──────────────────────────────────────────
        if opts.vacuum {
            if let Err(e) = self.vacuum().await {
                report.errors.push(format!("vacuum: {e}"));
            }
            if let Some(kv) = self.storage.kv_cache() {
                if let Err(e) = kv.vacuum().await {
                    report.errors.push(format!("cache vacuum: {e}"));
                }
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
        self.purge_db_rows(victims, report).await?;
        self.purge_files(victims, report).await;
        Ok(())
    }

    /// Delete DB rows (before files) for `victims`: a crash mid-way leaves
    /// orphan files (recoverable by the orphan sweep) rather than dangling
    /// DB rows. Real runs only — dry runs never reach here.
    async fn purge_db_rows(&self, victims: &[SessionId], report: &mut GcReport) -> Result<()> {
        if victims.is_empty() {
            return Ok(());
        }
        self.storage.session_store().delete_batch(victims).await?;
        report.channel_mappings_deleted = self
            .storage
            .channel_store()
            .delete_by_sessions(victims)
            .await?;
        Ok(())
    }

    /// Per-victim session files and checkpoint directories, as actions.
    async fn purge_files(&self, victims: &[SessionId], report: &mut GcReport) {
        for id in victims {
            for path in self.session_files(id.as_str()) {
                self.execute(
                    SweepAction::File {
                        path,
                        count_as: CountAs::SessionFile,
                    },
                    report,
                )
                .await;
            }
            let cp_dir = self.checkpoints_dir().join(id.as_str());
            if cp_dir.is_dir() {
                self.execute(
                    SweepAction::Dir {
                        path: cp_dir,
                        count_as: CountAs::CheckpointDir,
                    },
                    report,
                )
                .await;
            }
        }
    }

    /// Per-session data files (messages, todos, goals, file states)
    fn session_files(&self, id: &str) -> Vec<PathBuf> {
        let sessions_dir = self.sessions_dir();
        SESSION_FILE_KINDS
            .iter()
            .map(|(sub, ext)| sessions_dir.join(sub).join(format!("{id}.{ext}")))
            .collect()
    }

    fn sessions_dir(&self) -> PathBuf {
        self.storage.data_dir().join("sessions")
    }

    fn checkpoints_dir(&self) -> PathBuf {
        self.storage.data_dir().join("checkpoints")
    }

    /// Orphan file sweep + unreferenced asset sweep, sharing one snapshot of
    /// the live session set. `excluded_sessions` are live sessions whose
    /// message histories must not count as asset references (dry-run victims:
    /// they would be deleted, so their references must not protect assets).
    async fn sweep(&self, excluded_sessions: &[SessionId], report: &mut GcReport) {
        let live = match self.live_session_ids().await {
            Ok(set) => set,
            Err(e) => {
                report
                    .errors
                    .push(format!("orphan/asset sweep skipped: {e}"));
                return;
            }
        };
        self.sweep_orphans(&live, report).await;
        self.sweep_assets(excluded_sessions, &live, report).await;
    }

    /// Sweep files/directories whose session no longer exists in the DB.
    /// Also removes stale `.tmp` files (atomic-write leftovers).
    async fn sweep_orphans(&self, live: &HashSet<String>, report: &mut GcReport) {
        let sessions_dir = self.sessions_dir();
        // (dir, extension) pairs holding per-session files
        let file_dirs: Vec<(PathBuf, &str)> = SESSION_FILE_KINDS
            .iter()
            .map(|(sub, ext)| (sessions_dir.join(sub), *ext))
            .collect();

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
                        self.orphan_file(path, report).await;
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
                    self.orphan_file(path, report).await;
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
                self.execute(
                    SweepAction::Dir {
                        path,
                        count_as: CountAs::Orphan,
                    },
                    report,
                )
                .await;
            }
        }
    }

    async fn orphan_file(&self, path: PathBuf, report: &mut GcReport) {
        self.execute(
            SweepAction::File {
                path,
                count_as: CountAs::Orphan,
            },
            report,
        )
        .await;
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

    async fn sweep_assets(
        &self,
        excluded_sessions: &[SessionId],
        live: &HashSet<String>,
        report: &mut GcReport,
    ) {
        let assets_dir = self.storage.data_dir().join("assets");
        if !assets_dir.is_dir() {
            return;
        }

        let excluded: HashSet<&str> = excluded_sessions.iter().map(SessionId::as_str).collect();
        let message_paths: Vec<_> = live
            .iter()
            .filter(|id| !excluded.contains(id.as_str()))
            .map(|id| self.sessions_dir().join(format!("{id}.jsonl")))
            .collect();

        let referenced =
            match tokio::task::spawn_blocking(move || collect_asset_refs(&message_paths)).await {
                Ok(Ok(referenced)) => referenced,
                Ok(Err(e)) => {
                    report.errors.push(format!("asset sweep skipped: {e}"));
                    return;
                }
                Err(e) => {
                    report
                        .errors
                        .push(format!("asset sweep skipped: scanner failed: {e}"));
                    return;
                }
            };

        let Ok(mut entries) = tokio::fs::read_dir(&assets_dir).await else {
            report.errors.push(format!(
                "asset sweep skipped: cannot read {}",
                assets_dir.display()
            ));
            return;
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => {
                    report
                        .errors
                        .push(format!("asset sweep stopped while reading directory: {e}"));
                    break;
                }
            };
            let path = entry.path();
            if !path.is_file() || !is_stale(&path, ASSET_STALE_SECS).await {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if referenced.contains(name) {
                continue;
            }

            self.execute(
                SweepAction::File {
                    path,
                    count_as: CountAs::Asset,
                },
                report,
            )
            .await;
        }
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

    /// Perform one swept action, folding the outcome into `report`: a dry
    /// run (`report.dry_run`) only counts and estimates bytes, a real run
    /// deletes. This is the single place where the two modes diverge, so
    /// their counting and deletion logic cannot drift apart. Failures are
    /// non-fatal — they land in `report.errors`.
    async fn execute(&self, action: SweepAction, report: &mut GcReport) {
        match action {
            SweepAction::File { path, count_as } => {
                if report.dry_run {
                    if let Ok(meta) = tokio::fs::metadata(&path).await {
                        *counter_mut(report, count_as) += 1;
                        report.bytes_reclaimed += meta.len();
                    }
                    return;
                }
                // Assets are rechecked at delete time: one may have been
                // reused since the directory scan.
                if matches!(count_as, CountAs::Asset) && !is_stale(&path, ASSET_STALE_SECS).await {
                    return;
                }
                match remove_file_sized(&path).await {
                    Ok(Some(size)) => {
                        *counter_mut(report, count_as) += 1;
                        report.bytes_reclaimed += size;
                    }
                    Ok(None) => {}
                    Err(e) => report.errors.push(format!("{}: {e}", path.display())),
                }
            }
            SweepAction::Dir { path, count_as } => {
                let size = dir_size(&path).await;
                if report.dry_run {
                    *counter_mut(report, count_as) += 1;
                    report.bytes_reclaimed += size;
                    return;
                }
                match tokio::fs::remove_dir_all(&path).await {
                    Ok(()) => {
                        *counter_mut(report, count_as) += 1;
                        report.bytes_reclaimed += size;
                    }
                    Err(e) => report.errors.push(format!("{}: {e}", path.display())),
                }
            }
            SweepAction::CacheRows { cutoff_ms } => {
                let Some(kv) = self.storage.kv_cache() else {
                    return;
                };
                let res = if report.dry_run {
                    kv.count_older_than(cutoff_ms).await
                } else {
                    kv.sweep_older_than(cutoff_ms).await
                };
                match res {
                    Ok(n) => report.cache_entries_deleted += n,
                    Err(e) => report.errors.push(format!("cache sweep: {e}")),
                }
            }
        }
    }
}

fn collect_asset_refs(paths: &[PathBuf]) -> std::io::Result<HashSet<String>> {
    let mut referenced = HashSet::new();
    for path in paths {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("cannot read {}: {e}", path.display()),
                ));
            }
        };
        for (line_number, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| {
                std::io::Error::new(e.kind(), format!("cannot read {}: {e}", path.display()))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(&line).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "cannot parse {} line {}: {e}",
                        path.display(),
                        line_number + 1
                    ),
                )
            })?;
            collect_asset_refs_from_value(&value, &mut referenced);
        }
    }
    Ok(referenced)
}

fn collect_asset_refs_from_value(value: &serde_json::Value, referenced: &mut HashSet<String>) {
    match value {
        serde_json::Value::String(value) => {
            if let Some(name) = value.strip_prefix("asset://") {
                if !name.is_empty()
                    && Path::new(name).file_name().and_then(|part| part.to_str()) == Some(name)
                {
                    referenced.insert(name.to_string());
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_asset_refs_from_value(value, referenced);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_asset_refs_from_value(value, referenced);
            }
        }
        _ => {}
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

use crate::task::types::{CreateTaskInput, Task, TaskStatus};
use anyhow::Result;
use fs4::tokio::AsyncFileExt;
use serde_json;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

const HIGH_WATER_MARK_FILE: &str = ".highwatermark";
const LOCK_FILE: &str = ".lock";

pub struct TaskStorage {
    base_dir: PathBuf,
}

impl TaskStorage {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn tasks_dir(&self, task_list_id: &str) -> PathBuf {
        self.base_dir.join(sanitize_id(task_list_id))
    }

    fn task_path(&self, task_list_id: &str, task_id: &str) -> PathBuf {
        self.tasks_dir(task_list_id)
            .join(format!("{}.json", sanitize_id(task_id)))
    }

    fn lock_path(&self, task_list_id: &str) -> PathBuf {
        self.tasks_dir(task_list_id).join(LOCK_FILE)
    }

    fn high_water_mark_path(&self, task_list_id: &str) -> PathBuf {
        self.tasks_dir(task_list_id).join(HIGH_WATER_MARK_FILE)
    }

    async fn ensure_dir(&self, task_list_id: &str) -> Result<PathBuf> {
        let dir = self.tasks_dir(task_list_id);
        fs::create_dir_all(&dir).await?;
        Ok(dir)
    }

    async fn acquire_lock(&self, task_list_id: &str) -> Result<LockGuard> {
        self.ensure_dir(task_list_id).await?;
        let lock_file = self.lock_path(task_list_id);

        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_file)
            .await?;

        file.lock_exclusive()?;

        Ok(LockGuard { _file: file })
    }

    async fn read_high_water_mark(&self, task_list_id: &str) -> u64 {
        let path = self.high_water_mark_path(task_list_id);
        fs::read_to_string(&path)
            .await
            .map_or(0, |content| content.trim().parse().unwrap_or(0))
    }

    async fn write_high_water_mark(&self, task_list_id: &str, value: u64) -> Result<()> {
        let path = self.high_water_mark_path(task_list_id);
        let mut file = fs::File::create(&path).await?;
        file.write_all(value.to_string().as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }

    async fn find_highest_id(&self, task_list_id: &str) -> u64 {
        let dir = self.tasks_dir(task_list_id);
        let Ok(mut entries) = fs::read_dir(&dir).await else {
            return 0;
        };

        let mut highest = 0u64;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".json") && !name.starts_with('.') {
                if let Some(id_str) = name.strip_suffix(".json") {
                    if let Ok(id) = id_str.parse::<u64>() {
                        highest = highest.max(id);
                    }
                }
            }
        }
        highest
    }

    async fn next_id(&self, task_list_id: &str) -> Result<String> {
        let from_files = self.find_highest_id(task_list_id).await;
        let from_mark = self.read_high_water_mark(task_list_id).await;
        let next = from_files.max(from_mark) + 1;
        self.write_high_water_mark(task_list_id, next).await?;
        Ok(next.to_string())
    }

    pub async fn create_task(&self, task_list_id: &str, input: CreateTaskInput) -> Result<Task> {
        let _lock = self.acquire_lock(task_list_id).await?;

        let id = self.next_id(task_list_id).await?;
        let now = chrono::Utc::now();

        let task = Task {
            id: id.clone(),
            subject: input.subject,
            description: input.description,
            active_form: input.active_form,
            owner: None,
            status: TaskStatus::Pending,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: input.metadata,
            created_at: now,
            updated_at: now,
        };

        self.write_task_file(task_list_id, &task).await?;
        Ok(task)
    }

    pub async fn get_task(&self, task_list_id: &str, task_id: &str) -> Result<Option<Task>> {
        let path = self.task_path(task_list_id, task_id);

        match fs::read_to_string(&path).await {
            Ok(content) => match serde_json::from_str::<Task>(&content) {
                Ok(task) => Ok(Some(task)),
                Err(e) => {
                    tracing::warn!("Failed to parse task {}: {}", task_id, e);
                    Ok(None)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn update_task(
        &self,
        task_list_id: &str,
        task_id: &str,
        updates: TaskUpdates,
    ) -> Result<Option<Task>> {
        let _lock = self.acquire_lock(task_list_id).await?;
        self.update_task_inner(task_list_id, task_id, updates).await
    }

    /// Internal update logic without lock
    async fn update_task_inner(
        &self,
        task_list_id: &str,
        task_id: &str,
        updates: TaskUpdates,
    ) -> Result<Option<Task>> {
        let Some(existing) = self.get_task(task_list_id, task_id).await? else {
            return Ok(None);
        };

        let mut updated = existing;

        if let Some(subject) = updates.subject {
            updated.subject = subject;
        }
        if let Some(description) = updates.description {
            updated.description = description;
        }
        if let Some(active_form) = updates.active_form {
            updated.active_form = Some(active_form);
        }
        if let Some(status) = updates.status {
            updated.status = status;
        }
        if let Some(owner) = updates.owner {
            updated.owner = Some(owner);
        }
        if let Some(metadata) = updates.metadata {
            let mut merged = updated.metadata.unwrap_or_default();
            for (key, value) in metadata {
                if value.is_null() {
                    merged.remove(&key);
                } else {
                    merged.insert(key, value);
                }
            }
            updated.metadata = Some(merged);
        }
        if let Some(blocks) = updates.blocks {
            updated.blocks = blocks;
        }
        if let Some(blocked_by) = updates.blocked_by {
            updated.blocked_by = blocked_by;
        }

        updated.updated_at = chrono::Utc::now();

        self.write_task_file(task_list_id, &updated).await?;
        Ok(Some(updated))
    }

    pub async fn delete_task(&self, task_list_id: &str, task_id: &str) -> Result<bool> {
        let _lock = self.acquire_lock(task_list_id).await?;

        if let Ok(id_num) = task_id.parse::<u64>() {
            let current_mark = self.read_high_water_mark(task_list_id).await;
            if id_num > current_mark {
                let _ = self.write_high_water_mark(task_list_id, id_num).await;
            }
        }

        let path = self.task_path(task_list_id, task_id);

        match fs::remove_file(&path).await {
            Ok(()) => {
                // Clean up references without acquiring lock again
                // Note: list_tasks doesn't acquire lock, so it's safe to call here
                let all_tasks = self.list_tasks(task_list_id).await?;
                for task in all_tasks {
                    let new_blocks: Vec<_> = task
                        .blocks
                        .iter()
                        .filter(|id| *id != task_id)
                        .cloned()
                        .collect();
                    let new_blocked_by: Vec<_> = task
                        .blocked_by
                        .iter()
                        .filter(|id| *id != task_id)
                        .cloned()
                        .collect();

                    if new_blocks.len() != task.blocks.len()
                        || new_blocked_by.len() != task.blocked_by.len()
                    {
                        let _ = self
                            .update_task_inner(
                                task_list_id,
                                &task.id,
                                TaskUpdates {
                                    blocks: Some(new_blocks),
                                    blocked_by: Some(new_blocked_by),
                                    ..Default::default()
                                },
                            )
                            .await;
                    }
                }
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn list_tasks(&self, task_list_id: &str) -> Result<Vec<Task>> {
        let dir = self.tasks_dir(task_list_id);
        let mut tasks = Vec::new();

        let Ok(mut entries) = fs::read_dir(&dir).await else {
            return Ok(tasks);
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".json") && !name.starts_with('.') {
                if let Some(id) = name.strip_suffix(".json") {
                    if let Ok(Some(task)) = self.get_task(task_list_id, id).await {
                        tasks.push(task);
                    }
                }
            }
        }

        tasks.sort_by(|a, b| {
            let a_num = a.id.parse::<u64>().unwrap_or(0);
            let b_num = b.id.parse::<u64>().unwrap_or(0);
            a_num.cmp(&b_num)
        });

        Ok(tasks)
    }

    pub async fn reset_tasks(&self, task_list_id: &str) -> Result<()> {
        let _lock = self.acquire_lock(task_list_id).await?;

        let current_highest = self.find_highest_id(task_list_id).await;
        if current_highest > 0 {
            let existing_mark = self.read_high_water_mark(task_list_id).await;
            if current_highest > existing_mark {
                self.write_high_water_mark(task_list_id, current_highest)
                    .await?;
            }
        }

        let dir = self.tasks_dir(task_list_id);
        let Ok(mut entries) = fs::read_dir(&dir).await else {
            return Ok(());
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".json") && !name.starts_with('.') {
                let _ = fs::remove_file(entry.path()).await;
            }
        }

        Ok(())
    }

    async fn write_task_file(&self, task_list_id: &str, task: &Task) -> Result<()> {
        let path = self.task_path(task_list_id, &task.id);
        let temp_path = path.with_extension("tmp");

        let content = serde_json::to_string_pretty(task)?;
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(content.as_bytes()).await?;
        file.flush().await?;
        drop(file);

        fs::rename(&temp_path, &path).await?;
        Ok(())
    }
}

struct LockGuard {
    _file: tokio::fs::File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {}
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct TaskUpdates {
    pub subject: Option<String>,
    pub description: Option<String>,
    pub active_form: Option<String>,
    pub status: Option<TaskStatus>,
    pub owner: Option<String>,
    pub blocks: Option<Vec<String>>,
    pub blocked_by: Option<Vec<String>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

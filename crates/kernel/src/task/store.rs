use crate::task::storage::{TaskStorage, TaskUpdates};
use crate::task::types::*;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, Clone)]
pub enum TaskEvent {
    Created { task_list_id: String, task: Task },
    Updated { task_list_id: String, task: Task, updated_fields: Vec<String> },
    Deleted { task_list_id: String, task_id: String },
    Reset { task_list_id: String },
}

pub struct TaskStore {
    storage: TaskStorage,
    event_tx: broadcast::Sender<TaskEvent>,
    cache: RwLock<HashMap<String, Vec<Task>>>,
}

impl TaskStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let base_dir = data_dir.into().join("tasks");
        Self::with_storage(TaskStorage::new(base_dir))
    }

    pub fn with_storage(storage: TaskStorage) -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            storage,
            event_tx,
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.event_tx.subscribe()
    }

    pub async fn create_task(&self, task_list_id: &str, input: CreateTaskInput) -> Result<Task> {
        let task = self.storage.create_task(task_list_id, input).await?;

        let mut cache = self.cache.write().await;
        let tasks = cache.entry(task_list_id.to_string()).or_default();
        tasks.push(task.clone());

        let _ = self.event_tx.send(TaskEvent::Created {
            task_list_id: task_list_id.to_string(),
            task: task.clone(),
        });

        Ok(task)
    }

    pub async fn get_task(&self, task_list_id: &str, task_id: &str) -> Result<Option<Task>> {
        let cache = self.cache.read().await;
        if let Some(tasks) = cache.get(task_list_id) {
            if let Some(task) = tasks.iter().find(|t| t.id == task_id) {
                return Ok(Some(task.clone()));
            }
        }
        drop(cache);

        self.storage.get_task(task_list_id, task_id).await
    }

    pub async fn update_task(&self, task_list_id: &str, task_id: &str, updates: TaskUpdates) -> Result<Option<(Task, Vec<String>)>> {
        let existing = self.storage.get_task(task_list_id, task_id).await?;
        let existing = match existing {
            Some(t) => t,
            None => return Ok(None),
        };

        let mut updated_fields = Vec::new();
        if updates.subject.is_some() && updates.subject.as_ref() != Some(&existing.subject) {
            updated_fields.push("subject".to_string());
        }
        if updates.description.is_some() && updates.description.as_ref() != Some(&existing.description) {
            updated_fields.push("description".to_string());
        }
        if updates.active_form.is_some() && updates.active_form.as_ref() != existing.active_form.as_ref() {
            updated_fields.push("active_form".to_string());
        }
        if updates.status.is_some() && updates.status.as_ref() != Some(&existing.status) {
            updated_fields.push("status".to_string());
        }
        if updates.owner.is_some() && updates.owner.as_ref() != existing.owner.as_ref() {
            updated_fields.push("owner".to_string());
        }
        if updates.blocks.is_some() && updates.blocks.as_ref() != Some(&existing.blocks) {
            updated_fields.push("blocks".to_string());
        }
        if updates.blocked_by.is_some() && updates.blocked_by.as_ref() != Some(&existing.blocked_by) {
            updated_fields.push("blocked_by".to_string());
        }
        if updates.metadata.is_some() {
            updated_fields.push("metadata".to_string());
        }

        if updated_fields.is_empty() {
            return Ok(Some((existing, Vec::new())));
        }

        let task = self.storage.update_task(task_list_id, task_id, updates).await?;

        if let Some(ref task) = task {
            let mut cache = self.cache.write().await;
            if let Some(tasks) = cache.get_mut(task_list_id) {
                if let Some(idx) = tasks.iter().position(|t| t.id == task_id) {
                    tasks[idx] = task.clone();
                }
            }

            let _ = self.event_tx.send(TaskEvent::Updated {
                task_list_id: task_list_id.to_string(),
                task: task.clone(),
                updated_fields: updated_fields.clone(),
            });
        }

        Ok(task.map(|t| (t, updated_fields)))
    }

    pub async fn delete_task(&self, task_list_id: &str, task_id: &str) -> Result<bool> {
        let deleted = self.storage.delete_task(task_list_id, task_id).await?;

        if deleted {
            let mut cache = self.cache.write().await;
            if let Some(tasks) = cache.get_mut(task_list_id) {
                tasks.retain(|t| t.id != task_id);
            }

            let _ = self.event_tx.send(TaskEvent::Deleted {
                task_list_id: task_list_id.to_string(),
                task_id: task_id.to_string(),
            });
        }

        Ok(deleted)
    }

    pub async fn list_tasks(&self, task_list_id: &str) -> Result<Vec<Task>> {
        let cache = self.cache.read().await;
        if let Some(tasks) = cache.get(task_list_id) {
            return Ok(tasks.clone());
        }
        drop(cache);

        let tasks = self.storage.list_tasks(task_list_id).await?;

        let mut cache = self.cache.write().await;
        cache.insert(task_list_id.to_string(), tasks.clone());

        Ok(tasks)
    }

    pub async fn block_task(&self, task_list_id: &str, from_task_id: &str, to_task_id: &str) -> Result<bool> {
        let (from_task, to_task) = tokio::try_join!(
            self.storage.get_task(task_list_id, from_task_id),
            self.storage.get_task(task_list_id, to_task_id),
        )?;

        if from_task.is_none() || to_task.is_none() {
            return Ok(false);
        }

        let from_task = from_task.unwrap();
        let to_task = to_task.unwrap();

        if !from_task.blocks.contains(&to_task_id.to_string()) {
            let mut blocks = from_task.blocks.clone();
            blocks.push(to_task_id.to_string());
            self.storage.update_task(task_list_id, from_task_id, TaskUpdates {
                blocks: Some(blocks),
                ..Default::default()
            }).await?;
        }

        if !to_task.blocked_by.contains(&from_task_id.to_string()) {
            let mut blocked_by = to_task.blocked_by.clone();
            blocked_by.push(from_task_id.to_string());
            self.storage.update_task(task_list_id, to_task_id, TaskUpdates {
                blocked_by: Some(blocked_by),
                ..Default::default()
            }).await?;
        }

        Ok(true)
    }
}

pub type SharedTaskStore = Arc<TaskStore>;

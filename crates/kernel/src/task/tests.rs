#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::task::store::TaskStore;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_task_storage_create_and_get() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TaskStorage::new(temp_dir.path());

        // Create a task
        let input = CreateTaskInput {
            subject: "Test task".to_string(),
            description: "Test description".to_string(),
            active_form: Some("Testing".to_string()),
            metadata: None,
        };

        let task = storage.create_task("session1", input).await.unwrap();
        assert_eq!(task.id, "1");
        assert_eq!(task.subject, "Test task");
        assert_eq!(task.status, TaskStatus::Pending);

        // Get the task
        let retrieved = storage.get_task("session1", "1").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, "1");
        assert_eq!(retrieved.subject, "Test task");
    }

    #[tokio::test]
    async fn test_task_storage_list() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TaskStorage::new(temp_dir.path());

        // Create multiple tasks
        for i in 1..=3 {
            let input = CreateTaskInput {
                subject: format!("Task {}", i),
                description: format!("Description {}", i),
                active_form: None,
                metadata: None,
            };
            storage.create_task("session1", input).await.unwrap();
        }

        // List tasks
        let tasks = storage.list_tasks("session1").await.unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].id, "1");
        assert_eq!(tasks[1].id, "2");
        assert_eq!(tasks[2].id, "3");
    }

    #[tokio::test]
    async fn test_task_storage_update() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TaskStorage::new(temp_dir.path());

        // Create a task
        let input = CreateTaskInput {
            subject: "Original subject".to_string(),
            description: "Original description".to_string(),
            active_form: None,
            metadata: None,
        };
        storage.create_task("session1", input).await.unwrap();

        // Update the task
        let updates = TaskUpdates {
            subject: Some("Updated subject".to_string()),
            status: Some(TaskStatus::InProgress),
            ..Default::default()
        };
        let updated = storage.update_task("session1", "1", updates).await.unwrap();
        assert!(updated.is_some());
        let updated = updated.unwrap();
        assert_eq!(updated.subject, "Updated subject");
        assert!(matches!(updated.status, TaskStatus::InProgress));
    }

    #[tokio::test]
    async fn test_task_storage_delete() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TaskStorage::new(temp_dir.path());

        // Create and delete a task
        let input = CreateTaskInput {
            subject: "Task to delete".to_string(),
            description: "Will be deleted".to_string(),
            active_form: None,
            metadata: None,
        };
        storage.create_task("session1", input).await.unwrap();

        let deleted = storage.delete_task("session1", "1").await.unwrap();
        assert!(deleted);

        let retrieved = storage.get_task("session1", "1").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_task_storage_blocks() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TaskStorage::new(temp_dir.path());

        // Create two tasks
        let input1 = CreateTaskInput {
            subject: "Task 1".to_string(),
            description: "First task".to_string(),
            active_form: None,
            metadata: None,
        };
        storage.create_task("session1", input1).await.unwrap();

        let input2 = CreateTaskInput {
            subject: "Task 2".to_string(),
            description: "Second task".to_string(),
            active_form: None,
            metadata: None,
        };
        storage.create_task("session1", input2).await.unwrap();

        // Set up blocking relationship using TaskStore
        let store = TaskStore::with_storage(storage);
        store.block_task("session1", "1", "2").await.unwrap();

        // Verify the relationship
        let task1 = store.get_task("session1", "1").await.unwrap().unwrap();
        let task2 = store.get_task("session1", "2").await.unwrap().unwrap();

        assert!(task1.blocks.contains(&"2".to_string()));
        assert!(task2.blocked_by.contains(&"1".to_string()));
    }

    #[tokio::test]
    async fn test_task_store_events() {
        let temp_dir = TempDir::new().unwrap();
        let store = TaskStore::new(temp_dir.path());
        let mut rx = store.subscribe();

        // Create a task
        let input = CreateTaskInput {
            subject: "Event test".to_string(),
            description: "Testing events".to_string(),
            active_form: None,
            metadata: None,
        };
        store.create_task("session1", input).await.unwrap();

        // Check event was received
        let event = rx.try_recv();
        assert!(event.is_ok());
        match event.unwrap() {
            TaskEvent::Created { task_list_id, task } => {
                assert_eq!(task_list_id, "session1");
                assert_eq!(task.subject, "Event test");
            }
            _ => panic!("Expected Created event"),
        }
    }
}

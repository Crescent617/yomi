#[cfg(test)]
mod task_tests {
    use super::super::*;

    async fn create_test_storage() -> SqliteTaskStorage {
        // Use in-memory SQLite for tests
        SqliteTaskStorage::new(":memory:").await.unwrap()
    }

    #[tokio::test]
    async fn test_task_storage_create_and_get() {
        let storage = create_test_storage().await;

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
        let storage = create_test_storage().await;

        // Create multiple tasks
        for i in 1..=3 {
            let input = CreateTaskInput {
                subject: format!("Task {i}"),
                description: format!("Description {i}"),
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
        let storage = create_test_storage().await;

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
        let storage = create_test_storage().await;

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
        let storage = create_test_storage().await;

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

        // Set up blocking relationship using storage directly
        storage
            .update_task(
                "session1",
                "1",
                TaskUpdates {
                    blocks: Some(vec!["2".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        storage
            .update_task(
                "session1",
                "2",
                TaskUpdates {
                    blocked_by: Some(vec!["1".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Verify the relationship
        let task1 = storage.get_task("session1", "1").await.unwrap().unwrap();
        let task2 = storage.get_task("session1", "2").await.unwrap().unwrap();

        assert!(task1.blocks.contains(&"2".to_string()));
        assert!(task2.blocked_by.contains(&"1".to_string()));
    }

    #[tokio::test]
    async fn test_task_store_events() {
        use crate::task::store::TaskStore;

        let storage = SqliteTaskStorage::new(":memory:").await.unwrap();
        let store = TaskStore::with_storage(storage);
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

    #[tokio::test]
    async fn test_per_session_auto_increment() {
        let storage = create_test_storage().await;

        // Create tasks in session1
        let input = CreateTaskInput {
            subject: "Task 1".to_string(),
            description: "First task in session1".to_string(),
            active_form: None,
            metadata: None,
        };
        let task1 = storage.create_task("session1", input).await.unwrap();
        assert_eq!(task1.id, "1");

        let input = CreateTaskInput {
            subject: "Task 2".to_string(),
            description: "Second task in session1".to_string(),
            active_form: None,
            metadata: None,
        };
        let task2 = storage.create_task("session1", input).await.unwrap();
        assert_eq!(task2.id, "2");

        // Create tasks in session2 - should start from 1
        let input = CreateTaskInput {
            subject: "Task 1 in session2".to_string(),
            description: "First task in session2".to_string(),
            active_form: None,
            metadata: None,
        };
        let task3 = storage.create_task("session2", input).await.unwrap();
        assert_eq!(task3.id, "1"); // Should be 1, not 3

        let input = CreateTaskInput {
            subject: "Task 2 in session2".to_string(),
            description: "Second task in session2".to_string(),
            active_form: None,
            metadata: None,
        };
        let task4 = storage.create_task("session2", input).await.unwrap();
        assert_eq!(task4.id, "2"); // Should be 2

        // Continue session1 - should continue from 2
        let input = CreateTaskInput {
            subject: "Task 3".to_string(),
            description: "Third task in session1".to_string(),
            active_form: None,
            metadata: None,
        };
        let task5 = storage.create_task("session1", input).await.unwrap();
        assert_eq!(task5.id, "3"); // Should be 3, continuing session1's sequence
    }

    #[tokio::test]
    async fn test_delete_cascades_dependencies() {
        let storage = create_test_storage().await;

        // Create three tasks: 1 blocks 2, 2 blocks 3
        for i in 1..=3 {
            let input = CreateTaskInput {
                subject: format!("Task {i}"),
                description: format!("Description {i}"),
                active_form: None,
                metadata: None,
            };
            storage.create_task("session1", input).await.unwrap();
        }

        // Set up: 1 blocks 2, 2 blocks 3
        storage
            .update_task(
                "session1",
                "2",
                TaskUpdates {
                    blocked_by: Some(vec!["1".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        storage
            .update_task(
                "session1",
                "3",
                TaskUpdates {
                    blocked_by: Some(vec!["2".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Verify relationships
        let task1 = storage.get_task("session1", "1").await.unwrap().unwrap();
        let task2 = storage.get_task("session1", "2").await.unwrap().unwrap();
        let task3 = storage.get_task("session1", "3").await.unwrap().unwrap();

        assert!(task1.blocks.contains(&"2".to_string()));
        assert!(task2.blocked_by.contains(&"1".to_string()));
        assert!(task2.blocks.contains(&"3".to_string()));
        assert!(task3.blocked_by.contains(&"2".to_string()));

        // Delete task 2 - should cascade and remove its dependencies
        storage.delete_task("session1", "2").await.unwrap();

        // Verify task 2 is gone
        assert!(storage.get_task("session1", "2").await.unwrap().is_none());

        // Verify task 1's blocks no longer contains 2
        let task1 = storage.get_task("session1", "1").await.unwrap().unwrap();
        assert!(!task1.blocks.contains(&"2".to_string()));

        // Verify task 3's blocked_by no longer contains 2
        let task3 = storage.get_task("session1", "3").await.unwrap().unwrap();
        assert!(!task3.blocked_by.contains(&"2".to_string()));
    }

    #[tokio::test]
    async fn test_atomic_update() {
        let storage = create_test_storage().await;

        // Create a task
        let input = CreateTaskInput {
            subject: "Test task".to_string(),
            description: "Test description".to_string(),
            active_form: None,
            metadata: None,
        };
        storage.create_task("session1", input).await.unwrap();

        // Update all fields at once
        let updates = TaskUpdates {
            subject: Some("New subject".to_string()),
            description: Some("New description".to_string()),
            status: Some(TaskStatus::InProgress),
            owner: Some("test-owner".to_string()),
            active_form: Some("Testing task".to_string()),
            metadata: Some({
                let mut m = std::collections::HashMap::new();
                m.insert("key".to_string(), serde_json::json!("value"));
                m
            }),
            ..Default::default()
        };

        let updated = storage
            .update_task("session1", "1", updates)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(updated.subject, "New subject");
        assert_eq!(updated.description, "New description");
        assert!(matches!(updated.status, TaskStatus::InProgress));
        assert_eq!(updated.owner, Some("test-owner".to_string()));
        assert_eq!(updated.active_form, Some("Testing task".to_string()));
    }

    #[tokio::test]
    async fn test_reset_tasks_clears_sequence() {
        let storage = create_test_storage().await;

        // Create tasks
        for i in 1..=3 {
            let input = CreateTaskInput {
                subject: format!("Task {i}"),
                description: format!("Description {i}"),
                active_form: None,
                metadata: None,
            };
            storage.create_task("session1", input).await.unwrap();
        }

        // Reset tasks
        storage.reset_tasks("session1").await.unwrap();

        // Create new task - should start from 1 again
        let input = CreateTaskInput {
            subject: "New task".to_string(),
            description: "After reset".to_string(),
            active_form: None,
            metadata: None,
        };
        let task = storage.create_task("session1", input).await.unwrap();
        assert_eq!(task.id, "1");
    }

    #[tokio::test]
    async fn test_update_nonexistent_task() {
        let storage = create_test_storage().await;

        let updates = TaskUpdates {
            subject: Some("New subject".to_string()),
            ..Default::default()
        };

        let result = storage
            .update_task("session1", "999", updates)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_update_blocks_and_blocked_by() {
        let storage = create_test_storage().await;

        // Create tasks
        for i in 1..=3 {
            let input = CreateTaskInput {
                subject: format!("Task {i}"),
                description: format!("Description {i}"),
                active_form: None,
                metadata: None,
            };
            storage.create_task("session1", input).await.unwrap();
        }

        // Task 1 blocks task 2 (using blocks field)
        storage
            .update_task(
                "session1",
                "1",
                TaskUpdates {
                    blocks: Some(vec!["2".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Verify from both sides
        let task1 = storage.get_task("session1", "1").await.unwrap().unwrap();
        let task2 = storage.get_task("session1", "2").await.unwrap().unwrap();

        assert!(task1.blocks.contains(&"2".to_string()));
        assert!(task2.blocked_by.contains(&"1".to_string()));

        // Now update task 3 to be blocked by task 2 (using blocked_by field)
        storage
            .update_task(
                "session1",
                "3",
                TaskUpdates {
                    blocked_by: Some(vec!["2".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let task2 = storage.get_task("session1", "2").await.unwrap().unwrap();
        let task3 = storage.get_task("session1", "3").await.unwrap().unwrap();

        assert!(task2.blocks.contains(&"3".to_string()));
        assert!(task3.blocked_by.contains(&"2".to_string()));
    }

    #[tokio::test]
    async fn test_cache_consistency_after_dependency_update() {
        use crate::task::store::TaskStore;

        let storage = SqliteTaskStorage::new(":memory:").await.unwrap();
        let store = TaskStore::with_storage(storage);

        // Create tasks
        for i in 1..=3 {
            let input = CreateTaskInput {
                subject: format!("Task {i}"),
                description: format!("Description {i}"),
                active_form: None,
                metadata: None,
            };
            store.create_task("session1", input).await.unwrap();
        }

        // Prime the cache by listing tasks
        let tasks = store.list_tasks("session1").await.unwrap();
        assert_eq!(tasks.len(), 3);

        // Task 1 blocks task 2 and 3
        store
            .update_task(
                "session1",
                "1",
                TaskUpdates {
                    blocks: Some(vec!["2".to_string(), "3".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Cache should be invalidated, so list_tasks should return fresh data
        let tasks = store.list_tasks("session1").await.unwrap();
        let task2 = tasks.iter().find(|t| t.id == "2").unwrap();
        let task3 = tasks.iter().find(|t| t.id == "3").unwrap();

        assert!(task2.blocked_by.contains(&"1".to_string()));
        assert!(task3.blocked_by.contains(&"1".to_string()));

        // Delete task 1
        store.delete_task("session1", "1").await.unwrap();

        // Cache should be invalidated again
        let tasks = store.list_tasks("session1").await.unwrap();
        let task2 = tasks.iter().find(|t| t.id == "2").unwrap();
        let task3 = tasks.iter().find(|t| t.id == "3").unwrap();

        assert!(!task2.blocked_by.contains(&"1".to_string()));
        assert!(!task3.blocked_by.contains(&"1".to_string()));
    }

    #[tokio::test]
    async fn test_cross_session_dependency_isolation() {
        let storage = create_test_storage().await;

        // Create tasks in different sessions with same IDs
        for i in 1..=2 {
            let input = CreateTaskInput {
                subject: format!("Task {i}"),
                description: format!("Description {i}"),
                active_form: None,
                metadata: None,
            };
            storage
                .create_task("session1", input.clone())
                .await
                .unwrap();
            storage.create_task("session2", input).await.unwrap();
        }

        // In session1: task 1 blocks task 2
        storage
            .update_task(
                "session1",
                "1",
                TaskUpdates {
                    blocks: Some(vec!["2".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // In session2: also task 1 blocks task 2 (same IDs, different session)
        storage
            .update_task(
                "session2",
                "1",
                TaskUpdates {
                    blocks: Some(vec!["2".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Verify isolation - session1
        let tasks1 = storage.list_tasks("session1").await.unwrap();
        let task1_s1 = tasks1.iter().find(|t| t.id == "1").unwrap();
        let task2_s1 = tasks1.iter().find(|t| t.id == "2").unwrap();
        assert!(task1_s1.blocks.contains(&"2".to_string()));
        assert!(task2_s1.blocked_by.contains(&"1".to_string()));

        // Verify isolation - session2
        let tasks2 = storage.list_tasks("session2").await.unwrap();
        let task1_s2 = tasks2.iter().find(|t| t.id == "1").unwrap();
        let task2_s2 = tasks2.iter().find(|t| t.id == "2").unwrap();
        assert!(task1_s2.blocks.contains(&"2".to_string()));
        assert!(task2_s2.blocked_by.contains(&"1".to_string()));

        // Delete task 1 from session1 - should not affect session2
        storage.delete_task("session1", "1").await.unwrap();

        let tasks2 = storage.list_tasks("session2").await.unwrap();
        let task1_s2 = tasks2.iter().find(|t| t.id == "1").unwrap();
        let task2_s2 = tasks2.iter().find(|t| t.id == "2").unwrap();
        assert!(task1_s2.blocks.contains(&"2".to_string())); // Still blocking
        assert!(task2_s2.blocked_by.contains(&"1".to_string())); // Still blocked
    }
}

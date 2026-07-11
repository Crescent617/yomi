use super::*;

use crate::tools::task::types::CreateTaskInput;
use crate::tools::task::{SqliteTaskStorage, TaskStore};
use std::sync::Arc;

async fn create_tool() -> (TaskUpdateTool, SharedTaskStore) {
    let storage = SqliteTaskStorage::new(":memory:").await.unwrap();
    let store = Arc::new(TaskStore::with_storage(storage));
    store
        .create_task(
            "test-list",
            CreateTaskInput {
                subject: "Original subject".to_string(),
                description: "Original description".to_string(),
                metadata: None,
            },
        )
        .await
        .unwrap();

    (
        TaskUpdateTool::new(store.clone(), "test-list".to_string()),
        store,
    )
}

#[tokio::test]
async fn rejects_blank_task_id() {
    let (tool, _) = create_tool().await;
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");

    let result = tool.exec(json!({"taskId": "   "}), ctx).await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("taskId is required and must be non-empty"));
}

#[tokio::test]
async fn ignores_blank_optional_strings_and_dependency_ids() {
    let (tool, store) = create_tool().await;
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");

    let result = tool
        .exec(
            json!({
                "taskId": "1",
                "subject": "   ",
                "description": "",
                "owner": "   ",
                "status": "",
                "addBlocks": ["", "   "],
                "addBlockedBy": ["", "   "]
            }),
            ctx,
        )
        .await
        .unwrap();

    let output: Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(output["updated_fields"], json!([]));
    assert!(output.get("status_change").is_none());

    let task = store.get_task("test-list", "1").await.unwrap().unwrap();
    assert_eq!(task.subject, "Original subject");
    assert_eq!(task.description, "Original description");
    assert!(task.owner.is_none());
    assert_eq!(task.status, TaskStatus::Pending);
    assert!(task.blocks.is_empty());
    assert!(task.blocked_by.is_empty());
}

#[tokio::test]
async fn rejects_nonempty_invalid_status() {
    let (tool, _) = create_tool().await;
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");

    let result = tool
        .exec(json!({"taskId": "1", "status": " completed "}), ctx)
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid status"));
}

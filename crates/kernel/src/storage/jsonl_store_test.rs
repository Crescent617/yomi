use super::*;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct TestRecord {
    id: u32,
    data: String,
}

fn create_test_store() -> (JsonlStore<TestRecord, u32>, TempDir) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("test.jsonl");
    let store: JsonlStore<TestRecord, u32> = JsonlStore::new(&path, |r: &TestRecord| r.id);
    (store, temp)
}

#[tokio::test]
async fn test_create_and_meta() {
    let (store, _temp) = create_test_store();

    let meta = store.meta().await.unwrap();
    assert_eq!(meta.vacuum_count, 0);
}

#[tokio::test]
async fn test_append_and_read() {
    let (store, _temp) = create_test_store();

    store
        .append(&TestRecord {
            id: 1,
            data: "hello".to_string(),
        })
        .await
        .unwrap();
    store
        .append(&TestRecord {
            id: 2,
            data: "world".to_string(),
        })
        .await
        .unwrap();

    let records = store.read_all().await.unwrap();
    assert_eq!(records.len(), 2);
}

#[tokio::test]
async fn test_read_all_deduped() {
    let (store, _temp) = create_test_store();

    store
        .append(&TestRecord {
            id: 1,
            data: "first".to_string(),
        })
        .await
        .unwrap();
    store
        .append(&TestRecord {
            id: 1,
            data: "second".to_string(),
        })
        .await
        .unwrap();
    store
        .append(&TestRecord {
            id: 2,
            data: "other".to_string(),
        })
        .await
        .unwrap();

    // read_all() returns deduplicated records by default
    let records = store.read_all().await.unwrap();
    assert_eq!(records.len(), 2);
    // Last occurrence wins
    let r1 = records.iter().find(|r| r.id == 1).unwrap();
    assert_eq!(r1.data, "second");
}

#[tokio::test]
async fn test_auto_vacuum_dedup() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("test.jsonl");
    // Set low threshold for testing
    let store = JsonlStore::<TestRecord, u32>::new_with_threshold(&path, |r: &TestRecord| r.id, 5);

    // Append 5 records with duplicate ids
    for i in 0..5 {
        store
            .append(&TestRecord {
                id: i % 3, // 0, 1, 2, 0, 1 - duplicates
                data: format!("v{i}"),
            })
            .await
            .unwrap();
    }

    // At 5 records, vacuum should trigger, leaving 3 unique
    let records = store.read_all().await.unwrap();
    assert_eq!(records.len(), 3);

    // Check vacuum was recorded
    let meta = store.meta().await.unwrap();
    assert_eq!(meta.vacuum_count, 1);
}

#[tokio::test]
async fn test_manual_vacuum() {
    let (store, _temp) = create_test_store();

    for i in 0..3 {
        store
            .append(&TestRecord {
                id: 0, // same key
                data: format!("v{i}"),
            })
            .await
            .unwrap();
    }

    // Force vacuum
    store.vacuum().await.unwrap();

    let records = store.read_all().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].data, "v2"); // last one wins
}

#[tokio::test]
async fn test_clear() {
    let (store, _temp) = create_test_store();

    store
        .append(&TestRecord {
            id: 1,
            data: "x".to_string(),
        })
        .await
        .unwrap();

    store.truncate().await.unwrap();

    let records = store.read_all().await.unwrap();
    assert!(records.is_empty());

    let meta = store.meta().await.unwrap();
    assert_eq!(meta.truncate_count, 1);
}

#[tokio::test]
async fn test_persist_across_reopen() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("persist.jsonl");

    {
        let store: JsonlStore<TestRecord, u32> = JsonlStore::new(&path, |r: &TestRecord| r.id);
        store
            .append(&TestRecord {
                id: 42,
                data: "test".to_string(),
            })
            .await
            .unwrap();
    }

    {
        let store: JsonlStore<TestRecord, u32> = JsonlStore::new(&path, |r: &TestRecord| r.id);
        let records = store.read_all().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 42);
    }
}

#[tokio::test]
async fn test_append_batch() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("batch.jsonl");
    let store: JsonlStore<TestRecord, u32> = JsonlStore::new(&path, |r: &TestRecord| r.id);

    // Append batch of 5 records with single flush
    let records: Vec<TestRecord> = (0..5)
        .map(|i| TestRecord {
            id: i,
            data: format!("batch_{i}"),
        })
        .collect();

    store.append_batch(&records).await.unwrap();

    // Verify all records persisted
    let read = store.read_all().await.unwrap();
    assert_eq!(read.len(), 5);

    // Verify data integrity (sort by id since read_all returns unordered)
    let mut sorted: Vec<_> = read.into_iter().collect();
    sorted.sort_by_key(|r| r.id);
    for (i, r) in sorted.iter().enumerate() {
        assert_eq!(r.id as usize, i);
        assert_eq!(r.data, format!("batch_{i}"));
    }
}

#[tokio::test]
async fn test_append_batch_empty() {
    let (store, _temp) = create_test_store();

    // Empty batch should be no-op
    store.append_batch(&[]).await.unwrap();

    let records = store.read_all().await.unwrap();
    assert!(records.is_empty());
}

#[tokio::test]
async fn test_append_batch_triggers_vacuum() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("batch_vacuum.jsonl");
    // Set threshold to 5
    let store = JsonlStore::<TestRecord, u32>::new_with_threshold(&path, |r: &TestRecord| r.id, 5);

    // Append batch of 5 (exactly at threshold)
    let records: Vec<TestRecord> = (0..5)
        .map(|i| TestRecord {
            id: i % 3, // duplicates: 0, 1, 2, 0, 1
            data: format!("v{i}"),
        })
        .collect();

    store.append_batch(&records).await.unwrap();

    // Vacuum should have triggered, leaving 3 unique
    let read = store.read_all().await.unwrap();
    assert_eq!(read.len(), 3);

    // Check vacuum was recorded
    let meta = store.meta().await.unwrap();
    assert_eq!(meta.vacuum_count, 1);
}

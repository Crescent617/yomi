use super::*;

use std::time::Duration;

#[tokio::test]
async fn test_lock_key_basic() {
    let key = "test_lock_basic";

    let guard = g_lock(key).await;
    drop(guard);

    // Should be able to acquire again after drop
    let _guard2 = g_lock(key).await;
}

#[tokio::test]
async fn test_lock_key_timeout() {
    let key = "test_lock_timeout";

    let _guard = g_lock(key).await;

    // Try to acquire with a very short timeout - should timeout
    let result = g_lock_timeout(key, Duration::from_millis(1)).await;
    assert!(matches!(result, Err(GLockError::Timeout)));
}

#[tokio::test]
async fn test_concurrent_locks_different_keys() {
    let key1 = "test_key_1";
    let key2 = "test_key_2";

    // Different keys should not block each other
    let guard1 = g_lock(key1).await;
    let guard2 = g_lock(key2).await;

    drop(guard1);
    drop(guard2);
}

#[tokio::test]
async fn test_lock_key_string_owned() {
    let key = "test_owned_string".to_string();

    let guard = g_lock(key.clone()).await;
    drop(guard);

    // Should be able to acquire again with the same key
    let _guard2 = g_lock(key).await;
}

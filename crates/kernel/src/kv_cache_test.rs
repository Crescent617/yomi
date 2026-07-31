use super::KvCache;

#[tokio::test]
async fn put_get_replace_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let kv = KvCache::open(&dir.path().join("cache.db")).await.unwrap();

    assert_eq!(kv.get("ns", "k1").await.unwrap(), None);
    kv.put("ns", "k1", "v1").await.unwrap();
    assert_eq!(kv.get("ns", "k1").await.unwrap().as_deref(), Some("v1"));

    // Replace (e.g. card morph) overwrites the value.
    kv.put("ns", "k1", "v2").await.unwrap();
    assert_eq!(kv.get("ns", "k1").await.unwrap().as_deref(), Some("v2"));

    // Namespaces are isolated.
    assert_eq!(kv.get("other", "k1").await.unwrap(), None);
    kv.put("other", "k1", "x").await.unwrap();
    assert_eq!(kv.get("other", "k1").await.unwrap().as_deref(), Some("x"));
    assert_eq!(kv.get("ns", "k1").await.unwrap().as_deref(), Some("v2"));
}

#[tokio::test]
async fn entries_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.db");
    KvCache::open(&path)
        .await
        .unwrap()
        .put("ns", "k", "persisted")
        .await
        .unwrap();

    let reopened = KvCache::open(&path).await.unwrap();
    assert_eq!(
        reopened.get("ns", "k").await.unwrap().as_deref(),
        Some("persisted")
    );
}

#[tokio::test]
async fn prune_older_than_deletes_only_stale_rows() {
    let dir = tempfile::tempdir().unwrap();
    let kv = KvCache::open(&dir.path().join("cache.db")).await.unwrap();
    kv.put("ns", "old", "x").await.unwrap();
    kv.put("ns", "new", "y").await.unwrap();
    kv.put("other", "old", "z").await.unwrap();

    // Everything is fresh now; pruning with a future cutoff removes "ns"
    // rows only.
    let cutoff = chrono::Utc::now().timestamp_millis() + 1000;
    assert_eq!(kv.prune_older_than("ns", cutoff).await.unwrap(), 2);
    assert_eq!(kv.get("ns", "old").await.unwrap(), None);
    assert_eq!(kv.get("ns", "new").await.unwrap(), None);
    assert_eq!(kv.get("other", "old").await.unwrap().as_deref(), Some("z"));
}

#[tokio::test]
async fn count_and_sweep_older_than_cover_all_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let kv = KvCache::open(&dir.path().join("cache.db")).await.unwrap();
    kv.put("a", "k1", "x").await.unwrap();
    kv.put("b", "k2", "y").await.unwrap();

    let future = chrono::Utc::now().timestamp_millis() + 1000;
    assert_eq!(kv.count_older_than(future).await.unwrap(), 2);
    assert_eq!(kv.count_older_than(0).await.unwrap(), 0);

    assert_eq!(kv.sweep_older_than(future).await.unwrap(), 2);
    assert_eq!(kv.get("a", "k1").await.unwrap(), None);
    assert_eq!(kv.get("b", "k2").await.unwrap(), None);
}

#[tokio::test]
async fn vacuum_keeps_db_usable_after_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let kv = KvCache::open(&dir.path().join("cache.db")).await.unwrap();
    for i in 0..50 {
        kv.put("ns", &format!("k{i}"), &"x".repeat(512))
            .await
            .unwrap();
    }
    kv.sweep_older_than(chrono::Utc::now().timestamp_millis() + 1000)
        .await
        .unwrap();
    kv.vacuum().await.unwrap();
    kv.put("ns", "after", "v").await.unwrap();
    assert_eq!(kv.get("ns", "after").await.unwrap().as_deref(), Some("v"));
}

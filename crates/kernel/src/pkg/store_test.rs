//! 安装记录 store 测试：内存 sqlite + migrations，upsert/get/list/delete 全链。

use super::{ExtInstall, ExtInstallStore, Resources, SqliteExtInstallStore};

async fn test_store() -> SqliteExtInstallStore {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::storage::migrations::run_migrations(&pool)
        .await
        .unwrap();
    SqliteExtInstallStore::new(pool)
}

fn sample(name: &str, version: &str) -> ExtInstall {
    ExtInstall {
        name: name.to_string(),
        source: format!("/tmp/{name}"),
        mode: "symlink".to_string(),
        version: version.to_string(),
        resources: Resources {
            cron: vec![format!("ext:{name}:dream")],
            hooks: vec!["pre_tool_use/50-guard".to_string()],
            bins: vec!["recall".to_string()],
            snippets: vec!["memory.md".to_string()],
        },
        installed_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn roundtrip() {
    let store = test_store().await;
    store.upsert(&sample("demo", "0.1.0")).await.unwrap();

    let got = store.get("demo").await.unwrap().unwrap();
    assert_eq!(got.version, "0.1.0");
    assert_eq!(got.resources.cron, vec!["ext:demo:dream"]);
    assert_eq!(got.resources.bins, vec!["recall"]);

    assert!(store.get("nope").await.unwrap().is_none());
}

#[tokio::test]
async fn upsert_overwrites() {
    let store = test_store().await;
    store.upsert(&sample("demo", "0.1.0")).await.unwrap();
    store.upsert(&sample("demo", "0.2.0")).await.unwrap();

    let all = store.list().await.unwrap();
    assert_eq!(all.len(), 1, "upsert by name, no duplicates");
    assert_eq!(all[0].version, "0.2.0");
}

#[tokio::test]
async fn delete_returns_affected() {
    let store = test_store().await;
    store.upsert(&sample("demo", "0.1.0")).await.unwrap();
    assert!(store.delete("demo").await.unwrap());
    assert!(!store.delete("demo").await.unwrap());
    assert!(store.list().await.unwrap().is_empty());
}

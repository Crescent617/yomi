use super::*;

use crate::storage::migrations::run_migrations;
use crate::storage::session::{sqlite::SqliteSessionStore, SessionStore};
use crate::types::SessionId;
use std::collections::BTreeMap;
use std::sync::Arc;

async fn test_session_store() -> Arc<dyn SessionStore> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    Arc::new(SqliteSessionStore::new(pool))
}

fn shared_with(
    models: BTreeMap<String, ModelConfig>,
    default_model: &str,
    session_store: Option<Arc<dyn SessionStore>>,
) -> AgentShared {
    AgentShared::with_data_dir(
        Arc::new(models),
        default_model.to_string(),
        None,
        None,
        None,
        session_store,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        std::path::PathBuf::from("."),
    )
}

fn model(name: &str) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        model_id: format!("{name}-id"),
        ..ModelConfig::default()
    }
}

#[tokio::test]
async fn test_resolve_model_uses_session_model_key() {
    let store = test_session_store().await;
    let sid = SessionId::new();
    store
        .create(&sid, None, None, None, None, Some("alt"))
        .await
        .unwrap();

    let mut models = BTreeMap::new();
    models.insert("default".to_string(), model("default"));
    models.insert("alt".to_string(), model("alt"));
    let shared = shared_with(models, "default", Some(store));

    let (_, cfg) = shared.resolve_model(&sid).await.unwrap();
    assert_eq!(cfg.name, "alt");
}

#[tokio::test]
async fn test_resolve_model_falls_back_on_stale_key() {
    let store = test_session_store().await;
    let sid = SessionId::new();
    // model_key points to a model that no longer exists in the registry
    store
        .create(&sid, None, None, None, None, Some("removed-model"))
        .await
        .unwrap();

    let mut models = BTreeMap::new();
    models.insert("default".to_string(), model("default"));
    let shared = shared_with(models, "default", Some(store));

    let (_, cfg) = shared
        .resolve_model(&sid)
        .await
        .expect("stale model_key must fall back to default_model");
    assert_eq!(cfg.name, "default");
}

#[tokio::test]
async fn test_resolve_model_errors_when_default_missing() {
    let shared = shared_with(BTreeMap::new(), "default", None);
    let sid = SessionId::new();
    assert!(shared.resolve_model(&sid).await.is_err());
}

#[tokio::test]
async fn test_resolve_model_without_session_store_uses_default() {
    let mut models = BTreeMap::new();
    models.insert("default".to_string(), model("default"));
    let shared = shared_with(models, "default", None);
    let sid = SessionId::new();
    let (_, cfg) = shared.resolve_model(&sid).await.unwrap();
    assert_eq!(cfg.name, "default");
}

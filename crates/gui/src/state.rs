use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use kernel::client::CoordinatorApi;

pub struct AppState {
    pub coordinator: Arc<dyn CoordinatorApi>,
    pub data_dir: std::path::PathBuf,
    pub active_session: Arc<Mutex<Option<String>>>,
    pub event_tasks: Arc<Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>>,
}

impl AppState {
    /// Creates a new `AppState` backed by the given `CoordinatorApi`.
    pub fn new(coordinator: Arc<dyn CoordinatorApi>, data_dir: std::path::PathBuf) -> Self {
        Self {
            coordinator,
            data_dir,
            active_session: Arc::new(Mutex::new(None)),
            event_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn stop_event_task(&self, session_id: &str) {
        let mut tasks = self.event_tasks.lock().await;
        if let Some(handle) = tasks.remove(session_id) {
            handle.abort();
        }
    }
}

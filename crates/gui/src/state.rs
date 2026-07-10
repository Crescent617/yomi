use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use kernel::client::KernelApi;

pub struct AppState {
    pub kernel: Arc<dyn KernelApi>,
    /// Mutable because a daemon restart may reload a config with a
    /// different `data_dir`.
    pub data_dir: std::sync::Arc<std::sync::RwLock<std::path::PathBuf>>,
    pub active_session: Arc<Mutex<Option<String>>>,
    pub event_tasks: Arc<Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>>,
}

impl AppState {
    /// Creates a new `AppState` backed by the given `KernelApi`.
    pub fn new(kernel: Arc<dyn KernelApi>, data_dir: std::path::PathBuf) -> Self {
        Self {
            kernel,
            data_dir: std::sync::Arc::new(std::sync::RwLock::new(data_dir)),
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

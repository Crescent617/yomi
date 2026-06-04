use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use kernel::client::CoordinatorApi;

use crate::terminal::manager::TerminalManager;

pub struct AppState {
    pub coordinator: Arc<dyn CoordinatorApi>,
    pub active_session: Arc<Mutex<Option<String>>>,
    pub event_tasks: Arc<Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>>,
    pub terminal_manager: Arc<Mutex<TerminalManager>>,
}

impl AppState {
    /// Creates a new `AppState` backed by the given `CoordinatorApi`.
    ///
    /// DESIGN PRINCIPLE: The GUI layer never holds storage (e.g. `CronStore`)
    /// directly. All operations — including cron jobs — go through the
    /// `coordinator`, so the same code works for both local (in-process) and
    /// remote (IPC) kernel connections.
    pub fn new(coordinator: Arc<dyn CoordinatorApi>) -> Self {
        Self {
            coordinator,
            active_session: Arc::new(Mutex::new(None)),
            event_tasks: Arc::new(Mutex::new(HashMap::new())),
            terminal_manager: Arc::new(Mutex::new(TerminalManager::new())),
        }
    }

    pub async fn stop_event_task(&self, session_id: &str) {
        let mut tasks = self.event_tasks.lock().await;
        if let Some(handle) = tasks.remove(session_id) {
            handle.abort();
        }
    }

    #[allow(dead_code)]
    pub async fn remove_event_task(&self, session_id: &str) {
        let mut tasks = self.event_tasks.lock().await;
        tasks.remove(session_id);
    }
}

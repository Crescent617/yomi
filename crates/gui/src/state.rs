use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use kernel::client::RemoteCoordinator;

use crate::error::GuiError;
use crate::terminal::manager::TerminalManager;

pub struct AppState {
    pub coordinator: Arc<Mutex<Option<Arc<RemoteCoordinator>>>>,
    pub active_session: Arc<Mutex<Option<String>>>,
    pub event_tasks: Arc<Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>>,
    pub terminal_manager: Arc<Mutex<TerminalManager>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            coordinator: Arc::new(Mutex::new(None)),
            active_session: Arc::new(Mutex::new(None)),
            event_tasks: Arc::new(Mutex::new(HashMap::new())),
            terminal_manager: Arc::new(Mutex::new(TerminalManager::new())),
        }
    }

    pub async fn get_or_connect(&self) -> Result<Arc<RemoteCoordinator>, GuiError> {
        let mut guard = self.coordinator.lock().await;
        if let Some(coord) = guard.as_ref() {
            return Ok(Arc::clone(coord));
        }

        let addr = kernel::transport::socket_addr();
        let coord = Arc::new(
            RemoteCoordinator::connect(&addr)
                .await
                .map_err(GuiError::kernel)?,
        );
        *guard = Some(Arc::clone(&coord));
        Ok(coord)
    }

    pub async fn disconnect(&self) {
        let mut guard = self.coordinator.lock().await;
        *guard = None;
    }

    pub async fn stop_event_task(&self, session_id: &str) {
        let mut tasks = self.event_tasks.lock().await;
        if let Some(handle) = tasks.remove(session_id) {
            handle.abort();
        }
    }
}

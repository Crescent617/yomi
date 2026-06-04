use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use kernel::client::CoordinatorApi;

use crate::terminal::manager::TerminalManager;

pub struct AppState {
    pub coordinator: Arc<dyn CoordinatorApi>,
    pub active_session: Arc<Mutex<Option<String>>>,
    pub event_tasks: Arc<Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>>,
    pub terminal_manager: Arc<Mutex<TerminalManager>>,
    /// Shutdown token for the cron subsystem (only present in GUI in-process mode).
    cron_shutdown: Option<CancellationToken>,
    /// Direct access to cron store for automation CRUD commands.
    /// The GUI is always in-process, so we can hold this directly.
    pub cron_store: Option<Arc<dyn kernel::CronStore>>,
    /// Handle to the in-process cron scheduler so mutations can trigger reloads.
    pub cron_scheduler: Option<Arc<kernel::cron::CronScheduler>>,
}

impl AppState {
    /// Creates a new `AppState` backed by the given `CoordinatorApi`.
    pub fn new(
        coordinator: Arc<dyn CoordinatorApi>,
        cron_shutdown: Option<CancellationToken>,
        cron_store: Option<Arc<dyn kernel::CronStore>>,
        cron_scheduler: Option<Arc<kernel::cron::CronScheduler>>,
    ) -> Self {
        Self {
            coordinator,
            active_session: Arc::new(Mutex::new(None)),
            event_tasks: Arc::new(Mutex::new(HashMap::new())),
            terminal_manager: Arc::new(Mutex::new(TerminalManager::new())),
            cron_shutdown,
            cron_store,
            cron_scheduler,
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

impl Drop for AppState {
    fn drop(&mut self) {
        if let Some(ref shutdown) = self.cron_shutdown {
            shutdown.cancel();
        }
    }
}

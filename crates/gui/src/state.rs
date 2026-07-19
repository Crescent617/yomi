use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use kernel::client::KernelApi;
use tokio::sync::{Mutex, Notify};

use crate::pet::PetRuntime;

#[derive(Clone)]
pub struct AppState {
    pub kernel: Arc<dyn KernelApi>,
    /// Mutable because a daemon restart may reload a config with a
    /// different `data_dir`.
    pub data_dir: Arc<std::sync::RwLock<std::path::PathBuf>>,
    /// Fixed at GUI startup because the tracing appender cannot be reconfigured.
    pub gui_log_dir: std::path::PathBuf,
    pub active_session: Arc<Mutex<Option<String>>>,
    pub event_tasks: Arc<Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>>,
    pub pet_runtime: Arc<Mutex<PetRuntime>>,
    pub pet_runtime_notify: Arc<Notify>,
    pub pet_runtime_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub selected_pet_id: Arc<std::sync::RwLock<Option<String>>>,
    pub pet_scale: Arc<std::sync::RwLock<f64>>,
    pet_enabled: Arc<AtomicBool>,
}

impl AppState {
    /// Creates a new `AppState` backed by the given `KernelApi`.
    pub fn new(
        kernel: Arc<dyn KernelApi>,
        data_dir: std::path::PathBuf,
        gui_log_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            kernel,
            data_dir: Arc::new(std::sync::RwLock::new(data_dir)),
            gui_log_dir,
            active_session: Arc::new(Mutex::new(None)),
            event_tasks: Arc::new(Mutex::new(HashMap::new())),
            pet_runtime: Arc::new(Mutex::new(PetRuntime::default())),
            pet_runtime_notify: Arc::new(Notify::new()),
            pet_runtime_task: Arc::new(Mutex::new(None)),
            selected_pet_id: Arc::new(std::sync::RwLock::new(None)),
            pet_scale: Arc::new(std::sync::RwLock::new(1.0)),
            pet_enabled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn stop_event_task(&self, session_id: &str) {
        let mut tasks = self.event_tasks.lock().await;
        if let Some(handle) = tasks.remove(session_id) {
            handle.abort();
        }
    }

    pub fn set_pet_enabled(&self, enabled: bool) {
        self.pet_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_pet_enabled(&self) -> bool {
        self.pet_enabled.load(std::sync::atomic::Ordering::Relaxed)
    }
}

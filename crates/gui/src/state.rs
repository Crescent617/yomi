use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use kernel::client::KernelApi;
use tokio::sync::{Mutex, Notify};

use crate::pet::PetRuntime;

/// Which daemon the GUI currently talks to.
#[derive(Debug, Clone)]
pub enum ConnectionMode {
    /// Local daemon (spawned by this GUI or started externally), resolved
    /// via the standard socket address resolution.
    Local,
    /// Remote daemon at an explicit socket address.
    Remote(kernel::transport::SocketAddr),
}

#[derive(Clone)]
struct ConnectionState {
    kernel: Arc<dyn KernelApi>,
    mode: ConnectionMode,
}

#[derive(Clone)]
pub struct AppState {
    connection: Arc<std::sync::RwLock<ConnectionState>>,
    /// Serializes complete connect/validate/swap operations.
    pub connection_switch: Arc<Mutex<()>>,
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
            connection: Arc::new(std::sync::RwLock::new(ConnectionState {
                kernel,
                mode: ConnectionMode::Local,
            })),
            connection_switch: Arc::new(Mutex::new(())),
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

    /// Swap the kernel the GUI talks to (local <-> remote daemon).
    ///
    /// The old kernel is stopped, which closes its connection and any
    /// streams subscribed through it; subscribers re-subscribe onto the new
    /// kernel.
    pub fn kernel_snapshot(&self) -> Arc<dyn KernelApi> {
        self.connection
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .kernel
            .clone()
    }

    pub fn swap_kernel(&self, next: Arc<dyn KernelApi>, mode: ConnectionMode) {
        let old = {
            let mut guard = self.connection.write().unwrap_or_else(|e| e.into_inner());
            let old = Arc::clone(&guard.kernel);
            *guard = ConnectionState { kernel: next, mode };
            old
        };
        // fire-and-forget 优雅关停：旧 kernel 的持久化排空在后台走
        // 完（GUI 进程存活，drain 不会被进程退出截断；runtime 外调
        // 用 swap 的异常路径退化为同步 `stop`——fresh-eyes 复审）。
        // 已知窗口：swap 后进程随即退出时 drain 仍被截断（秒级，
        // 与旧 `stop()` 同型）。
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                old.graceful_stop().await;
            });
        } else {
            old.stop();
        }
    }

    pub fn connection_mode(&self) -> ConnectionMode {
        self.connection
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .mode
            .clone()
    }

    pub fn set_pet_enabled(&self, enabled: bool) {
        self.pet_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_pet_enabled(&self) -> bool {
        self.pet_enabled.load(std::sync::atomic::Ordering::Relaxed)
    }
}

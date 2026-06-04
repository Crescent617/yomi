#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod commands;
mod daemon;
mod error;
mod state;
mod terminal;

use std::sync::Arc;

use state::AppState;
use tauri::Manager;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            let init = tauri::async_runtime::block_on(daemon::init_coordinator())
                .map_err(|e| format!("failed to initialise kernel coordinator: {e}"))?;
            let coordinator: Arc<dyn kernel::client::CoordinatorApi> = init.coordinator;
            app.manage(AppState::new(coordinator));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::project::list_projects,
            commands::project::create_project,
            commands::project::get_project,
            commands::project::rename_project,
            commands::project::delete_project,
            commands::session::list_sessions,
            commands::session::create_session,
            commands::session::restore_session,
            commands::session::fork_session,
            commands::session::delete_session,
            commands::session::shutdown_session,
            commands::chat::send_message,
            commands::chat::send_message_blocks,
            commands::chat::subscribe,
            commands::chat::unsubscribe,
            commands::chat::get_messages,
            commands::chat::cancel_session,
            commands::chat::respond_permission,
            commands::chat::respond_ask_user,
            commands::chat::compact_session,
            commands::chat::get_todos,
            commands::chat::set_permission_level,
            commands::chat::start_goal,
            commands::chat::rename_session,
            commands::chat::send_steer,
            commands::chat::stop_goal,
            commands::checkpoint::get_checkpoints,
            commands::checkpoint::rewind,
            commands::skill::list_skills,
            commands::skill::reload_config,
            commands::system::ping,
            commands::system::get_cwd,
            commands::system::get_config_toml,
            commands::system::save_config_toml,
            commands::system::get_config,
            commands::system::get_usage_summary,
            commands::system::get_daily_usage,
            commands::system::get_session_usage,
            commands::system::open_in_explorer,
            commands::system::open_in_vscode,
            commands::system::open_in_zed,
            commands::system::open_in_editor,
            commands::terminal::terminal_spawn,
            commands::terminal::terminal_write,
            commands::terminal::terminal_resize,
            commands::terminal::terminal_kill,
        ]);

    #[cfg(debug_assertions)]
    {
        builder = builder.plugin(tauri_plugin_pilot::init());
    }

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    let _guard = init_logging();
    run();
}

/// Initialise rolling-file logging to `~/.yomi/logs/app.log`.
/// Falls back to stderr-only if the log directory cannot be created.
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let config_file = kernel::config::Config::discover_file();
    let mut config = config_file
        .as_ref()
        .and_then(|p| kernel::config::Config::from_file(p).ok())
        .unwrap_or_default();

    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    config.finalize(&working_dir);

    let log_dir = config.log_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "Failed to create log directory '{}': {e}. Logging to stderr only.",
            log_dir.display()
        );
        let _ = tracing_subscriber::fmt::try_init();
        return None;
    }

    let log_path = log_dir.join("gui-app.log");
    let file_appender = match tracing_rolling_file::RollingFileAppenderBase::builder()
        .filename(log_path.to_string_lossy().to_string())
        .condition_max_file_size(10 * 1024 * 1024)
        .max_filecount(5)
        .build()
    {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "Failed to create rolling file appender for '{}': {e}. Logging to stderr only.",
                log_path.display()
            );
            let _ = tracing_subscriber::fmt::try_init();
            return None;
        }
    };

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    if tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true),
        )
        .try_init()
        .is_ok()
    {
        tracing::info!("Logging initialised. Log directory: {}", log_dir.display());
        Some(guard)
    } else {
        drop(guard);
        None
    }
}

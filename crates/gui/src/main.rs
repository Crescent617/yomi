#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod commands;
mod daemon;
mod error;
mod state;

use state::AppState;
use tauri::Manager;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            let coordinator = tauri::async_runtime::block_on(daemon::get_coordinator())
                .map_err(|e| format!("failed to get coordinator: {e}"))?;
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
            commands::chat::get_session_status,
            commands::chat::cancel_session,
            commands::chat::respond_permission,
            commands::chat::respond_ask_user,
            commands::chat::compact_session,
            commands::chat::get_todos,
            commands::chat::set_permission_level,
            commands::chat::start_goal,
            commands::chat::pause_goal,
            commands::chat::resume_goal,
            commands::chat::edit_goal,
            commands::chat::get_goal,
            commands::chat::rename_session,
            commands::chat::continue_session,
            commands::chat::send_steer,
            commands::chat::stop_goal,
            commands::automation::list_cron_jobs,
            commands::automation::create_cron_job,
            commands::automation::update_cron_job,
            commands::automation::delete_cron_job,
            commands::automation::trigger_cron_job,
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
            commands::system::open_in_browser,
            commands::system::open_in_explorer,
            commands::system::open_in_vscode,
            commands::system::open_in_zed,
            commands::system::open_in_editor,
            commands::system::get_git_info,
            commands::system::get_git_diff_summary,
            commands::system::get_git_file_diff_raw,
        ]);

    #[cfg(debug_assertions)]
    let builder = builder.plugin(tauri_plugin_pilot::init());

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            tauri::async_runtime::block_on(async {
                if let Err(e) = daemon::stop_daemon().await {
                    tracing::warn!("Failed to stop daemon: {e}");
                }
            });
        }
    });
}

fn main() {
    // Load ~/.env before anything else so env vars are available to the app.
    if let Some(home) = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
    {
        let env_path = std::path::Path::new(&home).join(".env");
        if env_path.exists() {
            if let Err(e) = dotenvy::from_path(&env_path) {
                eprintln!("Failed to load {}: {e}", env_path.display());
            }
        }
    }

    if let Err(e) = fix_path_env::fix() {
        tracing::warn!("Failed to fix PATH environment: {e}");
    }
    let _guard = init_logging();
    run();
}

/// Initialise daily-rotating file logging to `~/.yomi/logs/gui-app.<date>.log` **and stderr**.
/// Falls back to stderr-only if the log directory cannot be created.
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let config_file = kernel::config::Config::discover_file();
    let mut config = config_file
        .as_ref()
        .and_then(|p| kernel::config::Config::from_file(p).ok())
        .unwrap_or_default();

    config.finalize();

    let log_dir = config.log_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "Failed to create log directory '{}': {e}. Logging to stderr only.",
            log_dir.display()
        );
        let _ = tracing_subscriber::fmt::try_init();
        return None;
    }

    let file_appender = match tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("gui-app")
        .filename_suffix("log")
        .build(&log_dir)
    {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "Failed to create rolling file appender in '{}': {e}. Logging to stderr only.",
                log_dir.display()
            );
            let _ = tracing_subscriber::fmt::try_init();
            return None;
        }
    };

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true);

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(true);

    if tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer)
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

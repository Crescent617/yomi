#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod commands;
mod daemon;
mod error;
mod state;

use state::AppState;
use tauri::Emitter;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            let (kernel, data_dir) = tauri::async_runtime::block_on(daemon::get_kernel())
                .map_err(|e| format!("failed to get kernel: {e}"))?;
            app.manage(AppState::new(kernel.clone(), data_dir));

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut rx = match kernel.subscribe_notifications().await {
                    Ok(rx) => rx,
                    Err(e) => {
                        tracing::warn!("Failed to subscribe to notifications: {e}");
                        return;
                    }
                };
                while let Some(noti) = rx.recv().await {
                    let payload = serde_json::to_value(&noti).unwrap_or_default();
                    let _ = app_handle.emit("kernel:noti", payload);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::project::list_projects,
            commands::project::create_project,
            commands::project::get_project,
            commands::project::rename_project,
            commands::project::delete_project,
            commands::session::list_sessions,
            commands::session::list_subagents,
            commands::session::create_session,
            commands::session::restore_session,
            commands::session::fork_session,
            commands::session::delete_session,
            commands::session::shutdown_session,
            commands::session::clear_session,
            commands::session::pin_session,
            commands::session::unpin_session,
            commands::session::set_pinned_session_emoji,
            commands::session::list_pinned_sessions,
            commands::chat::send_message,
            commands::chat::send_message_blocks,
            commands::chat::subscribe,
            commands::chat::unsubscribe,
            commands::chat::get_messages,
            commands::chat::get_session,
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
            commands::skill::list_session_skills,
            commands::skill::reload_config,
            commands::system::read_asset,
            commands::system::ping,
            commands::system::get_daemon_status,
            commands::system::restart_daemon,
            commands::system::get_cwd,
            commands::system::get_config_toml,
            commands::system::save_config_toml,
            commands::system::get_config,
            commands::system::get_usage_summary,
            commands::system::get_daily_usage,
            commands::system::get_model_usage,
            commands::system::get_today_model_usage,
            commands::system::get_models,
            commands::system::get_session_model,
            commands::system::set_session_model,
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

    let mut config = kernel::config::Config::discover_file()
        .and_then(|p| kernel::config::Config::from_file(&p).ok())
        .unwrap_or_default();
    if let Err(e) = config.inject_env() {
        eprintln!("Failed to inject config environment: {e}");
    }
    config.apply_env_overrides();
    config.finalize();

    let _guard = kernel::utils::logging::init_logging(&config, "gui", true).unwrap_or_else(|e| {
        eprintln!("Failed to initialize file logging: {e}. Logging to stderr only.");
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
            .try_init();
        None
    });
    run();
}

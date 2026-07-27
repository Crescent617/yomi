#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod commands;
mod daemon;
mod error;
mod pet;
mod pet_pack;
#[cfg(test)]
mod pet_pack_test;
mod pet_runtime;
#[cfg(test)]
mod pet_test;
mod state;

use state::AppState;
use tauri::Emitter;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Install rustls crypto provider before any TLS operations.
    // Required by rustls 0.23+ when multiple crypto providers (ring and
    // aws-lc-rs via reqwest) are available in the dependency tree. Must
    // happen before the setup hook, which may connect over WSS when
    // YOMI_SOCKET points to a wss:// endpoint.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            let (kernel, data_dir) = tauri::async_runtime::block_on(daemon::get_kernel())
                .map_err(|e| format!("failed to get kernel: {e}"))?;
            let mut restart_rx = daemon::take_restart_receiver()
                .ok_or_else(|| "daemon restart receiver was already taken".to_string())?;
            let gui_log_dir = commands::debug::configured_log_dir();
            let state = AppState::new(kernel.clone(), data_dir, gui_log_dir);
            app.manage(state.clone());

            // Snapshot the current kernel for each subscription attempt so
            // retries follow local/remote swaps.
            let notif_kernel = state.clone();

            let pet_app_handle = app.handle().clone();
            let pet_state = state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) =
                    pet_runtime::start_pet_runtime(&pet_state, &pet_app_handle).await
                {
                    tracing::warn!("Failed to start desktop pet runtime: {error}");
                }
            });

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Resubscribe loop: when the kernel is swapped (remote mode
                // switch), the old notification channel closes and we
                // re-subscribe onto the new kernel.
                loop {
                    let mut rx = match notif_kernel
                        .kernel_snapshot()
                        .subscribe_notifications()
                        .await
                    {
                        Ok(rx) => rx,
                        Err(e) => {
                            tracing::warn!("Failed to subscribe to notifications: {e}");
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            continue;
                        }
                    };
                    while let Some(noti) = rx.recv().await {
                        let payload = serde_json::to_value(&noti).unwrap_or_default();
                        let _ = app_handle.emit("kernel:noti", payload);
                    }
                    tracing::debug!("Kernel notification channel closed; resubscribing");
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            });

            let restart_app_handle = app.handle().clone();
            let restart_state = state.clone();
            tauri::async_runtime::spawn(async move {
                while restart_rx.recv().await.is_some() {
                    match daemon::restart_daemon().await {
                        Ok(config) => {
                            if let Ok(mut data_dir) = restart_state.data_dir.write() {
                                *data_dir = config.data_dir;
                            }
                            let _ = restart_app_handle.emit("kernel:restarted", ());
                        }
                        Err(error) => {
                            tracing::error!(%error, "failed to restart GUI-managed daemon");
                        }
                    }
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
            commands::session::list_running_sessions,
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
            commands::pet::get_pet_state,
            commands::pet::set_pet_enabled,
            commands::pet::list_pet_packs,
            commands::pet::select_pet_pack,
            commands::pet::get_selected_pet_pack,
            commands::pet::read_selected_pet_spritesheet,
            commands::pet::get_pet_scale,
            commands::pet::set_pet_scale,
            commands::automation::list_cron_jobs,
            commands::automation::create_cron_job,
            commands::automation::update_cron_job,
            commands::automation::delete_cron_job,
            commands::automation::trigger_cron_job,
            commands::checkpoint::get_checkpoints,
            commands::checkpoint::rewind,
            commands::favorite::add_favorite,
            commands::favorite::remove_favorite,
            commands::favorite::remove_favorite_by_message,
            commands::favorite::list_favorites,
            commands::favorite::update_favorite_note,
            commands::skill::list_session_skills,
            commands::skill::reload_config,
            commands::debug::list_gui_logs,
            commands::debug::read_session_jsonl,
            commands::debug::read_gui_log,
            commands::system::read_asset,
            commands::system::ping,
            commands::system::get_daemon_status,
            commands::system::get_connection_info,
            commands::system::connect_remote,
            commands::system::disconnect_remote,
            commands::system::restart_daemon,
            commands::system::get_cwd,
            commands::system::get_config_toml,
            commands::system::save_config_toml,
            commands::system::get_config,
            commands::system::get_usage_summary,
            commands::system::get_daily_usage,
            commands::system::get_model_usage,
            commands::system::get_today_model_usage,
            commands::system::get_usage_records,
            commands::system::get_models,
            commands::system::get_session_model,
            commands::system::set_session_model,
            commands::system::open_default,
            commands::system::open_attachment,
            commands::system::read_attachment_image,
            commands::system::open_in_vscode,
            commands::system::open_in_zed,
            commands::system::get_git_info,
            commands::system::get_git_diff_summary,
            commands::system::get_git_file_diff_raw,
        ]);

    #[cfg(debug_assertions)]
    let builder = builder.plugin(tauri_plugin_pilot::init());

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let process_shutdown = tokio_util::sync::CancellationToken::new();
        let _signal_handle = kernel::utils::signal::spawn_signal_listener(process_shutdown.clone());
        process_shutdown.cancelled().await;
        app_handle.exit(0);
    });

    app.run(|app_handle, event| match event {
        tauri::RunEvent::WindowEvent { label, event, .. }
            if label == "main" && matches!(event, tauri::WindowEvent::Destroyed) =>
        {
            if let Some(window) = app_handle.get_webview_window("pet") {
                let _ = window.destroy();
            }
        }
        tauri::RunEvent::Exit => {
            tauri::async_runtime::block_on(async {
                if let Err(e) = daemon::stop_daemon().await {
                    tracing::warn!("Failed to stop daemon: {e}");
                }
            });
        }
        _ => {}
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

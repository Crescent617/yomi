#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod commands;
mod daemon;
mod error;
mod state;
mod terminal;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|_app| {
            // Spawn the yomi daemon inside Tauri's async runtime so the
            // background server task survives the whole app lifetime.
            tauri::async_runtime::spawn(async move {
                if let Err(e) = daemon::spawn_daemon().await {
                    tracing::warn!("failed to spawn daemon: {e}");
                }
            });
            Ok(())
        })
        .manage(AppState::new())
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
            commands::chat::subscribe,
            commands::chat::unsubscribe,
            commands::chat::get_messages,
            commands::checkpoint::get_checkpoints,
            commands::checkpoint::rewind,
            commands::skill::list_skills,
            commands::skill::reload_config,
            commands::system::ping,
            commands::system::get_cwd,
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
    // 先初始化简单日志，确保 spawn_daemon 的错误能输出
    let _ = tracing_subscriber::fmt::try_init();
    run();
}

#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod commands;
mod daemon;
mod error;
mod state;
mod terminal;

use std::sync::Arc;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let coordinator = tauri::async_runtime::block_on(daemon::init_coordinator())
                .map_err(|e| format!("failed to initialise kernel coordinator: {e}"))?;
            let coordinator: Arc<dyn kernel::client::CoordinatorApi> = coordinator;
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
    // 先初始化简单日志，确保 spawn_daemon 的错误能输出
    let _ = tracing_subscriber::fmt::try_init();
    run();
}

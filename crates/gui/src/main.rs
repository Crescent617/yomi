#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod commands;
mod error;
mod state;
mod terminal;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
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
            commands::terminal::terminal_spawn,
            commands::terminal::terminal_write,
            commands::terminal::terminal_resize,
            commands::terminal::terminal_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}

mod commands;
mod engine;
mod error;
mod model;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::repo_open,
            commands::repo_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

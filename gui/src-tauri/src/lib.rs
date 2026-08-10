mod changelists;
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
            commands::changelist_create,
            commands::changelist_rename,
            commands::changelist_set_comment,
            commands::changelist_delete,
            commands::changelist_set_active,
            commands::files_move,
            commands::file_rollback,
            commands::list_rollback,
            commands::diff_file,
            commands::hunk_stage,
            commands::hunk_unstage,
            commands::hunk_revert,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

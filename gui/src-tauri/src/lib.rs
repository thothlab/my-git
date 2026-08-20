mod changelists;
mod commands;
mod engine;
mod error;
mod model;
mod uistate;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::repo_open,
            commands::repo_state,
            commands::set_show_ignored,
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
            commands::commit_list,
            commands::branch_list,
            commands::branch_create,
            commands::branch_checkout,
            commands::push,
            commands::fetch,
            commands::pull,
            commands::log_page,
            commands::log_authors,
            commands::commit_details,
            commands::commit_files,
            commands::commit_file_diff,
            commands::commits_compare,
            commands::commits_unreachable,
            commands::commits_compare_diff,
            commands::branch_tree,
            commands::branch_rename,
            commands::branch_delete,
            commands::branch_unmerged_count,
            commands::branch_merge,
            commands::branch_rebase_onto,
            commands::commit_revert,
            commands::commit_reset,
            commands::commit_cherry_pick,
            commands::commit_checkout,
            commands::commit_contains,
            commands::commit_reset_lost_count,
            commands::repo_local_changes,
            commands::tag_create,
            commands::op_continue,
            commands::op_abort,
            commands::op_skip,
            commands::stash_list_app,
            commands::stash_restore,
            commands::ui_state_get,
            commands::ui_state_set,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

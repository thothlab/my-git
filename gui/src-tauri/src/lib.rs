mod changelists;
mod commands;
mod engine;
mod error;
mod model;
mod uistate;
mod watch;

use commands::AppState;
use tauri::{Emitter, Manager};
// `Menu` is built on every platform (`Menu::default` below runs
// unconditionally); `MenuItem`/`MenuItemKind` are only reached inside the
// `#[cfg(target_os = "macos")]` block that customises the app submenu, so
// only those two stay gated - an ungated `Menu` import that used to sit
// behind the same `#[cfg(target_os = "macos")]` left `Menu` unresolved on
// Linux/Windows: `cargo build -p graft` never catches that on a macOS dev
// machine, only a non-macOS CI leg does.
use tauri::menu::Menu;
#[cfg(target_os = "macos")]
use tauri::menu::{MenuItem, MenuItemKind};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState::default())
        // Interactive rebase keeps its plan in the application data directory
        // (`engine::rebase`). Resolved once here; a platform that cannot name it
        // leaves `None`, and only rewriting history is refused for it.
        .setup(|app| {
            if let Ok(dir) = app.path().app_data_dir() {
                *app.state::<AppState>().data_dir.lock().unwrap() = Some(dir);
            }
            Ok(())
        })
        // The default macOS app menu's About item opens the native panel
        // directly and never reaches `on_menu_event`, so it can't show our
        // own About dialog. Everything else in the default menu (Edit's
        // clipboard shortcuts, Window, Help) stays untouched — only the
        // About item is swapped for one with our own id, and a "Check for
        // Updates…" item is inserted right after it (the platform
        // convention: About, Check for Updates…, then the default
        // separator ahead of Services). Position 0 is where `Menu::default`
        // puts About (see tauri's menu.rs); guarded by a type check so a
        // future Tauri reshuffling this submenu drops nothing silently.
        .menu(|app| {
            let menu = Menu::default(app)?;
            #[cfg(target_os = "macos")]
            if let Some(MenuItemKind::Submenu(app_submenu)) = menu.items()?.first() {
                let removed = app_submenu.items()?.first().cloned();
                if let Some(MenuItemKind::Predefined(_)) = removed {
                    let about_text = format!("About {}", app_submenu.text()?);
                    let about = MenuItem::with_id(app, "about", about_text, true, None::<&str>)?;
                    app_submenu.remove_at(0)?;
                    app_submenu.insert(&about, 0)?;
                    let check_for_updates = MenuItem::with_id(
                        app,
                        "check-for-updates",
                        "Check for Updates…",
                        true,
                        None::<&str>,
                    )?;
                    app_submenu.insert(&check_for_updates, 1)?;
                }
            }
            Ok(menu)
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "about" => {
                let _ = app.emit("open-about", ());
            }
            "check-for-updates" => {
                let _ = app.emit("check-for-updates", ());
            }
            _ => {}
        })
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
            commands::discard_list,
            commands::discard_check,
            commands::discard_restore,
            commands::diff_file,
            commands::file_read,
            commands::file_write,
            commands::lines_stage,
            commands::lines_unstage,
            commands::lines_revert,
            commands::commit_list,
            commands::branch_list,
            commands::branch_create,
            commands::branch_checkout,
            commands::push,
            commands::fetch,
            commands::pull,
            commands::git_exec,
            commands::remote_list,
            commands::remote_add,
            commands::remote_rename,
            commands::remote_remove,
            commands::remote_set_url,
            commands::repo_clone,
            commands::repo_clone_cancel,
            commands::log_page,
            commands::log_authors,
            commands::commit_details,
            commands::commit_files,
            commands::commit_file_diff,
            commands::file_history,
            commands::file_blame,
            commands::file_blame_before,
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
            commands::op_bisect_start,
            commands::op_bisect_mark,
            commands::op_bisect_reset,
            commands::op_rebase_range,
            commands::op_rebase_start,
            commands::commit_reword,
            commands::commits_squash,
            commands::conflict_read,
            commands::conflict_resolve,
            commands::conflict_take,
            commands::stash_list_app,
            commands::stash_restore,
            commands::stash_list,
            commands::stash_apply,
            commands::stash_pop,
            commands::stash_drop,
            commands::stash_files,
            commands::stash_push,
            commands::branch_update,
            commands::ui_state_get,
            commands::ui_state_set,
            commands::undo_state,
            commands::undo_step,
            commands::journal_list,
            commands::journal_output,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

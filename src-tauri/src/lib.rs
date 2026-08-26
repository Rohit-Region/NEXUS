mod commands;
mod db;

use db::DbState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Initialize logging in debug builds.
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Initialize the SQLite database and register it as managed state.
            let conn = db::init(app.handle())
                .expect("Failed to initialize NEXUS database");
            app.manage(DbState(Mutex::new(conn)));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::nexus_get_db_status,
            commands::nexus_get_db_counts,
            commands::nexus_create_project,
            commands::nexus_list_projects,
            commands::nexus_update_project,
            commands::nexus_delete_project,
            commands::nexus_create_task,
            commands::nexus_list_tasks,
            commands::nexus_update_task,
            commands::nexus_update_task_status,
            commands::nexus_delete_task,
            commands::nexus_create_ide,
            commands::nexus_list_ides,
            commands::nexus_update_ide,
            commands::nexus_delete_ide,
            commands::nexus_create_agent,
            commands::nexus_list_agents,
            commands::nexus_update_agent,
            commands::nexus_delete_agent,
            commands::nexus_assign_task_agent,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

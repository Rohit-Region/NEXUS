mod assistant;
mod commands;
mod db;
mod voice;

use assistant::approval::ApprovalStore;
use assistant::session::AssistantSession;
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

            // NEXUS-012: pending approvals live here and nowhere else.
            // Deliberately not persisted, so a restart cannot leave a queue
            // of pre-approved actions waiting to fire.
            app.manage(ApprovalStore::default());

            // NEXUS-013: assistant state and the conversation. In memory and
            // bounded; a conversation that survived a restart would be a
            // transcript on disk, which the voice milestones deliberately
            // avoided creating.
            app.manage(AssistantSession::default());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::nexus_get_db_status,
            commands::nexus_get_db_counts,
            commands::nexus_create_project,
            commands::nexus_list_projects,
            commands::nexus_update_project,
            commands::nexus_create_task,
            commands::nexus_list_tasks,
            commands::nexus_update_task,
            commands::nexus_update_task_status,
            commands::nexus_create_ide,
            commands::nexus_list_ides,
            commands::nexus_update_ide,
            commands::nexus_delete_ide,
            commands::nexus_create_agent,
            commands::nexus_list_agents,
            commands::nexus_update_agent,
            commands::nexus_delete_agent,
            commands::nexus_assign_task_agent,
            commands::nexus_get_workspace_summary,
            commands::nexus_count_tasks_by_project,
            commands::nexus_count_tasks_by_agent,
            commands::nexus_list_recent_tasks,
            commands::nexus_get_settings,
            commands::nexus_update_settings,
            commands::nexus_reset_settings,
            commands::nexus_search_workspace,
            commands::nexus_voice_status,
            commands::nexus_voice_request_authorization,
            commands::nexus_voice_start,
            commands::nexus_voice_stop,
            commands::nexus_resolve_voice_intent,
            commands::nexus_voice_speak,
            commands::nexus_voice_stop_speaking,
            commands::nexus_voice_list_voices,
            commands::nexus_execute_action,
            commands::nexus_cancel_approval,
            commands::nexus_list_connectors,
            commands::nexus_set_permission_grant,
            commands::nexus_set_connector_enabled,
            commands::nexus_set_connector_config,
            commands::nexus_list_audit,
            commands::nexus_reasoning_status,
            commands::nexus_set_reasoning_policy,
            commands::nexus_set_reasoning_model,
            commands::nexus_local_models,
            commands::nexus_list_ai_audit,
            commands::nexus_list_suggestions,
            commands::nexus_dismiss_suggestion,
            commands::nexus_restore_suggestion,
            commands::nexus_surface_suggestions,
            commands::nexus_preview_suggestions,
            commands::nexus_briefing,
            commands::nexus_accept_suggestion,
            commands::nexus_proactive_policy,
            commands::nexus_set_proactive_policy,
            commands::nexus_assistant_snapshot,
            commands::nexus_assistant_context,
            commands::nexus_assistant_resolve,
            commands::nexus_assistant_remember_list,
            commands::nexus_assistant_settle,
            commands::nexus_assistant_clear,
            commands::nexus_assistant_ask,
            commands::nexus_assistant_cancel_turn,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

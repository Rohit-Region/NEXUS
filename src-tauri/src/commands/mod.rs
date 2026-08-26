use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::{
    self,
    projects::{
        count_all_tables, delete_project, insert_project, list_projects, CreateProjectInput,
        Project,
    },
    DbState,
};

/// Returned by nexus_get_db_status.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStatus {
    pub initialized: bool,
    pub migration_level: i64,
    pub db_path: String,
}

/// Returned by nexus_get_db_counts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbCounts {
    pub projects: i64,
    pub tasks: i64,
    pub ai_agents: i64,
    pub ides: i64,
    pub settings: i64,
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// Return current DB status: initialized flag, migration level, file path.
#[tauri::command]
pub fn nexus_get_db_status(
    state: State<'_, DbState>,
    app: tauri::AppHandle,
) -> Result<DbStatus, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let level = db::migration_level(&conn)?;

    use tauri::Manager;
    let db_path = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Path error: {e}"))?
        .join("nexus.db")
        .to_string_lossy()
        .into_owned();

    Ok(DbStatus {
        initialized: true,
        migration_level: level,
        db_path,
    })
}

/// Return record counts for all five tables.
#[tauri::command]
pub fn nexus_get_db_counts(state: State<'_, DbState>) -> Result<DbCounts, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let counts = count_all_tables(&conn)?;
    Ok(DbCounts {
        projects:  counts.projects,
        tasks:     counts.tasks,
        ai_agents: counts.ai_agents,
        ides:      counts.ides,
        settings:  counts.settings,
    })
}

/// Insert a new project and return the full row.
#[tauri::command]
pub fn nexus_create_project(
    state: State<'_, DbState>,
    input: CreateProjectInput,
) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_project(&conn, &input)
}

/// Return all projects ordered by creation date descending.
#[tauri::command]
pub fn nexus_list_projects(state: State<'_, DbState>) -> Result<Vec<Project>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_projects(&conn)
}

/// Delete a project by ID.
#[tauri::command]
pub fn nexus_delete_project(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_project(&conn, id)
}
